//! Read the hitsounds of an existing osu! difficulty into hitsmith tracks.
//!
//! Every hit plays the base hitnormal plus its additions (whistle / finish / clap).
//! The sample set and custom index come from the object itself, or else from the
//! green line in effect, exactly like osu!. Each distinct sound becomes one track
//! (the mapset's numbered file, or the skin's default when the mapset lacks it),
//! and the green lines' volumes become volume points.

use std::collections::BTreeMap;

use crate::project::{ControlPoint, SampleEntry, SampleRef, SampleSource, Track};

#[derive(Clone, Copy)]
struct TimingPoint {
    time: f64,
    set: u8,
    index: u32,
    volume: u8,
}

const SETS: [&str; 4] = ["", "normal", "soft", "drum"];

fn set_from_name(s: &str) -> u8 {
    match s.trim().to_ascii_lowercase().as_str() {
        "normal" => 1,
        "soft" => 2,
        "drum" => 3,
        _ => 1,
    }
}

pub struct Imported {
    pub tracks: Vec<Track>,
    pub control_points: Vec<ControlPoint>,
    pub hits: usize,
    /// Sounds whose sample couldn't be found anywhere (tracks left without a sample).
    pub missing: Vec<String>,
}

/// Parse `text` (a .osu file) and build tracks. `library` is the project's sample list
/// (mapset files and the skin's hitsounds); `first_id` is the id for the first new track.
pub fn import(text: &str, library: &[SampleEntry], first_id: u64) -> Imported {
    let mut section = "";
    let mut default_set = 1u8;
    let mut points: Vec<TimingPoint> = Vec::new();
    let mut objects: Vec<Vec<String>> = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with("//") {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = &line[1..line.len() - 1];
            continue;
        }
        match section {
            "General" => {
                if let Some((k, v)) = line.split_once(':') {
                    if k.trim() == "SampleSet" {
                        default_set = set_from_name(v);
                    }
                }
            }
            "TimingPoints" => {
                let f: Vec<&str> = line.split(',').map(str::trim).collect();
                if f.len() >= 6 {
                    if let Ok(time) = f[0].parse::<f64>() {
                        points.push(TimingPoint {
                            time,
                            set: f[3].parse().unwrap_or(0),
                            index: f[4].parse().unwrap_or(0),
                            volume: f[5].parse::<f64>().map(|v| v.clamp(0.0, 100.0) as u8).unwrap_or(100),
                        });
                    }
                }
            }
            "HitObjects" => objects.push(line.split(',').map(|s| s.trim().to_string()).collect()),
            _ => {}
        }
    }
    points.sort_by(|a, b| a.time.total_cmp(&b.time));
    let point_at = |t: f64| -> TimingPoint {
        // osu! looks up sample settings 5 ms after the hit, so lines placed a hair late still apply.
        let i = points.partition_point(|p| p.time <= t + 5.0);
        points.get(i.saturating_sub(1)).copied().unwrap_or(TimingPoint { time: 0.0, set: 0, index: 0, volume: 100 })
    };

    // Sound (set, kind, index) -> times.
    let mut sounds: BTreeMap<(u8, &'static str, u32), Vec<i64>> = BTreeMap::new();
    let mut keysounds: BTreeMap<String, Vec<i64>> = BTreeMap::new();
    let mut hits = 0;
    for o in &objects {
        if o.len() < 5 {
            continue;
        }
        let (Ok(time), Ok(kind), Ok(mut bits)) = (o[2].parse::<f64>(), o[3].parse::<u32>(), o[4].parse::<u32>()) else { continue };
        let mut t = time;
        // hitSample is the last field for circles/spinners; for sliders the head uses
        // edgeSounds/edgeSets (fields 8/9) when present.
        let mut sample_field = o.last().cloned().unwrap_or_default();
        if kind & 128 != 0 {
            // osu!mania hold note: "endTime:normalSet:addSet:index:volume:filename".
            sample_field = o.get(5).and_then(|f| f.split_once(':')).map(|(_, rest)| rest.to_string()).unwrap_or_default();
        } else if kind & 2 != 0 {
            if let Some(edges) = o.get(8) {
                bits = edges.split('|').next().and_then(|s| s.parse().ok()).unwrap_or(bits);
            }
            if let Some(sets) = o.get(9) {
                let head = sets.split('|').next().unwrap_or("0:0");
                let tail: Vec<&str> = sample_field.split(':').skip(2).collect();
                sample_field = format!("{head}:{}", tail.join(":"));
            }
        } else if kind & 8 != 0 {
            // Spinners sound at their end.
            if let Some(end) = o.get(5).and_then(|s| s.parse::<f64>().ok()) {
                t = end;
            }
        }
        let s: Vec<&str> = sample_field.split(':').collect();
        let num = |i: usize| s.get(i).and_then(|v| v.parse::<u32>().ok()).unwrap_or(0);
        let tp = point_at(t);
        let normal_set = match num(0) as u8 {
            0 => {
                if tp.set == 0 {
                    default_set
                } else {
                    tp.set
                }
            }
            n => n,
        }
        .clamp(1, 3);
        let add_set = match num(1) as u8 {
            0 => normal_set,
            n => n.clamp(1, 3),
        };
        let index = match num(2) {
            0 => tp.index,
            n => n,
        };
        let ms = t.round() as i64;
        hits += 1;
        // A file named on the hit (osu!mania keysounds) plays instead of the hitnormal.
        let keysound = s.get(4).map(|f| f.trim()).filter(|f| !f.is_empty()).map(str::to_string);
        if let Some(file) = keysound {
            keysounds.entry(file).or_default().push(ms);
        } else {
            sounds.entry((normal_set, "normal", index)).or_default().push(ms);
        }
        for (bit, name) in [(2, "whistle"), (4, "finish"), (8, "clap")] {
            if bits & bit != 0 {
                sounds.entry((add_set, name, index)).or_default().push(ms);
            }
        }
    }

    // Resolve each sound to a sample, the way osu! does: the mapset's numbered file,
    // else the skin's default for that set and kind.
    let find_mapset = |stem: &str| {
        library.iter().find(|e| {
            e.r.source != SampleSource::Skin
                && e.r.file.rsplit_once('.').is_some_and(|(s, x)| s.eq_ignore_ascii_case(stem) && ["wav", "ogg", "mp3"].iter().any(|ok| x.eq_ignore_ascii_case(ok)))
        })
    };
    let find_skin = |stem: &str| library.iter().find(|e| e.r.source == SampleSource::Skin && e.r.file.rsplit_once('.').is_some_and(|(s, _)| s.eq_ignore_ascii_case(stem)));
    let mut by_sample: BTreeMap<String, (Option<SampleRef>, Vec<i64>)> = BTreeMap::new();
    let mut missing = Vec::new();
    for ((set, kind, index), times) in sounds {
        let base = format!("{}-hit{kind}", SETS[set as usize]);
        let custom = if index >= 2 { format!("{base}{index}") } else { base.clone() };
        // Index 0 means "use the skin"; 1 means the mapset's un-numbered file.
        let found = if index == 0 { None } else { find_mapset(&custom) };
        let (label, sample) = match found {
            Some(e) => (custom.clone(), Some(e.r.clone())),
            None => match find_skin(&base) {
                Some(e) => (format!("{base} (skin)"), Some(e.r.clone())),
                None => {
                    missing.push(base.clone());
                    (format!("{base} (skin, missing)"), None)
                }
            },
        };
        let entry = by_sample.entry(label).or_insert((sample, Vec::new()));
        entry.1.extend(times);
    }

    for (file, times) in keysounds {
        let found = library.iter().find(|e| e.r.source != SampleSource::Skin && e.r.file.eq_ignore_ascii_case(&file));
        if found.is_none() {
            missing.push(file.clone());
        }
        let stem = file.rsplit_once('.').map(|(s, _)| s.to_string()).unwrap_or_else(|| file.clone());
        let label = if found.is_some() { stem } else { format!("{stem} (missing)") };
        let entry = by_sample.entry(label).or_insert((found.map(|e| e.r.clone()), Vec::new()));
        entry.1.extend(times);
    }

    // Order tracks like a hitsounder would read them: normals, then whistles, finishes, claps.
    let rank = |label: &str| ["hitnormal", "hitwhistle", "hitfinish", "hitclap"].iter().position(|k| label.contains(k)).unwrap_or(4);
    let mut entries: Vec<(String, (Option<SampleRef>, Vec<i64>))> = by_sample.into_iter().collect();
    entries.sort_by(|a, b| rank(&a.0).cmp(&rank(&b.0)).then(b.1 .1.len().cmp(&a.1 .1.len())));
    let tracks = entries
        .into_iter()
        .enumerate()
        .map(|(i, (name, (sample, mut notes)))| {
            notes.sort_unstable();
            notes.dedup();
            Track { id: first_id + i as u64, name, sample, volume: 100, muted: false, solo: false, color: (i % 8) as u8, notes, key: None }
        })
        .collect();

    // Volume points: one per change of the green lines' volume.
    let mut control_points: Vec<ControlPoint> = Vec::new();
    for p in &points {
        let v = p.volume.clamp(5, 100);
        if control_points.last().map(|c| c.volume) != Some(v) {
            let time = p.time.max(0.0).round() as i64;
            if let Some(last) = control_points.last_mut().filter(|c| c.time == time) {
                last.volume = v;
            } else {
                control_points.push(ControlPoint { time, volume: v });
            }
        }
    }
    if control_points.is_empty() {
        control_points.push(ControlPoint { time: 0, volume: 70 });
    }
    control_points.dedup_by_key(|c| c.time);
    Imported { tracks, control_points, hits, missing: { missing.sort(); missing.dedup(); missing } }
}

/// Does this difficulty contain hitsounds worth importing (any hit objects)?
pub fn has_hits(text: &str) -> bool {
    let mut in_objects = false;
    for line in text.lines() {
        let l = line.trim();
        if l.starts_with('[') {
            in_objects = l == "[HitObjects]";
        } else if in_objects && !l.is_empty() {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn lib(source: SampleSource, files: &[&str]) -> Vec<SampleEntry> {
        files.iter().map(|f| SampleEntry { r: SampleRef { source, file: f.to_string() }, path: PathBuf::from(f) }).collect()
    }

    #[test]
    fn reads_sets_indices_and_volumes_like_osu() {
        let text = "osu file format v14\r\n[General]\r\nSampleSet: Soft\r\n[TimingPoints]\r\n0,500,4,2,0,70,1,0\r\n1000,-100,4,2,2,80,0,0\r\n2000,-100,4,2,3,80,0,0\r\n[HitObjects]\r\n256,192,500,1,0,0:0:0:0:\r\n256,192,1000,1,8,0:1:0:0:\r\n256,192,2000,5,10,0:3:0:0:\r\n256,192,2500,1,4,1:0:0:0:\r\n";
        let mut l = lib(SampleSource::Mapset, &["normal-hitclap2.ogg", "drum-hitwhistle3.ogg", "drum-hitclap3.ogg"]);
        l.extend(lib(SampleSource::Skin, &["soft-hitnormal.wav", "normal-hitnormal.wav", "normal-hitfinish.wav"]));
        let imp = import(text, &l, 1);
        assert_eq!(imp.hits, 4);
        let get = |name: &str| imp.tracks.iter().find(|t| t.name == name).unwrap_or_else(|| panic!("no track {name}: {:?}", imp.tracks.iter().map(|t| &t.name).collect::<Vec<_>>()));
        // Hit at 500: index 0 on the red line -> skin soft-hitnormal.
        assert_eq!(get("soft-hitnormal (skin)").notes, vec![500, 1000, 2000]);
        // 1000: clap with addition set Normal, index 2 from the green line.
        assert_eq!(get("normal-hitclap2").notes, vec![1000]);
        // 2000: whistle + clap on the drum addition set, index 3.
        assert_eq!(get("drum-hitwhistle3").notes, vec![2000]);
        assert_eq!(get("drum-hitclap3").notes, vec![2000]);
        // 2500: normal set from the object, index 3 has no file -> skin defaults.
        assert_eq!(get("normal-hitnormal (skin)").notes, vec![2500]);
        assert_eq!(get("normal-hitfinish (skin)").notes, vec![2500]);
        // Normals come first.
        assert!(imp.tracks[0].name.contains("hitnormal"));
        // Volumes 70 -> 80.
        assert_eq!(imp.control_points.iter().map(|c| (c.time, c.volume)).collect::<Vec<_>>(), vec![(0, 70), (1000, 80)]);
        assert!(imp.missing.is_empty());
    }
}

#[cfg(test)]
mod mania_tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn hold_notes_and_keysounds() {
        let text = "osu file format v14\r\n[General]\r\nSampleSet: Normal\r\nMode: 3\r\n[TimingPoints]\r\n0,500,4,1,0,60,1,0\r\n[HitObjects]\r\n64,192,1000,128,2,1500:2:0:0:0:\r\n192,192,2000,1,0,0:0:0:80:kick.wav\r\n";
        let lib = vec![
            SampleEntry { r: SampleRef { source: SampleSource::Mapset, file: "kick.wav".into() }, path: PathBuf::from("kick.wav") },
            SampleEntry { r: SampleRef { source: SampleSource::Skin, file: "soft-hitnormal.wav".into() }, path: PathBuf::from("a") },
            SampleEntry { r: SampleRef { source: SampleSource::Skin, file: "soft-hitwhistle.wav".into() }, path: PathBuf::from("b") },
        ];
        let imp = import(text, &lib, 1);
        let names: Vec<&str> = imp.tracks.iter().map(|t| t.name.as_str()).collect();
        // Hold note: the sample set (2 = soft) comes after the end time, not the end time itself.
        assert!(names.contains(&"soft-hitnormal (skin)"), "{names:?}");
        assert!(names.contains(&"soft-hitwhistle (skin)"), "{names:?}");
        // Keysound note plays kick.wav instead of a hitnormal.
        let kick = imp.tracks.iter().find(|t| t.name == "kick").expect("keysound track");
        assert_eq!(kick.notes, vec![2000]);
        assert!(imp.missing.is_empty());
    }
}

#[cfg(test)]
mod real_map {
    use super::*;
    use std::io::Read;
    use std::path::PathBuf;

    /// Run with HITSMITH_TEST_OSZ=<path to an .osz with a hitsound difficulty> cargo test -- --ignored real_
    #[test]
    #[ignore = "needs a real .osz; set HITSMITH_TEST_OSZ"]
    fn real_hitsound_difficulty() {
        let path = std::env::var("HITSMITH_TEST_OSZ").expect("set HITSMITH_TEST_OSZ");
        let mut z = zip::ZipArchive::new(std::fs::File::open(&path).unwrap()).unwrap();
        let mut lib = Vec::new();
        let mut text = String::new();
        for i in 0..z.len() {
            let mut e = z.by_index(i).unwrap();
            let name = e.name().unwrap().to_string();
            let lower = name.to_ascii_lowercase();
            if lower.contains("hitsound") && lower.ends_with(".osu") {
                e.read_to_string(&mut text).unwrap();
            } else if [".wav", ".ogg", ".mp3"].iter().any(|x| lower.ends_with(x)) {
                lib.push(SampleEntry { r: SampleRef { source: SampleSource::Mapset, file: name.clone() }, path: PathBuf::from(&name) });
            }
        }
        for f in ["normal-hitnormal.wav", "soft-hitnormal.wav", "drum-hitnormal.wav", "normal-hitwhistle.wav", "soft-hitwhistle.wav", "drum-hitwhistle.wav", "normal-hitfinish.wav", "soft-hitfinish.wav", "drum-hitfinish.wav", "normal-hitclap.wav", "soft-hitclap.wav", "drum-hitclap.wav"] {
            lib.push(SampleEntry { r: SampleRef { source: SampleSource::Skin, file: f.into() }, path: PathBuf::from(f) });
        }
        let imp = import(&text, &lib, 1);
        println!("hits {} tracks {} volume points {} missing {:?}", imp.hits, imp.tracks.len(), imp.control_points.len(), imp.missing);
        for t in imp.tracks.iter().take(8) {
            println!("  {:<28} {:>5} hits", t.name, t.notes.len());
        }
        let total: usize = imp.tracks.iter().map(|t| t.notes.len()).sum();
        println!("total sounds placed {total}");
        assert!(imp.hits > 0);
    }
}
