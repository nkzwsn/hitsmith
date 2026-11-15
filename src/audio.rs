//! Audio decoding (symphonia), resampling and a tiny real-time mixer (cpal).

use std::fs::File;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, anyhow, bail};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

/// Interleaved stereo f32 PCM.
#[derive(Clone)]
pub struct Pcm {
    pub rate: u32,
    pub data: Vec<f32>,
}

impl Pcm {
    pub fn frames(&self) -> usize {
        self.data.len() / 2
    }
    pub fn duration_ms(&self) -> f64 {
        self.frames() as f64 * 1000.0 / self.rate as f64
    }
}

const MAX_DECODED_SAMPLES: usize = 44_100 * 2 * 60 * 20; // 20 minutes of stereo

pub fn decode_file(path: &Path) -> Result<Pcm> {
    let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let mut format = symphonia::default::get_probe()
        .probe(&hint, mss, FormatOptions::default(), MetadataOptions::default())
        .with_context(|| format!("unsupported audio: {}", path.display()))?;
    let track = format.default_track(TrackType::Audio).ok_or_else(|| anyhow!("no audio track"))?;
    let params = track.codec_params.as_ref().and_then(|p| p.audio()).ok_or_else(|| anyhow!("no audio parameters"))?;
    let mut decoder = symphonia::default::get_codecs().make_audio_decoder(params, &AudioDecoderOptions::default())?;
    let track_id = track.id;

    let mut out = Vec::new();
    let mut scratch: Vec<f32> = Vec::new();
    let mut rate = 0u32;
    loop {
        let packet = match format.next_packet() {
            Ok(Some(p)) => p,
            Ok(None) => break,
            Err(SymError::IoError(_)) => break,
            Err(e) => return Err(e.into()),
        };
        if packet.track_id != track_id {
            continue;
        }
        let buf = match decoder.decode(&packet) {
            Ok(b) => b,
            Err(SymError::DecodeError(_)) => continue,
            Err(e) => return Err(e.into()),
        };
        let spec = buf.spec();
        rate = spec.rate();
        let ch = spec.channels().count().max(1);
        scratch.resize(buf.samples_interleaved(), 0.0);
        buf.copy_to_slice_interleaved(&mut scratch);
        for frame in scratch.chunks_exact(ch) {
            let l = frame[0];
            let r = if ch > 1 { frame[1] } else { l };
            out.push(l);
            out.push(r);
        }
        if out.len() > MAX_DECODED_SAMPLES {
            bail!("audio is too long: {}", path.display());
        }
    }
    if rate == 0 {
        bail!("audio file is empty: {}", path.display());
    }
    Ok(Pcm { rate, data: out })
}

/// Linear-interpolation resampler. Good enough for previews and hitsound samples.
pub fn resample(pcm: &Pcm, target: u32) -> Pcm {
    if pcm.rate == target || pcm.data.is_empty() {
        return pcm.clone();
    }
    let frames_in = pcm.frames();
    let ratio = pcm.rate as f64 / target as f64;
    let frames_out = (frames_in as f64 * target as f64 / pcm.rate as f64).round() as usize;
    let mut data = Vec::with_capacity(frames_out * 2);
    for i in 0..frames_out {
        let pos = i as f64 * ratio;
        let i0 = pos.floor() as usize;
        let i1 = (i0 + 1).min(frames_in - 1);
        let t = (pos - i0 as f64) as f32;
        for c in 0..2 {
            let a = pcm.data[i0 * 2 + c];
            let b = pcm.data[i1 * 2 + c];
            data.push(a + (b - a) * t);
        }
    }
    Pcm { rate: target, data }
}

/// Min/max peaks per block of frames, for waveform drawing.
pub struct Peaks {
    pub block: usize,
    pub rate: u32,
    pub mins: Vec<f32>,
    pub maxs: Vec<f32>,
}

pub fn compute_peaks(pcm: &Pcm, block: usize) -> Peaks {
    let mut mins = Vec::with_capacity(pcm.frames() / block + 1);
    let mut maxs = Vec::with_capacity(pcm.frames() / block + 1);
    for chunk in pcm.data.chunks(block * 2) {
        let (mut lo, mut hi) = (0f32, 0f32);
        for pair in chunk.chunks_exact(2) {
            let v = (pair[0] + pair[1]) * 0.5;
            lo = lo.min(v);
            hi = hi.max(v);
        }
        mins.push(lo);
        maxs.push(hi);
    }
    Peaks { block, rate: pcm.rate, mins, maxs }
}

#[derive(Clone, Copy)]
pub struct Event {
    pub frame: u64,
    pub sample: u32,
    pub gain: f32,
}

struct Voice {
    sample: u32,
    pos: usize,
    gain: f32,
}

/// Scrub feel: how fast the head chases its target, how heavy it is, and its top speed.
const SCRUB_TAU_SECS: f64 = 0.07;
const SCRUB_INERTIA_SECS: f64 = 0.018;
const SCRUB_MAX_SPEED: f64 = 10.0;
/// Length of the fade around an instant jump, so it doesn't click.
const JUMP_FADE_SECS: f64 = 0.005;

#[derive(Default)]
struct Mix {
    song: Arc<Vec<i16>>,
    /// Song position in frames; fractional so speed can change (and run backwards when scrubbing).
    pos: f64,
    rate: f64,
    playing: bool,
    /// While set, the head glides toward this frame like a hand dragging vinyl.
    target: Option<f64>,
    /// Current head velocity in frames per output frame.
    vel: f64,
    /// Play the vinyl-style scrub when seeking by ticks (off: instant jumps).
    scrub_sound: bool,
    /// An instant jump waiting for the fade-out to finish.
    pending_jump: Option<f64>,
    /// Song gain used to fade around jumps (0..1).
    fade: f32,
    sr: f64,
    song_gain: f32,
    hit_gain: f32,
    master: f32,
    samples: Vec<Arc<Vec<f32>>>,
    events: Arc<Vec<Event>>,
    cursor: usize,
    voices: Vec<Voice>,
}

impl Mix {
    fn reset_cursor(&mut self) {
        self.cursor = self.events.partition_point(|e| (e.frame as f64) < self.pos);
        self.voices.clear();
    }
    fn base_speed(&self) -> f64 {
        if self.playing { self.rate } else { 0.0 }
    }
}

/// State the UI reads every frame without touching the mixer lock, so drawing
/// never makes the audio thread wait.
#[derive(Default)]
struct Shared {
    pos: AtomicU64,
    target: AtomicU64,
    playing: AtomicBool,
}

impl Shared {
    fn publish(&self, m: &Mix) {
        self.pos.store(m.pos.to_bits(), Ordering::Relaxed);
        self.target.store(m.target.or(m.pending_jump).unwrap_or(f64::NAN).to_bits(), Ordering::Relaxed);
        self.playing.store(m.playing, Ordering::Relaxed);
    }
}

pub struct Engine {
    _stream: cpal::Stream,
    pub rate: u32,
    mix: Arc<Mutex<Mix>>,
    shared: Arc<Shared>,
}

impl Engine {
    pub fn new() -> Result<Self> {
        let host = cpal::default_host();
        let device = host.default_output_device().ok_or_else(|| anyhow!("no audio output device"))?;
        let supported = device.default_output_config()?;
        let format = supported.sample_format();
        let config: cpal::StreamConfig = supported.into();
        let rate = config.sample_rate;
        let channels = config.channels as usize;
        let mix = Arc::new(Mutex::new(Mix { song_gain: 0.8, hit_gain: 1.0, master: 1.0, rate: 1.0, sr: rate as f64, fade: 1.0, ..Default::default() }));
        let shared = Arc::new(Shared::default());
        let (m, sh) = (mix.clone(), shared.clone());
        let err = |e| eprintln!("audio stream error: {e}");
        let stream = match format {
            cpal::SampleFormat::F32 => device.build_output_stream(config, move |out: &mut [f32], _: &cpal::OutputCallbackInfo| render(&m, &sh, out, channels, |v| v), err, None)?,
            cpal::SampleFormat::I16 => device.build_output_stream(config, move |out: &mut [i16], _: &cpal::OutputCallbackInfo| render(&m, &sh, out, channels, |v| (v * i16::MAX as f32) as i16), err, None)?,
            cpal::SampleFormat::I32 => device.build_output_stream(config, move |out: &mut [i32], _: &cpal::OutputCallbackInfo| render(&m, &sh, out, channels, |v| (v as f64 * i32::MAX as f64) as i32), err, None)?,
            other => bail!("unsupported output sample format {other}"),
        };
        stream.play()?;
        Ok(Self { _stream: stream, rate, mix, shared })
    }

    fn with<R>(&self, f: impl FnOnce(&mut Mix) -> R) -> R {
        let mut g = self.mix.lock().unwrap_or_else(|e| e.into_inner());
        let r = f(&mut g);
        self.shared.publish(&g);
        r
    }

    pub fn set_song(&self, song: Arc<Vec<i16>>) {
        self.with(|m| {
            m.song = song;
            m.pos = 0.0;
            m.target = None;
            m.playing = false;
            m.vel = 0.0;
            m.reset_cursor();
        });
    }
    pub fn set_samples(&self, samples: Vec<Arc<Vec<f32>>>) {
        self.with(|m| {
            m.voices.clear();
            m.samples = samples;
        });
    }
    pub fn set_events(&self, events: Vec<Event>) {
        let events = Arc::new(events);
        self.with(|m| {
            m.events = events;
            m.cursor = m.events.partition_point(|e| (e.frame as f64) < m.pos);
        });
    }
    pub fn set_gains(&self, song: f32, hits: f32) {
        self.with(|m| {
            m.song_gain = song;
            m.hit_gain = hits;
        });
    }
    pub fn play(&self, on: bool) {
        self.with(|m| {
            if on && !m.playing {
                m.reset_cursor();
            }
            m.playing = on;
        });
    }
    pub fn is_playing(&self) -> bool {
        self.shared.playing.load(Ordering::Relaxed)
    }
    /// Playback speed (1.0 = normal). Hitsounds keep their pitch; the song is resampled.
    pub fn set_speed(&self, speed: f64) {
        self.with(|m| m.rate = speed.clamp(0.1, 4.0));
    }
    /// Jump straight to a time (clicks on the ruler, Home, etc.).
    pub fn seek_ms(&self, ms: f64) {
        let frame = ms.max(0.0) * self.rate as f64 / 1000.0;
        self.with(|m| {
            m.pos = frame;
            m.target = None;
            m.pending_jump = None;
            m.fade = 1.0;
            m.vel = m.base_speed();
            m.reset_cursor();
        });
    }
    /// Seek by ticks: a vinyl-style glide if scrub sound is on, otherwise an
    /// instant jump with a tiny fade so it doesn't click.
    pub fn scrub_to_ms(&self, ms: f64) {
        let frame = ms.max(0.0) * self.rate as f64 / 1000.0;
        self.with(|m| {
            if m.scrub_sound {
                if m.target.is_none() {
                    m.vel = m.base_speed();
                }
                m.target = Some(frame);
            } else if m.playing {
                m.target = None;
                m.pending_jump = Some(frame);
            } else {
                // Nothing audible to fade while paused.
                m.pos = frame;
                m.pending_jump = None;
                m.reset_cursor();
            }
        });
    }
    pub fn set_scrub_sound(&self, on: bool) {
        self.with(|m| m.scrub_sound = on);
    }
    /// Where the head is right now (moves smoothly while scrubbing).
    pub fn position_ms(&self) -> f64 {
        f64::from_bits(self.shared.pos.load(Ordering::Relaxed)) * 1000.0 / self.rate as f64
    }
    /// Where the head is heading: the scrub target if any, else the position.
    /// Stepping by ticks counts from here, so quick notches add up.
    pub fn heading_ms(&self) -> f64 {
        let t = f64::from_bits(self.shared.target.load(Ordering::Relaxed));
        if t.is_nan() { self.position_ms() } else { t * 1000.0 / self.rate as f64 }
    }
    /// Play a sample right away (auditioning), independent of the transport.
    pub fn audition(&self, sample: u32, gain: f32) {
        self.with(|m| {
            if (sample as usize) < m.samples.len() {
                m.voices.push(Voice { sample, pos: 0, gain });
            }
        });
    }
    pub fn stop_all(&self) {
        self.with(|m| {
            m.playing = false;
            m.target = None;
            m.vel = 0.0;
            m.voices.clear();
        });
    }
}

fn song_frame(song: &[i16], pos: f64) -> (f32, f32) {
    if pos < 0.0 {
        return (0.0, 0.0);
    }
    let i0 = pos as usize;
    let i = i0 * 2;
    if i + 3 < song.len() {
        // Linear interpolation for non-1x speeds and scrubbing.
        let f = (pos - i0 as f64) as f32;
        (song[i] as f32 + (song[i + 2] as f32 - song[i] as f32) * f, song[i + 1] as f32 + (song[i + 3] as f32 - song[i + 1] as f32) * f)
    } else if i + 1 < song.len() {
        (song[i] as f32, song[i + 1] as f32)
    } else {
        (0.0, 0.0)
    }
}

fn render<T: Copy>(mix: &Mutex<Mix>, shared: &Shared, out: &mut [T], channels: usize, conv: impl Fn(f32) -> T) {
    let zero = conv(0.0);
    // Wait for the lock instead of dropping the buffer: the UI only holds it for a
    // moment, while a skipped buffer is an audible stutter.
    let mut guard = mix.lock().unwrap_or_else(|e| e.into_inner());
    let m = &mut *guard;
    let tau = SCRUB_TAU_SECS * m.sr;
    let inertia = 1.0 / (SCRUB_INERTIA_SECS * m.sr);
    let fade_step = (1.0 / (JUMP_FADE_SECS * m.sr)) as f32;
    for frame in out.chunks_mut(channels) {
        let (mut l, mut r) = (0f32, 0f32);
        // Instant jump: fade the song out, move, then fade back in.
        if let Some(to) = m.pending_jump {
            m.fade -= fade_step;
            if m.fade <= 0.0 {
                m.fade = 0.0;
                m.pos = to;
                m.pending_jump = None;
                m.cursor = m.events.partition_point(|e| (e.frame as f64) < m.pos);
            }
        } else if m.fade < 1.0 {
            m.fade = (m.fade + fade_step).min(1.0);
        }
        let base = m.base_speed();
        let scrubbing = m.target.is_some();
        if let Some(tg) = m.target.as_mut() {
            // The target keeps running with playback, so the head lands in time with the music.
            *tg += base;
            let dist = *tg - m.pos;
            let want = (dist / tau).clamp(-SCRUB_MAX_SPEED, SCRUB_MAX_SPEED) + base;
            m.vel += (want - m.vel) * inertia;
            if dist.abs() < 1.5 && (m.vel - base).abs() < 0.02 {
                m.pos = *tg;
                m.target = None;
                m.vel = base;
                // Resume hitsounds from here without cutting ones still ringing.
                m.cursor = m.events.partition_point(|e| (e.frame as f64) < m.pos);
            }
        } else {
            m.vel = base;
        }

        if m.vel != 0.0 {
            if m.playing && !scrubbing {
                while m.cursor < m.events.len() && m.events[m.cursor].frame as f64 <= m.pos {
                    let e = m.events[m.cursor];
                    if m.voices.len() < 96 {
                        m.voices.push(Voice { sample: e.sample, pos: 0, gain: e.gain });
                    }
                    m.cursor += 1;
                }
            }
            let g = m.song_gain * m.fade / 32768.0;
            let (sl, sr) = song_frame(&m.song, m.pos);
            l += sl * g;
            r += sr * g;
            m.pos = (m.pos + m.vel).max(0.0);
            if m.playing && !scrubbing && (m.pos as usize) * 2 + 1 >= m.song.len() && m.events.get(m.cursor).is_none() && m.voices.is_empty() {
                m.playing = false;
            }
        }
        let hit = m.hit_gain;
        let samples = &m.samples;
        m.voices.retain_mut(|v| {
            let Some(buf) = samples.get(v.sample as usize) else { return false };
            let i = v.pos * 2;
            if i + 1 >= buf.len() {
                return false;
            }
            l += buf[i] * v.gain * hit;
            r += buf[i + 1] * v.gain * hit;
            v.pos += 1;
            true
        });
        let (l, r) = ((l * m.master).clamp(-1.0, 1.0), (r * m.master).clamp(-1.0, 1.0));
        match frame.len() {
            0 => {}
            1 => frame[0] = conv((l + r) * 0.5),
            _ => {
                frame[0] = conv(l);
                frame[1] = conv(r);
                for s in frame.iter_mut().skip(2) {
                    *s = zero;
                }
            }
        }
    }
    shared.publish(m);
}

/// Halve memory for the long song buffer; 16-bit is plenty for playback.
pub fn to_i16(data: &[f32]) -> Vec<i16> {
    data.iter().map(|v| (v.clamp(-1.0, 1.0) * 32767.0) as i16).collect()
}

/// Write interleaved stereo f32 as 16-bit PCM WAV.
pub fn write_wav(path: &Path, pcm: &Pcm) -> Result<()> {
    let spec = hound::WavSpec { channels: 2, sample_rate: pcm.rate, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
    let mut w = hound::WavWriter::create(path, spec)?;
    for &v in &pcm.data {
        w.write_sample((v.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16)?;
    }
    w.finalize()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resample_keeps_duration() {
        let pcm = Pcm { rate: 48_000, data: vec![0.5; 48_000 * 2] };
        let out = resample(&pcm, 44_100);
        assert_eq!(out.frames(), 44_100);
        assert!((out.data[100] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn wav_roundtrip() {
        let dir = std::env::temp_dir().join(format!("hitsmith-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("t.wav");
        let pcm = Pcm { rate: 44_100, data: (0..2000).map(|i| ((i as f32) * 0.01).sin() * 0.5).collect() };
        write_wav(&p, &pcm).unwrap();
        let back = decode_file(&p).unwrap();
        assert_eq!(back.rate, 44_100);
        assert_eq!(back.frames(), 1000);
        assert!((back.data[10] - pcm.data[10]).abs() < 1e-3);
        std::fs::remove_dir_all(&dir).ok();
    }
}

#[cfg(test)]
mod scrub_tests {
    use super::*;

    #[test]
    fn scrub_rewinds_then_lands_in_time() {
        let sr = 48_000.0;
        let mix = Mutex::new(Mix {
            song: Arc::new(vec![1000i16; 48_000 * 2 * 10]),
            pos: sr * 2.0,
            rate: 1.0,
            playing: true,
            scrub_sound: true,
            fade: 1.0,
            sr,
            song_gain: 1.0,
            hit_gain: 1.0,
            master: 1.0,
            ..Default::default()
        });
        let shared = Shared::default();
        // Ask to go 300 ms back while playing.
        {
            let mut m = mix.lock().unwrap();
            m.vel = 1.0;
            m.target = Some(m.pos - 0.3 * sr);
        }
        let mut buf = vec![0f32; 2 * 480]; // 10 ms blocks
        let mut went_backwards = false;
        for _ in 0..60 {
            let before = mix.lock().unwrap().pos;
            render(&mix, &shared, &mut buf, 2, |v| v);
            if mix.lock().unwrap().pos < before {
                went_backwards = true;
            }
        }
        let m = mix.lock().unwrap();
        assert!(went_backwards, "the head should audibly run backwards");
        assert!(m.target.is_none(), "scrub should settle");
        // 600 ms of playback after starting 300 ms earlier: 2.0 - 0.3 + 0.6 = 2.3 s.
        let expect = (2.0 - 0.3 + 0.6) * sr;
        assert!((m.pos - expect).abs() < 0.01 * sr, "landed at {} s, expected ~2.3 s", m.pos / sr);
        assert!((m.vel - 1.0).abs() < 1e-9);
    }
}

#[cfg(test)]
mod jump_tests {
    use super::*;

    #[test]
    fn default_seek_jumps_with_a_short_fade() {
        let sr = 48_000.0;
        let mix = Mutex::new(Mix { song: Arc::new(vec![1000i16; 48_000 * 2 * 10]), pos: sr * 2.0, rate: 1.0, playing: true, sr, fade: 1.0, song_gain: 1.0, hit_gain: 1.0, master: 1.0, ..Default::default() });
        let shared = Shared::default();
        mix.lock().unwrap().pending_jump = Some(sr * 1.0);
        let mut buf = vec![0f32; 2 * 480];
        render(&mix, &shared, &mut buf, 2, |v| v);
        let m = mix.lock().unwrap();
        // Jumped back to ~1 s within the first 10 ms block, never ran backwards.
        assert!(m.pending_jump.is_none());
        assert!(m.pos > sr * 1.0 && m.pos < sr * 1.01, "pos {}", m.pos / sr);
        // Volume dipped (fade) somewhere in that block and came back up.
        let min = buf.iter().step_by(2).cloned().fold(f32::MAX, f32::min);
        assert!(min < 0.01, "fade should dip near silence");
        assert!(m.fade > 0.99);
    }
}
