//! Export a project as a hitsound difficulty.
//!
//! Every distinct set of sounds that plays at the same moment is mixed into one
//! WAV and saved as a custom `normal-hitnormalN.wav`. Each hit circle then plays
//! just that hitnormal, picked with a green line (sample index + volume). This
//! reproduces any arrangement exactly, whatever the skin or layering settings.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::audio::{self, Pcm};
use crate::osu;
use crate::project::{Project, SampleEntry};

pub const MANIFEST: &str = ".hitsmith-export.json";
const RATE: u32 = 44_100;

/// What this app created in a mapset folder, grouped per exported difficulty
/// (.osu file name), so one export never overwrites or deletes another's files.
#[derive(Default, Serialize, Deserialize)]
pub struct Manifest {
    #[serde(default)]
    pub exports: BTreeMap<String, Vec<String>>,
    /// Older single-list format; migrated into `exports` when read.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
}

impl Manifest {
    /// Every file any export created (hidden from the sample list).
    pub fn all_files(&self) -> Vec<String> {
        self.exports.values().flatten().cloned().collect()
    }
    fn key_for(&self, osu_name: &str) -> Option<String> {
        self.exports.keys().find(|k| k.eq_ignore_ascii_case(osu_name)).cloned()
    }
}

pub fn read_manifest(mapset_dir: &Path) -> Manifest {
    let mut m: Manifest = fs::read(mapset_dir.join(MANIFEST)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    if !m.files.is_empty() {
        let files = std::mem::take(&mut m.files);
        let key = files.iter().find(|f| f.to_ascii_lowercase().ends_with(".osu")).cloned().unwrap_or_default();
        m.exports.entry(key).or_default().extend(files);
    }
    m
}

fn write_manifest(mapset_dir: &Path, m: &Manifest) -> Result<()> {
    let tmp = mapset_dir.join(format!("{MANIFEST}.tmp"));
    fs::write(&tmp, serde_json::to_vec_pretty(m)?)?;
    fs::rename(&tmp, mapset_dir.join(MANIFEST))?;
    Ok(())
}

pub struct Report {
    pub osu_file: String,
    pub circles: usize,
    pub sample_files: usize,
    pub clipped: usize,
    pub first_index: u32,
    pub last_index: u32,
}

fn hitnormal_name(index: u32, ext: &str) -> String {
    if index == 1 { format!("normal-hitnormal.{ext}") } else { format!("normal-hitnormal{index}.{ext}") }
}

/// A timing-point line split into its 8 fields.
struct RawRed {
    offset: f64,
    fields: Vec<String>,
}

fn raw_red_lines(text: &str) -> Vec<RawRed> {
    let mut out = Vec::new();
    let mut in_timing = false;
    for raw in text.lines() {
        let line = raw.trim();
        if line.starts_with('[') {
            in_timing = line == "[TimingPoints]";
            continue;
        }
        if !in_timing || line.is_empty() || line.starts_with("//") {
            continue;
        }
        let mut f: Vec<String> = line.split(',').map(|s| s.trim().to_string()).collect();
        let (Ok(offset), Ok(bl)) = (f[0].parse::<f64>(), f.get(1).map(|s| s.parse::<f64>()).unwrap_or(Ok(-1.0))) else { continue };
        let uninherited = f.get(6).map(|v| v == "1").unwrap_or(bl > 0.0);
        if !uninherited || bl <= 0.0 {
            continue;
        }
        let defaults = ["0", "0", "4", "1", "0", "100", "1", "0"];
        while f.len() < 8 {
            f.push(defaults[f.len()].to_string());
        }
        out.push(RawRed { offset, fields: f });
    }
    out.sort_by(|a, b| a.offset.total_cmp(&b.offset));
    out
}

pub fn export(project: &Project, library: &[SampleEntry]) -> Result<Report> {
    let opts = &project.export;
    let diff_name = opts.diff_name.trim();
    if diff_name.is_empty() {
        bail!("Enter a difficulty name.");
    }
    if osu::sanitize_file_part(diff_name) != diff_name || diff_name.contains(['[', ']']) {
        bail!("The difficulty name can't contain \\ / : * ? \" < > | [ ]");
    }
    let start_index = opts.start_index.max(1);
    let dir = &project.mapset_dir;
    let source = osu::read_beatmap(&dir.join(&project.osu_file))?;
    let reds = raw_red_lines(&source.text);

    // 1. Collect what plays at each moment: time -> sorted list of (sample path, gain in 1/1000).
    let paths: HashMap<_, _> = library.iter().map(|e| (e.r.clone(), e.path.clone())).collect();
    let mut moments: BTreeMap<i64, Vec<(PathBuf, u32)>> = BTreeMap::new();
    for track in &project.content.tracks {
        if track.notes.is_empty() || track.volume == 0 {
            continue;
        }
        let Some(r) = &track.sample else {
            bail!("Track \"{}\" has notes but no sample selected.", track.name);
        };
        let Some(path) = paths.get(r) else {
            bail!("Sample \"{}\" used by track \"{}\" is missing.", r.file, track.name);
        };
        for &t in &track.notes {
            moments.entry(t).or_default().push((path.clone(), track.volume as u32 * 10));
        }
    }
    if moments.is_empty() {
        bail!("There are no notes to export.");
    }

    // 2. Distinct combinations become one generated sample each.
    let mut combos: Vec<Vec<(PathBuf, u32)>> = Vec::new();
    let mut combo_of: HashMap<Vec<(PathBuf, u32)>, usize> = HashMap::new();
    let mut circles: Vec<(i64, usize)> = Vec::new();
    for (t, mut parts) in moments {
        parts.sort();
        let id = *combo_of.entry(parts.clone()).or_insert_with(|| {
            combos.push(parts);
            combos.len() - 1
        });
        circles.push((t, id));
    }

    // 3. The difficulty's file name, and the files a previous export of it created.
    let m = &source.meta;
    let osu_name = format!("{} - {} ({}) [{}].osu", osu::sanitize_file_part(&m.artist), osu::sanitize_file_part(&m.title), osu::sanitize_file_part(&m.creator), diff_name);
    let mut manifest = read_manifest(dir);
    let key = manifest.key_for(&osu_name).unwrap_or_else(|| osu_name.clone());
    let previous: Vec<String> = manifest.exports.get(&key).cloned().unwrap_or_default();
    // Only files from this difficulty's own last export may be replaced; everything else
    // (the mapset's samples, other difficulties' exports) is left alone.
    let owned = |name: &str| previous.iter().any(|f| f.eq_ignore_ascii_case(name));
    let occupied = |index: u32| {
        ["wav", "ogg", "mp3"].iter().any(|ext| {
            let name = hitnormal_name(index, ext);
            dir.join(&name).exists() && !owned(&name)
        })
    };
    let mut indices = Vec::with_capacity(combos.len());
    let mut next = start_index;
    for _ in 0..combos.len() {
        while occupied(next) {
            next += 1;
        }
        indices.push(next);
        next += 1;
    }

    // 4. Target .osu must be new or ours.
    if osu_name.eq_ignore_ascii_case(&project.osu_file) {
        bail!("That name would overwrite the source difficulty. Pick another name.");
    }
    if dir.join(&osu_name).exists() && !owned(&osu_name) {
        bail!("\"{osu_name}\" already exists and wasn't made by this app. Pick another difficulty name.");
    }

    // 5. Render samples.
    let mut decoded: HashMap<PathBuf, Pcm> = HashMap::new();
    for combo in &combos {
        for (p, _) in combo {
            if !decoded.contains_key(p) {
                let pcm = audio::decode_file(p).with_context(|| format!("decoding {}", p.display()))?;
                decoded.insert(p.clone(), audio::resample(&pcm, RATE));
            }
        }
    }
    // Record what we are about to create before writing it, so a failed export
    // still leaves files that the next export recognises as ours and cleans up.
    let mut planned: Vec<String> = indices.iter().map(|&i| hitnormal_name(i, "wav")).collect();
    planned.push(osu_name.clone());
    {
        let entry = manifest.exports.entry(key.clone()).or_default();
        for f in &planned {
            if !entry.iter().any(|e| e.eq_ignore_ascii_case(f)) {
                entry.push(f.clone());
            }
        }
    }
    write_manifest(dir, &manifest)?;

    let mut written: Vec<String> = Vec::new();
    let mut clipped = 0;
    for (combo, &index) in combos.iter().zip(&indices) {
        let len = combo.iter().map(|(p, _)| decoded[p].data.len()).max().unwrap_or(0).max(2);
        let mut mix = vec![0f32; len];
        for (p, g) in combo {
            let gain = *g as f32 / 1000.0;
            for (o, s) in mix.iter_mut().zip(&decoded[p].data) {
                *o += s * gain;
            }
        }
        if mix.iter().any(|v| v.abs() > 1.0) {
            clipped += 1;
        }
        let name = hitnormal_name(index, "wav");
        audio::write_wav(&dir.join(&name), &Pcm { rate: RATE, data: mix })?;
        written.push(name);
    }

    // 6. Timing: red lines copied from the source, green lines for index/volume changes.
    let state_at = |t: i64, combo: usize| (indices[combo], project.volume_at(t).clamp(5, 100));
    let mut timing: Vec<String> = Vec::new();
    let mut objects: Vec<String> = Vec::new();
    let mut current: Option<(u32, u8)> = None;
    let mut ri = 0;
    let red_for = |t: f64| reds.iter().rev().find(|r| r.offset <= t + 0.5).or(reds.first());
    for (n, &(t, combo)) in circles.iter().enumerate() {
        while ri < reds.len() && reds[ri].offset <= t as f64 + 0.5 {
            // A red line resets the sample settings, so give it the next circle's state.
            let (idx, vol) = state_at(t, combo);
            let mut f = reds[ri].fields.clone();
            f[3] = "1".into();
            f[4] = idx.to_string();
            f[5] = vol.to_string();
            timing.push(f.join(","));
            current = Some((idx, vol));
            ri += 1;
        }
        let want = state_at(t, combo);
        if current != Some(want) {
            let (meter, effects) = red_for(t as f64).map(|r| (r.fields[2].clone(), r.fields[7].clone())).unwrap_or(("4".into(), "0".into()));
            timing.push(format!("{t},-100,{meter},1,{},{},0,{effects}", want.0, want.1));
            current = Some(want);
        }
        let kind = if n == 0 { 5 } else { 1 };
        objects.push(format!("256,192,{t},{kind},0,0:0:0:0:"));
    }
    for r in &reds[ri..] {
        let mut f = r.fields.clone();
        if let Some((idx, vol)) = current {
            f[3] = "1".into();
            f[4] = idx.to_string();
            f[5] = vol.to_string();
        }
        timing.push(f.join(","));
    }

    // 7. Write the difficulty from the source as a template.
    let mut text = osu::set_value(&source.text, "Metadata", "Version", diff_name);
    text = osu::set_value(&text, "Metadata", "BeatmapID", "0");
    // The project's bookmarks replace the source's (an empty list clears them).
    if project.bookmarks.is_empty() {
        text = osu::remove_value(&text, "Editor", "Bookmarks");
    } else {
        let marks: Vec<String> = project.bookmarks.iter().map(|b| b.to_string()).collect();
        text = osu::ensure_section(&text, "Editor", "Metadata");
        text = osu::set_value(&text, "Editor", "Bookmarks", &marks.join(","));
    }
    text = osu::replace_section(&text, "TimingPoints", &timing.join("\r\n"));
    text = osu::replace_section(&text, "HitObjects", &objects.join("\r\n"));
    let osu_tmp = dir.join(format!("{osu_name}.tmp"));
    let wrote = fs::write(&osu_tmp, text.as_bytes()).and_then(|_| fs::rename(&osu_tmp, dir.join(&osu_name)));
    if let Err(e) = wrote {
        let _ = fs::remove_file(&osu_tmp);
        return Err(anyhow::Error::new(e).context(format!("writing {osu_name} (is it open in osu!?)")));
    }
    written.push(osu_name.clone());

    // 8. Remove this difficulty's leftovers from its previous export (never files another
    //    export still uses), then record exactly what this export owns.
    let others: Vec<String> = manifest.exports.iter().filter(|(k, _)| **k != key).flat_map(|(_, v)| v.clone()).collect();
    for old in &previous {
        let still_mine = written.iter().any(|w| w.eq_ignore_ascii_case(old));
        let used_elsewhere = others.iter().any(|o| o.eq_ignore_ascii_case(old));
        if !still_mine && !used_elsewhere {
            let p = dir.join(old);
            if p.parent() == Some(dir.as_path()) {
                let _ = fs::remove_file(p);
            }
        }
    }
    manifest.exports.insert(key, written);
    write_manifest(dir, &manifest)?;

    Ok(Report {
        osu_file: osu_name,
        circles: circles.len(),
        sample_files: combos.len(),
        clipped,
        first_index: *indices.first().unwrap_or(&start_index),
        last_index: *indices.last().unwrap_or(&start_index),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::{ControlPoint, SampleRef, SampleSource, Track};

    #[test]
    fn exports_and_reexports_safely() {
        let dir = std::env::temp_dir().join(format!("hitsmith-export-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let src = "osu file format v14\r\n\r\n[General]\r\nAudioFilename: audio.mp3\r\nMode: 0\r\n\r\n[Metadata]\r\nTitle:Song\r\nArtist:Art\r\nCreator:me\r\nVersion:Hard\r\nBeatmapID:55\r\n\r\n[Difficulty]\r\nHPDrainRate:5\r\n\r\n[Events]\r\n\r\n[TimingPoints]\r\n1000,500,4,2,0,60,1,0\r\n1500,-50,4,2,0,60,0,1\r\n3000,400,3,2,0,60,1,0\r\n\r\n[HitObjects]\r\n256,192,1000,1,0,0:0:0:0:\r\n";
        fs::write(dir.join("Art - Song (me) [Hard].osu"), src).unwrap();
        // The mapset already ships index 1, which we must not overwrite.
        audio::write_wav(&dir.join("normal-hitnormal.wav"), &Pcm { rate: 44_100, data: vec![0.1; 200] }).unwrap();
        let clap = dir.join("myclap.wav");
        let kick = dir.join("mykick.wav");
        audio::write_wav(&clap, &Pcm { rate: 44_100, data: vec![0.2; 400] }).unwrap();
        audio::write_wav(&kick, &Pcm { rate: 48_000, data: vec![0.3; 600] }).unwrap();
        let lib = vec![
            SampleEntry { r: SampleRef { source: SampleSource::Mapset, file: "myclap.wav".into() }, path: clap },
            SampleEntry { r: SampleRef { source: SampleSource::Mapset, file: "mykick.wav".into() }, path: kick },
        ];
        let mut p = Project::new(dir.clone(), "Art - Song (me) [Hard].osu".into());
        p.content.control_points = vec![ControlPoint { time: 0, volume: 60 }, ControlPoint { time: 2000, volume: 30 }];
        p.content.tracks = vec![
            Track { id: 1, name: "clap".into(), sample: Some(lib[0].r.clone()), volume: 100, muted: false, solo: false, color: 0, notes: vec![1000, 1500, 2500], key: None },
            Track { id: 2, name: "kick".into(), sample: Some(lib[1].r.clone()), volume: 50, muted: false, solo: false, color: 1, notes: vec![1000, 3200], key: None },
        ];
        let r = export(&p, &lib).unwrap();
        assert_eq!(r.circles, 4);
        assert_eq!(r.sample_files, 3); // clap+kick, clap, kick
        assert_eq!(r.first_index, 2);
        let out = fs::read_to_string(dir.join(&r.osu_file)).unwrap();
        assert!(out.contains("Version:Hitsounds"));
        assert!(out.contains("BeatmapID:0"));
        assert!(out.contains("1000,500,4,1,2,60,1,0"), "{out}");
        assert!(out.contains("1500,-100,4,1,3,60,0,0"), "{out}");
        assert!(out.contains("2500,-100,4,1,3,30,0,0"), "{out}");
        assert!(out.contains("3000,400,3,1,3,30,1,0") || out.contains("3000,400,3,1,4,30,1,0"), "{out}");
        assert!(out.contains("256,192,1000,5,0,0:0:0:0:"));
        assert!(fs::metadata(dir.join("normal-hitnormal.wav")).unwrap().len() < 1000, "mapset sample untouched");
        assert!(dir.join("normal-hitnormal4.wav").exists());

        // Re-export with fewer combos: stale generated file is removed, source untouched.
        p.content.tracks.truncate(1);
        let r2 = export(&p, &lib).unwrap();
        assert_eq!(r2.sample_files, 1);
        assert!(!dir.join("normal-hitnormal4.wav").exists());
        assert!(dir.join("normal-hitnormal.wav").exists());
        assert!(fs::read_to_string(dir.join("Art - Song (me) [Hard].osu")).unwrap().contains("Version:Hard"));

        // Refuses to overwrite a difficulty it didn't make.
        p.export.diff_name = "Hard".into();
        assert!(export(&p, &lib).is_err());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn separate_difficulties_keep_their_own_files() {
        let dir = std::env::temp_dir().join(format!("hitsmith-export2-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let src = "osu file format v14\r\n\r\n[General]\r\nAudioFilename: a.mp3\r\n\r\n[Metadata]\r\nTitle:S\r\nArtist:A\r\nCreator:c\r\nVersion:Hard\r\n\r\n[TimingPoints]\r\n0,500,4,2,0,60,1,0\r\n\r\n[HitObjects]\r\n";
        fs::write(dir.join("A - S (c) [Hard].osu"), src).unwrap();
        let clap = dir.join("clap.wav");
        audio::write_wav(&clap, &Pcm { rate: 44_100, data: vec![0.2; 400] }).unwrap();
        let lib = vec![SampleEntry { r: SampleRef { source: SampleSource::Mapset, file: "clap.wav".into() }, path: clap }];
        let mut p = Project::new(dir.clone(), "A - S (c) [Hard].osu".into());
        p.content.tracks = vec![Track { id: 1, name: "c".into(), sample: Some(lib[0].r.clone()), volume: 100, muted: false, solo: false, color: 0, notes: vec![0, 500], key: None }];
        p.export.diff_name = "One".into();
        let a = export(&p, &lib).unwrap();
        p.export.diff_name = "Two".into();
        let b = export(&p, &lib).unwrap();
        assert!(dir.join(&a.osu_file).exists(), "first difficulty must survive the second export");
        assert!(dir.join(&b.osu_file).exists());
        assert_ne!(a.first_index, b.first_index, "the second export must not reuse the first one's sample");
        // Re-exporting "One" keeps its own index and doesn't delete "Two".
        p.export.diff_name = "One".into();
        let a2 = export(&p, &lib).unwrap();
        assert_eq!(a2.first_index, a.first_index);
        assert!(dir.join(&b.osu_file).exists());
        // Bookmarks: none in the project means none in the export.
        assert!(!fs::read_to_string(dir.join(&a2.osu_file)).unwrap().contains("Bookmarks"));
        fs::remove_dir_all(&dir).ok();
    }
}
