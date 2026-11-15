//! Minimal reader/writer for the parts of the osu! (.osu) file format we need.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct Meta {
    pub title: String,
    pub title_unicode: String,
    pub artist: String,
    pub artist_unicode: String,
    pub creator: String,
    pub version: String,
    pub audio_filename: String,
    pub background: String,
}

impl Meta {
    pub fn display_title(&self) -> &str {
        if self.title_unicode.is_empty() { &self.title } else { &self.title_unicode }
    }
    pub fn display_artist(&self) -> &str {
        if self.artist_unicode.is_empty() { &self.artist } else { &self.artist_unicode }
    }
}

/// An uninherited (red) timing point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RedLine {
    pub offset: f64,
    pub beat_length: f64,
    pub meter: u32,
}

impl RedLine {
    pub fn bpm(&self) -> f64 {
        60_000.0 / self.beat_length
    }
}

pub struct Beatmap {
    pub meta: Meta,
    pub red_lines: Vec<RedLine>,
    /// Editor bookmarks (ms) from the [Editor] section.
    pub bookmarks: Vec<i64>,
    pub text: String,
}

fn parse_bookmarks(text: &str) -> Vec<i64> {
    let mut in_editor = false;
    for raw in text.lines() {
        let line = raw.trim();
        if line.starts_with('[') {
            in_editor = line == "[Editor]";
            continue;
        }
        if in_editor {
            if let Some(("Bookmarks", v)) = split_kv(line) {
                let mut b: Vec<i64> = v.split(',').filter_map(|x| x.trim().parse().ok()).collect();
                b.sort_unstable();
                b.dedup();
                return b;
            }
        }
    }
    Vec::new()
}

fn read_text(path: &Path) -> Result<String> {
    let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let text = String::from_utf8_lossy(&bytes);
    Ok(text.trim_start_matches('\u{feff}').to_string())
}

fn split_kv(line: &str) -> Option<(&str, &str)> {
    let (k, v) = line.split_once(':')?;
    Some((k.trim(), v.trim()))
}

fn parse_background(line: &str) -> Option<String> {
    // 0,0,"bg.jpg",0,0
    let mut parts = line.splitn(4, ',');
    let kind = parts.next()?.trim();
    if kind != "0" && !kind.eq_ignore_ascii_case("Background") {
        return None;
    }
    parts.next()?;
    let name = parts.next()?.trim().trim_matches('"');
    if name.is_empty() { None } else { Some(name.to_string()) }
}

fn parse_meta(text: &str) -> Meta {
    let mut meta = Meta::default();
    let mut section = "";
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with("//") {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = &line[1..line.len() - 1];
            if section == "TimingPoints" || section == "HitObjects" {
                break;
            }
            continue;
        }
        match section {
            "General" => {
                if let Some(("AudioFilename", v)) = split_kv(line) {
                    meta.audio_filename = v.to_string();
                }
            }
            "Metadata" => {
                if let Some((k, v)) = split_kv(line) {
                    let v = v.to_string();
                    match k {
                        "Title" => meta.title = v,
                        "TitleUnicode" => meta.title_unicode = v,
                        "Artist" => meta.artist = v,
                        "ArtistUnicode" => meta.artist_unicode = v,
                        "Creator" => meta.creator = v,
                        "Version" => meta.version = v,
                        _ => {}
                    }
                }
            }
            "Events" => {
                if meta.background.is_empty() {
                    if let Some(bg) = parse_background(line) {
                        meta.background = bg;
                    }
                }
            }
            _ => {}
        }
    }
    meta
}

pub fn read_beatmap(path: &Path) -> Result<Beatmap> {
    let text = read_text(path)?;
    let meta = parse_meta(&text);
    let mut red_lines = Vec::new();
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
        let f: Vec<&str> = line.split(',').map(str::trim).collect();
        if f.len() < 2 {
            continue;
        }
        let (Ok(offset), Ok(beat_length)) = (f[0].parse::<f64>(), f[1].parse::<f64>()) else { continue };
        // Field 7 is "uninherited"; old files without it treat positive beat length as red.
        let uninherited = f.get(6).map(|v| *v == "1").unwrap_or(beat_length > 0.0);
        if uninherited && beat_length > 0.0 && beat_length.is_finite() && offset.is_finite() {
            let meter = f.get(2).and_then(|m| m.parse().ok()).filter(|m| *m > 0).unwrap_or(4);
            red_lines.push(RedLine { offset, beat_length, meter });
        }
    }
    red_lines.sort_by(|a, b| a.offset.total_cmp(&b.offset));
    if red_lines.is_empty() {
        bail!("{} has no BPM timing points", path.display());
    }
    let bookmarks = parse_bookmarks(&text);
    Ok(Beatmap { meta, red_lines, bookmarks, text })
}

/// Red line in effect at `ms` (the first one also covers time before it).
pub fn red_line_at(lines: &[RedLine], ms: f64) -> &RedLine {
    let idx = lines.partition_point(|l| l.offset <= ms + 0.5);
    &lines[idx.saturating_sub(1)]
}

/// Snap a time to the nearest 1/`div` beat of the timing section it falls in.
pub fn snap(lines: &[RedLine], ms: f64, div: u32) -> f64 {
    let rl = red_line_at(lines, ms);
    let step = rl.beat_length / div as f64;
    let n = ((ms - rl.offset) / step).round();
    let snapped = rl.offset + n * step;
    // Never snap into the previous section's territory across a red line.
    let idx = lines.partition_point(|l| l.offset <= ms + 0.5);
    if let Some(next) = lines.get(idx) {
        if snapped >= next.offset - 0.5 {
            return next.offset;
        }
    }
    snapped
}

/// Grid lines (time, subdivision index within the beat) between `start` and `end`.
pub fn grid_lines(lines: &[RedLine], start: f64, end: f64, div: u32, mut f: impl FnMut(f64, u32, bool)) {
    for (i, rl) in lines.iter().enumerate() {
        let sec_end = lines.get(i + 1).map(|n| n.offset).unwrap_or(f64::INFINITY);
        let sec_start = if i == 0 { f64::NEG_INFINITY } else { rl.offset };
        let lo = start.max(sec_start);
        let hi = end.min(sec_end);
        if lo >= hi {
            continue;
        }
        let step = rl.beat_length / div as f64;
        let mut n = ((lo - rl.offset) / step).ceil() as i64;
        loop {
            let t = rl.offset + n as f64 * step;
            if t >= hi {
                break;
            }
            let sub = n.rem_euclid(div as i64) as u32;
            let beat = n.div_euclid(div as i64);
            let measure_start = sub == 0 && beat.rem_euclid(rl.meter as i64) == 0;
            f(t, sub, measure_start);
            n += 1;
        }
    }
}

/// Replace (or add) `key: value` inside a `[section]` of an .osu text.
pub fn set_value(text: &str, section: &str, key: &str, value: &str) -> String {
    let mut out = String::with_capacity(text.len() + 64);
    let mut in_section = false;
    let mut done = false;
    let header = format!("[{section}]");
    for line in text.split_inclusive('\n') {
        let t = line.trim();
        if t.starts_with('[') {
            if in_section && !done {
                out.push_str(&format!("{key}:{value}\r\n"));
                done = true;
            }
            in_section = t == header;
        } else if in_section && !done {
            if let Some((k, _)) = split_kv(t) {
                if k == key {
                    out.push_str(&format!("{key}:{value}\r\n"));
                    done = true;
                    continue;
                }
            }
        }
        out.push_str(line);
    }
    if in_section && !done {
        if !out.ends_with('\n') {
            out.push_str("\r\n");
        }
        out.push_str(&format!("{key}:{value}\r\n"));
    }
    out
}

/// Replace the body of a section with `body` (keeps the header line).
pub fn replace_section(text: &str, section: &str, body: &str) -> String {
    let header = format!("[{section}]");
    let mut out = String::with_capacity(text.len());
    let mut skipping = false;
    let mut found = false;
    for line in text.split_inclusive('\n') {
        let t = line.trim();
        if t.starts_with('[') && t.ends_with(']') {
            if skipping {
                // Keep the usual blank line between sections.
                out.push_str("\r\n");
                skipping = false;
            }
            if t == header {
                out.push_str(line);
                if !line.ends_with('\n') {
                    out.push_str("\r\n");
                }
                out.push_str(body);
                out.push_str("\r\n");
                skipping = true;
                found = true;
                continue;
            }
        }
        if !skipping {
            out.push_str(line);
        }
    }
    if !found {
        out.push_str(&format!("\r\n{header}\r\n{body}\r\n"));
    }
    out
}

/// Remove characters Windows forbids in file names.
pub fn sanitize_file_part(s: &str) -> String {
    s.chars().filter(|c| !matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') && !c.is_control()).collect::<String>().trim().trim_end_matches('.').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "osu file format v14\r\n\r\n[General]\r\nAudioFilename: audio.mp3\r\n\r\n[Metadata]\r\nTitle:Song\r\nTitleUnicode:曲\r\nArtist:Art\r\nCreator:me\r\nVersion:Hard\r\nBeatmapID:123\r\n\r\n[Difficulty]\r\nHPDrainRate:5\r\n\r\n[Events]\r\n//Background and Video events\r\n0,0,\"bg.jpg\",0,0\r\n\r\n[TimingPoints]\r\n1000,500,4,2,0,60,1,0\r\n2000,-50,4,2,0,60,0,0\r\n3000,400,3,2,0,60,1,0\r\n\r\n[HitObjects]\r\n256,192,1000,1,0,0:0:0:0:\r\n";

    #[test]
    fn parses_meta_and_timing() {
        let m = parse_meta(SAMPLE);
        assert_eq!(m.audio_filename, "audio.mp3");
        assert_eq!(m.display_title(), "曲");
        assert_eq!(m.background, "bg.jpg");
        assert_eq!(m.version, "Hard");
    }

    #[test]
    fn snapping_respects_sections() {
        let lines = vec![
            RedLine { offset: 1000.0, beat_length: 500.0, meter: 4 },
            RedLine { offset: 3000.0, beat_length: 400.0, meter: 3 },
        ];
        assert_eq!(snap(&lines, 1130.0, 4), 1125.0);
        assert_eq!(snap(&lines, 2990.0, 1), 3000.0);
        assert_eq!(snap(&lines, 3190.0, 2), 3200.0);
        assert_eq!(snap(&lines, 900.0, 2), 1000.0);
    }

    #[test]
    fn edits_sections() {
        let t = set_value(SAMPLE, "Metadata", "Version", "Hitsounds");
        assert!(t.contains("Version:Hitsounds\r\n") && !t.contains("Version:Hard"));
        let t = set_value(&t, "Difficulty", "OverallDifficulty", "5");
        assert!(t.contains("HPDrainRate:5\r\n\r\nOverallDifficulty:5\r\n") || t.contains("OverallDifficulty:5\r\n"));
        let t = replace_section(&t, "HitObjects", "256,192,5,1,0,0:0:0:0:");
        assert!(t.ends_with("[HitObjects]\r\n256,192,5,1,0,0:0:0:0:\r\n"));
        assert!(!t.contains("256,192,1000"));
    }
}

/// 1-based bar (measure) number at `t`, counting across timing sections.
pub fn measure_number(lines: &[RedLine], t: f64) -> i64 {
    let mut count = 0i64;
    for (i, rl) in lines.iter().enumerate() {
        let len = rl.beat_length * rl.meter as f64;
        let end = lines.get(i + 1).map(|n| n.offset).unwrap_or(f64::INFINITY);
        if t < end || i + 1 == lines.len() {
            return count + ((t - rl.offset) / len + 1e-6).floor() as i64 + 1;
        }
        // A section shorter than a whole bar still starts a new bar at the next red line.
        count += ((end - rl.offset) / len - 1e-6).ceil() as i64;
    }
    count
}

#[cfg(test)]
mod measure_tests {
    use super::*;

    #[test]
    fn counts_bars_across_sections() {
        let lines = vec![RedLine { offset: 0.0, beat_length: 500.0, meter: 4 }, RedLine { offset: 3000.0, beat_length: 400.0, meter: 3 }];
        assert_eq!(measure_number(&lines, 0.0), 1);
        assert_eq!(measure_number(&lines, 1999.0), 1);
        assert_eq!(measure_number(&lines, 2000.0), 2);
        // 3000 ms = 1.5 bars of 4/4, so the new section starts bar 3.
        assert_eq!(measure_number(&lines, 3000.0), 3);
        assert_eq!(measure_number(&lines, 4200.0), 4);
    }
}

/// The grid tick strictly after (`forward`) or strictly before `t` at 1/`div`.
pub fn adjacent_tick(lines: &[RedLine], t: f64, div: u32, forward: bool) -> f64 {
    let s = snap(lines, t, div);
    let step = red_line_at(lines, s).beat_length / div as f64;
    if forward {
        if s > t + 0.5 { s } else { snap(lines, s + step, div) }
    } else if s < t - 0.5 {
        s
    } else {
        // Step back from a tick: use the previous section's spacing if we cross a red line.
        let prev = s - red_line_at(lines, s - 1.0).beat_length / div as f64;
        snap(lines, prev, div).min(s - 0.5)
    }
}

#[cfg(test)]
mod tick_tests {
    use super::*;

    #[test]
    fn steps_strictly_past_the_current_position() {
        let lines = vec![RedLine { offset: 1000.0, beat_length: 400.0, meter: 4 }];
        // On a tick: move a full tick.
        assert_eq!(adjacent_tick(&lines, 1400.0, 4, false), 1300.0);
        assert_eq!(adjacent_tick(&lines, 1400.0, 4, true), 1500.0);
        // Just after a tick (what happens while playing): backward goes to that tick...
        assert_eq!(adjacent_tick(&lines, 1420.0, 4, false), 1400.0);
        // ...and from the tick itself, to the one before. Never stuck on the same tick.
        let a = adjacent_tick(&lines, 1420.0, 4, false);
        assert!(adjacent_tick(&lines, a, 4, false) < a);
        // Just before a tick: forward goes to it.
        assert_eq!(adjacent_tick(&lines, 1390.0, 4, true), 1400.0);
    }
}
