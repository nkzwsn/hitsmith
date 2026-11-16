//! Project model, persistence and the list of usable hitsound samples.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

pub const AUDIO_EXTS: [&str; 3] = ["wav", "ogg", "mp3"];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SampleSource {
    Mapset,
    Skin,
    Project,
}

impl SampleSource {
    pub fn label(self) -> &'static str {
        match self {
            SampleSource::Mapset => "Mapset",
            SampleSource::Skin => "Skin",
            SampleSource::Project => "Project",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SampleRef {
    pub source: SampleSource,
    pub file: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub id: u64,
    pub name: String,
    pub sample: Option<SampleRef>,
    pub volume: u8,
    #[serde(default)]
    pub muted: bool,
    #[serde(default)]
    pub solo: bool,
    pub color: u8,
    /// Sorted, unique note times in milliseconds.
    pub notes: Vec<i64>,
    /// Key that toggles a hit at the playhead on this track.
    /// `None` = use the default (Q/W/E/R), `Some("")` = no key, otherwise an egui key name.
    #[serde(default)]
    pub key: Option<String>,
}

impl Track {
    pub fn insert_note(&mut self, t: i64) -> bool {
        match self.notes.binary_search(&t) {
            Ok(_) => false,
            Err(i) => {
                self.notes.insert(i, t);
                true
            }
        }
    }
    pub fn remove_note(&mut self, t: i64) -> bool {
        match self.notes.binary_search(&t) {
            Ok(i) => {
                self.notes.remove(i);
                true
            }
            Err(_) => false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ControlPoint {
    pub time: i64,
    pub volume: u8,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExportOptions {
    pub diff_name: String,
    pub start_index: u32,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self { diff_name: "Hitsounds".into(), start_index: 1 }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Content {
    pub tracks: Vec<Track>,
    pub control_points: Vec<ControlPoint>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Project {
    pub format: u32,
    pub mapset_dir: PathBuf,
    pub osu_file: String,
    pub content: Content,
    #[serde(default)]
    pub export: ExportOptions,
    #[serde(default = "default_zoom")]
    pub zoom: f32,
    #[serde(default = "default_div")]
    pub divisor: u32,
    #[serde(default)]
    pub next_id: u64,
    #[serde(default)]
    pub bookmarks: Vec<i64>,
}

fn default_zoom() -> f32 {
    0.25
}
fn default_div() -> u32 {
    4
}

impl Project {
    pub fn new(mapset_dir: PathBuf, osu_file: String) -> Self {
        Self {
            format: 1,
            mapset_dir,
            osu_file,
            content: Content { tracks: Vec::new(), control_points: vec![ControlPoint { time: 0, volume: 70 }] },
            export: ExportOptions::default(),
            zoom: default_zoom(),
            divisor: default_div(),
            next_id: 1,
            bookmarks: Vec::new(),
        }
    }

    pub fn volume_at(&self, t: i64) -> u8 {
        let cps = &self.content.control_points;
        let i = cps.partition_point(|c| c.time <= t);
        if i == 0 { cps.first().map(|c| c.volume).unwrap_or(100) } else { cps[i - 1].volume }
    }

    pub fn audible(&self, track: &Track) -> bool {
        let any_solo = self.content.tracks.iter().any(|t| t.solo);
        if any_solo { track.solo } else { !track.muted }
    }
}

/// Where a project lives on disk: `<projects>/<name>/project.json` plus `samples/`.
#[derive(Clone, Debug)]
pub struct ProjectEntry {
    pub name: String,
    pub dir: PathBuf,
    pub modified: SystemTime,
    pub mapset_label: String,
    /// Mapset background image, for the project card.
    pub background: Option<PathBuf>,
    pub hits: usize,
}

impl ProjectEntry {
    pub fn json(&self) -> PathBuf {
        self.dir.join("project.json")
    }
    pub fn samples_dir(&self) -> PathBuf {
        self.dir.join("samples")
    }
}

pub fn validate_name(name: &str) -> Result<()> {
    let n = name.trim();
    if n.is_empty() {
        bail!("Enter a project name.");
    }
    if n.len() > 80 {
        bail!("Name is too long.");
    }
    if n.chars().any(|c| matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || c.is_control()) {
        bail!("Name contains characters Windows doesn't allow.");
    }
    if n.ends_with('.') || n == ".." {
        bail!("Name can't end with a dot.");
    }
    let stem = n.split('.').next().unwrap_or("").to_ascii_uppercase();
    let reserved = ["CON", "PRN", "AUX", "NUL"].contains(&stem.as_str())
        || ((stem.starts_with("COM") || stem.starts_with("LPT")) && stem.len() == 4 && stem.as_bytes()[3].is_ascii_digit());
    if reserved {
        bail!("That name is reserved by Windows.");
    }
    Ok(())
}

pub fn list_projects(root: &Path) -> Vec<ProjectEntry> {
    let Ok(rd) = fs::read_dir(root) else { return Vec::new() };
    let mut out: Vec<ProjectEntry> = rd
        .filter_map(|e| e.ok())
        .filter(|e| e.path().join("project.json").is_file())
        .map(|e| {
            let dir = e.path();
            let json = dir.join("project.json");
            let modified = fs::metadata(&json).and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
            let project = load(&json).ok();
            let meta = project.as_ref().and_then(|p| crate::osu::read_beatmap(&p.mapset_dir.join(&p.osu_file)).ok().map(|b| (p.mapset_dir.clone(), b.meta)));
            let mapset_label = match (&meta, &project) {
                (Some((_, m)), _) => format!("{} - {}", m.display_artist(), m.display_title()),
                (None, Some(p)) => p.mapset_dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                (None, None) => "(unreadable project)".into(),
            };
            let background = meta.as_ref().filter(|(_, m)| !m.background.is_empty()).map(|(d, m)| d.join(&m.background)).filter(|p| p.is_file());
            let hits = project.as_ref().map(|p| p.content.tracks.iter().map(|t| t.notes.len()).sum()).unwrap_or(0);
            ProjectEntry { name: e.file_name().to_string_lossy().into_owned(), dir, modified, mapset_label, background, hits }
        })
        .collect();
    out.sort_by(|a, b| b.modified.cmp(&a.modified));
    out
}

pub fn load(path: &Path) -> Result<Project> {
    let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let mut p: Project = serde_json::from_slice(&bytes).with_context(|| format!("{} is not a valid project", path.display()))?;
    for t in &mut p.content.tracks {
        t.notes.sort_unstable();
        t.notes.dedup();
        t.volume = t.volume.min(100);
    }
    p.content.control_points.sort_by_key(|c| c.time);
    p.content.control_points.dedup_by_key(|c| c.time);
    for c in &mut p.content.control_points {
        c.volume = c.volume.clamp(5, 100);
    }
    p.divisor = p.divisor.clamp(1, 16);
    p.bookmarks.sort_unstable();
    p.bookmarks.dedup();
    p.next_id = p.next_id.max(p.content.tracks.iter().map(|t| t.id + 1).max().unwrap_or(1));
    Ok(p)
}

pub fn save(path: &Path, project: &Project) -> Result<()> {
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(project)?)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

pub fn create(root: &Path, name: &str, project: &Project) -> Result<ProjectEntry> {
    validate_name(name)?;
    let dir = root.join(name.trim());
    if dir.exists() {
        bail!("A project with that name already exists.");
    }
    fs::create_dir_all(dir.join("samples"))?;
    let entry = ProjectEntry { name: name.trim().to_string(), dir, modified: SystemTime::now(), mapset_label: String::new(), background: None, hits: 0 };
    save(&entry.json(), project)?;
    Ok(entry)
}

/// One selectable sample file.
#[derive(Clone, Debug)]
pub struct SampleEntry {
    pub r: SampleRef,
    pub path: PathBuf,
}

fn is_audio(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).map(|e| AUDIO_EXTS.iter().any(|x| x.eq_ignore_ascii_case(e))).unwrap_or(false)
}

fn list_audio(dir: &Path, source: SampleSource, skip: &str, out: &mut Vec<SampleEntry>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    let mut found: Vec<SampleEntry> = rd
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file() && is_audio(p))
        .filter_map(|p| {
            let file = p.file_name()?.to_string_lossy().into_owned();
            (!file.eq_ignore_ascii_case(skip)).then(|| SampleEntry { r: SampleRef { source, file }, path: p })
        })
        .collect();
    found.sort_by(|a, b| a.r.file.to_lowercase().cmp(&b.r.file.to_lowercase()));
    out.extend(found);
}

/// Every sample a track can use: project imports, mapset hitsounds, the current skin's hitsounds.
pub fn sample_library(project_samples: &Path, mapset_dir: &Path, song_audio: &str, skin_dir: Option<&Path>, exported: &[String]) -> Vec<SampleEntry> {
    let mut out = Vec::new();
    list_audio(project_samples, SampleSource::Project, "", &mut out);
    let before = out.len();
    list_audio(mapset_dir, SampleSource::Mapset, song_audio, &mut out);
    // Hide files our own exporter generated so they don't feed back into the next export.
    let mut i = before;
    while i < out.len() {
        if exported.iter().any(|f| f.eq_ignore_ascii_case(&out[i].r.file)) {
            out.remove(i);
        } else {
            i += 1;
        }
    }
    if let Some(skin) = skin_dir {
        let start = out.len();
        list_audio(skin, SampleSource::Skin, "", &mut out);
        // Only real hitsound names from skins; skins carry lots of UI sounds.
        let mut j = start;
        while j < out.len() {
            let f = out[j].r.file.to_ascii_lowercase();
            let keep = ["normal-hit", "soft-hit", "drum-hit"].iter().any(|p| f.starts_with(p));
            if keep { j += 1 } else { out.remove(j); }
        }
    }
    out
}

/// Copy an external audio file into the project's samples folder. Returns the new file name.
pub fn import_sample(samples_dir: &Path, src: &Path) -> Result<String> {
    if !is_audio(src) {
        bail!("Pick a .wav, .ogg or .mp3 file.");
    }
    fs::create_dir_all(samples_dir)?;
    let name = src.file_name().context("bad file name")?.to_string_lossy().into_owned();
    let stem = Path::new(&name).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = Path::new(&name).extension().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let mut target = samples_dir.join(&name);
    let mut n = 2;
    while target.exists() {
        target = samples_dir.join(format!("{stem} ({n}).{ext}"));
        n += 1;
    }
    fs::copy(src, &target).with_context(|| format!("copying {}", src.display()))?;
    Ok(target.file_name().unwrap().to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert!(validate_name("My map").is_ok());
        assert!(validate_name("con").is_err());
        assert!(validate_name("COM1.txt").is_err());
        assert!(validate_name("a/b").is_err());
        assert!(validate_name("x.").is_err());
        assert!(validate_name("Combo").is_ok());
    }

    #[test]
    fn volume_lookup() {
        let mut p = Project::new(PathBuf::from("x"), "a.osu".into());
        p.content.control_points = vec![ControlPoint { time: 1000, volume: 40 }, ControlPoint { time: 2000, volume: 80 }];
        assert_eq!(p.volume_at(0), 40);
        assert_eq!(p.volume_at(1500), 40);
        assert_eq!(p.volume_at(2000), 80);
    }
}

/// Move a folder to the Windows Recycle Bin so a deleted project can be restored.
pub fn move_to_recycle_bin(path: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::{FO_DELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, SHFILEOPSTRUCTW, SHFileOperationW};
    let abs = std::path::absolute(path)?;
    // pFrom is a list of paths, so it needs a double NUL terminator.
    let mut wide: Vec<u16> = abs.as_os_str().encode_wide().collect();
    wide.extend([0, 0]);
    let mut op = SHFILEOPSTRUCTW {
        wFunc: FO_DELETE,
        pFrom: wide.as_ptr(),
        fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_NOERRORUI | FOF_SILENT) as u16,
        ..Default::default()
    };
    let code = unsafe { SHFileOperationW(&mut op) };
    if code != 0 || op.fAnyOperationsAborted != 0 {
        bail!("Windows couldn't move it to the Recycle Bin (code {code}).");
    }
    Ok(())
}

#[cfg(test)]
mod recycle_tests {
    #[test]
    #[ignore = "puts a folder in the real Recycle Bin; run with --ignored"]
    fn recycles_a_folder() {
        let dir = std::env::temp_dir().join(format!("hitsmith-recycle-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("samples")).unwrap();
        std::fs::write(dir.join("project.json"), b"{}").unwrap();
        super::move_to_recycle_bin(&dir).unwrap();
        assert!(!dir.exists());
    }
}

/// The key each track actually responds to: custom keys first, then the defaults
/// Q/W/E/R (normal/whistle/finish/clap) go to the track whose sample is that kind,
/// and any left over go to the first four tracks in order.
pub fn effective_keys(tracks: &[Track]) -> Vec<Option<String>> {
    // Match the hitsound part of the name ("hitclap"), not the sample set ("normal-..."),
    // so normal-hitclap.wav counts as a clap.
    const DEFAULTS: [(&str, &str); 4] = [("hitnormal", "Q"), ("hitwhistle", "W"), ("hitfinish", "E"), ("hitclap", "R")];
    let mut out: Vec<Option<String>> = tracks.iter().map(|t| t.key.clone().filter(|k| !k.is_empty())).collect();
    let taken = |out: &Vec<Option<String>>, key: &str| out.iter().any(|k| k.as_deref() == Some(key));
    let free = |out: &Vec<Option<String>>, i: usize| tracks[i].key.is_none() && out[i].is_none();
    let mut pending = Vec::new();
    for (idx, (kind, key)) in DEFAULTS.iter().enumerate() {
        if taken(&out, key) {
            continue;
        }
        let by_sample = (0..tracks.len()).find(|&i| free(&out, i) && tracks[i].sample.as_ref().is_some_and(|s| s.file.to_ascii_lowercase().contains(kind)));
        match by_sample {
            Some(i) => out[i] = Some(key.to_string()),
            None => pending.push((idx, key)),
        }
    }
    for (idx, key) in pending {
        if !taken(&out, key) && idx < tracks.len() && free(&out, idx) {
            out[idx] = Some(key.to_string());
        }
    }
    out
}

#[cfg(test)]
mod key_tests {
    use super::*;

    fn track(id: u64, file: &str, key: Option<&str>) -> Track {
        Track {
            id,
            name: format!("t{id}"),
            sample: Some(SampleRef { source: SampleSource::Mapset, file: file.into() }),
            volume: 80,
            muted: false,
            solo: false,
            color: 0,
            notes: vec![],
            key: key.map(str::to_string),
        }
    }

    #[test]
    fn defaults_follow_sample_kind_then_order() {
        let t = vec![track(1, "kick.wav", None), track(2, "soft-hitclap.wav", None), track(3, "drum-hitwhistle.ogg", None)];
        let k = effective_keys(&t);
        // whistle and clap match by sample; Q falls back to the first track, E has no free track left in order.
        assert_eq!(k, vec![Some("Q".into()), Some("R".into()), Some("W".into())]);
    }

    #[test]
    fn custom_keys_win_and_free_their_default() {
        let t = vec![track(1, "normal-hitnormal.wav", Some("A")), track(2, "x.wav", Some("")), track(3, "y.wav", None)];
        let k = effective_keys(&t);
        assert_eq!(k[0].as_deref(), Some("A"));
        assert_eq!(k[1], None, "explicitly no key");
        // Q is unused by anyone, but track 0 (Q's fallback) has a custom key, so Q stays free; track 3 gets E by order.
        assert_eq!(k[2].as_deref(), Some("E"));
    }

    #[test]
    fn a_custom_key_can_take_a_default_letter() {
        let t = vec![track(1, "normal-hitnormal.wav", None), track(2, "z.wav", Some("Q"))];
        let k = effective_keys(&t);
        assert_eq!(k[1].as_deref(), Some("Q"));
        assert_ne!(k[0].as_deref(), Some("Q"));
    }
}

#[cfg(test)]
mod key_match_tests {
    use super::*;

    #[test]
    fn sample_set_prefix_is_not_the_hitsound() {
        let mk = |id, f: &str| Track { id, name: String::new(), sample: Some(SampleRef { source: SampleSource::Mapset, file: f.into() }), volume: 80, muted: false, solo: false, color: 0, notes: vec![], key: None };
        let k = effective_keys(&[mk(1, "normal-hitclap.wav"), mk(2, "normal-hitnormal.wav")]);
        assert_eq!(k[0].as_deref(), Some("R"));
        assert_eq!(k[1].as_deref(), Some("Q"));
    }
}

/// Unzip an .osz (a zipped mapset) into `songs_dir/<name>`, like osu! does on import.
/// If that folder already holds a mapset, it is reused instead of overwritten.
pub fn import_osz(osz: &Path, songs_dir: &Path) -> Result<PathBuf> {
    const MAX_TOTAL: u64 = 2 * 1024 * 1024 * 1024; // refuse absurd archives
    let stem = osz.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let name: String = stem.chars().filter(|c| !matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') && !c.is_control()).collect();
    let name = name.trim().trim_end_matches('.').to_string();
    if name.is_empty() {
        bail!("That .osz has no usable name.");
    }
    let target = songs_dir.join(&name);
    let has_osu = |dir: &Path| fs::read_dir(dir).map(|rd| rd.filter_map(|e| e.ok()).any(|e| e.file_name().to_string_lossy().to_ascii_lowercase().ends_with(".osu"))).unwrap_or(false);
    if has_osu(&target) {
        return Ok(target);
    }
    let file = fs::File::open(osz).with_context(|| format!("opening {}", osz.display()))?;
    let mut archive = zip::ZipArchive::new(file).context("that .osz isn't a valid archive")?;
    // osu! may already have this mapset under its own folder name ("123456 Artist - Title"):
    // reuse that instead of unpacking a duplicate.
    if let Some(meta) = osz_meta(&mut archive) {
        if let Some(existing) = find_existing_mapset(songs_dir, &meta) {
            return Ok(existing);
        }
    }
    fs::create_dir_all(&target)?;
    let mut total = 0u64;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        // enclosed_name rejects absolute paths and ".." so nothing lands outside the mapset folder.
        let Some(rel) = entry.enclosed_name() else { continue };
        let out = target.join(rel);
        if entry.is_dir() {
            fs::create_dir_all(&out)?;
            continue;
        }
        total += entry.size();
        if total > MAX_TOTAL {
            bail!("That .osz is too large to import.");
        }
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut f = fs::File::create(&out)?;
        std::io::copy(&mut entry, &mut f)?;
    }
    if !has_osu(&target) {
        bail!("That .osz doesn't contain any difficulty (.osu).");
    }
    Ok(target)
}

/// Starter tracks for a new project: Normal, Whistle, Finish and Clap, each with the
/// best matching sample (the mapset's own first, plain "normal-" set preferred, then the skin).
pub fn default_tracks(library: &[SampleEntry]) -> Vec<Track> {
    const KINDS: [(&str, &str); 4] = [("Normal", "hitnormal"), ("Whistle", "hitwhistle"), ("Finish", "hitfinish"), ("Clap", "hitclap")];
    let score = |e: &SampleEntry, kind: &str| -> Option<u32> {
        let f = e.r.file.to_ascii_lowercase();
        let stem = f.rsplit_once('.').map(|(a, _)| a).unwrap_or(&f);
        let (set, rest) = stem.split_once('-')?;
        let digits = rest.strip_prefix(kind)?;
        if !digits.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        let source = match e.r.source {
            SampleSource::Mapset => 0,
            SampleSource::Project => 1,
            SampleSource::Skin => 2,
        };
        let set_rank = match set {
            "normal" => 0,
            "soft" => 1,
            "drum" => 2,
            _ => return None,
        };
        Some(source * 100 + set_rank * 10 + if digits.is_empty() { 0 } else { 1 })
    };
    KINDS
        .iter()
        .enumerate()
        .map(|(i, (name, kind))| {
            let sample = library.iter().filter_map(|e| score(e, kind).map(|s| (s, e))).min_by_key(|(s, _)| *s).map(|(_, e)| e.r.clone());
            Track { id: i as u64 + 1, name: name.to_string(), sample, volume: 80, muted: false, solo: false, color: i as u8, notes: Vec::new(), key: None }
        })
        .collect()
}

#[cfg(test)]
mod quick_start_tests {
    use super::*;

    fn entry(source: SampleSource, file: &str) -> SampleEntry {
        SampleEntry { r: SampleRef { source, file: file.into() }, path: PathBuf::from(file) }
    }

    #[test]
    fn default_tracks_prefer_mapset_plain_normal_set() {
        let lib = vec![
            entry(SampleSource::Skin, "normal-hitclap.wav"),
            entry(SampleSource::Mapset, "soft-hitclap.wav"),
            entry(SampleSource::Mapset, "normal-hitnormal2.wav"),
            entry(SampleSource::Mapset, "normal-hitnormal.wav"),
            entry(SampleSource::Mapset, "soft-sliderslide.wav"),
            entry(SampleSource::Skin, "drum-hitwhistle.wav"),
        ];
        let t = default_tracks(&lib);
        let pick = |i: usize| t[i].sample.as_ref().map(|s| (s.source, s.file.clone()));
        assert_eq!(pick(0), Some((SampleSource::Mapset, "normal-hitnormal.wav".into())));
        assert_eq!(pick(1), Some((SampleSource::Skin, "drum-hitwhistle.wav".into())));
        assert_eq!(pick(2), None, "no finish sample anywhere");
        assert_eq!(pick(3), Some((SampleSource::Mapset, "soft-hitclap.wav".into())));
        // Their default keys follow the sample kind.
        let keys = effective_keys(&t);
        assert_eq!(keys[0].as_deref(), Some("Q"));
        assert_eq!(keys[3].as_deref(), Some("R"));
    }

    #[test]
    fn osz_import_extracts_and_reuses() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("hitsmith-osz-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let osz = dir.join("123 Artist - Song.osz");
        {
            let mut w = zip::ZipWriter::new(fs::File::create(&osz).unwrap());
            let opts = zip::write::SimpleFileOptions::default();
            w.start_file("Artist - Song (me) [Easy].osu", opts).unwrap();
            w.write_all(b"osu file format v14").unwrap();
            w.start_file("../evil.txt", opts).unwrap();
            w.write_all(b"no").unwrap();
            w.finish().unwrap();
        }
        let songs = dir.join("Songs");
        fs::create_dir_all(&songs).unwrap();
        let out = import_osz(&osz, &songs).unwrap();
        assert_eq!(out, songs.join("123 Artist - Song"));
        assert!(out.join("Artist - Song (me) [Easy].osu").exists());
        assert!(!songs.join("evil.txt").exists() && !dir.join("evil.txt").exists(), "zip-slip entry must be skipped");
        // Second import reuses the folder.
        assert_eq!(import_osz(&osz, &songs).unwrap(), out);
        fs::remove_dir_all(&dir).ok();
    }
}

/// Metadata of the first difficulty inside an .osz.
fn osz_meta(archive: &mut zip::ZipArchive<fs::File>) -> Option<crate::osu::Meta> {
    use std::io::Read;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).ok()?;
        if entry.name().is_ok_and(|n| n.to_ascii_lowercase().ends_with(".osu")) && entry.size() < 16 * 1024 * 1024 {
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).ok()?;
            let text = String::from_utf8_lossy(&bytes);
            return Some(crate::osu::parse_meta(text.trim_start_matches('\u{feff}')));
        }
    }
    None
}

/// Letters and digits only, lower-cased, so "feat." and "feat" compare equal.
fn loose(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// Find a folder in `songs_dir` that already holds the same mapset. Candidates are picked
/// by folder name (cheap), then confirmed by reading one difficulty's metadata.
pub fn find_existing_mapset(songs_dir: &Path, meta: &crate::osu::Meta) -> Option<PathBuf> {
    let want_name = loose(&format!("{}{}", meta.artist, meta.title));
    if want_name.is_empty() {
        return None;
    }
    let id_prefix = (meta.set_id > 0).then(|| format!("{} ", meta.set_id));
    let mut candidates: Vec<PathBuf> = fs::read_dir(songs_dir)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let by_id = id_prefix.as_ref().is_some_and(|p| name.starts_with(p.as_str()));
            (by_id || loose(&name).contains(&want_name)).then(|| e.path())
        })
        .collect();
    candidates.sort();
    // Same mapset = same mapper, artist and title (and set ID when both have one).
    let same = |other: &crate::osu::Meta| {
        let ids_agree = meta.set_id <= 0 || other.set_id <= 0 || meta.set_id == other.set_id;
        ids_agree && loose(&other.creator) == loose(&meta.creator) && loose(&other.title) == loose(&meta.title) && loose(&other.artist) == loose(&meta.artist)
    };
    candidates.into_iter().take(20).find(|dir| {
        let Ok(rd) = fs::read_dir(dir) else { return false };
        let osu = rd.filter_map(|e| e.ok()).map(|e| e.path()).find(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("osu")));
        osu.and_then(|p| fs::read(p).ok()).is_some_and(|b| same(&crate::osu::parse_meta(String::from_utf8_lossy(&b).trim_start_matches('\u{feff}'))))
    })
}

#[cfg(test)]
mod duplicate_tests {
    use super::*;
    use std::io::Write;

    fn osu_text(artist: &str, title: &str, creator: &str, set_id: i64, version: &str) -> String {
        format!("osu file format v14\r\n\r\n[General]\r\nAudioFilename: audio.mp3\r\n\r\n[Metadata]\r\nTitle:{title}\r\nArtist:{artist}\r\nCreator:{creator}\r\nVersion:{version}\r\nBeatmapSetID:{set_id}\r\n")
    }

    fn make_osz(path: &Path, osu: &str) {
        let mut w = zip::ZipWriter::new(fs::File::create(path).unwrap());
        let opts = zip::write::SimpleFileOptions::default();
        w.start_file("map [Hard].osu", opts).unwrap();
        w.write_all(osu.as_bytes()).unwrap();
        w.finish().unwrap();
    }

    #[test]
    fn reuses_the_copy_osu_already_has() {
        let dir = std::env::temp_dir().join(format!("hitsmith-dupe-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let songs = dir.join("Songs");
        // osu!'s own copy: ID-prefixed folder, punctuation dropped from the name.
        let existing = songs.join("1884717 40mP feat Hatsune Miku - The Noise of Rain [no video]");
        fs::create_dir_all(&existing).unwrap();
        fs::write(existing.join("a [Easy].osu"), osu_text("40mP feat. Hatsune Miku", "The Noise of Rain", "SupaV", 1884717, "Easy")).unwrap();
        // A different map of the same song by another mapper.
        let other = songs.join("859370 _namirin - The Noise of Rain");
        fs::create_dir_all(&other).unwrap();
        fs::write(other.join("b.osu"), osu_text("_namirin", "The Noise of Rain", "someone", 859370, "x")).unwrap();

        let osz = dir.join("40mP feat. Hatsune Miku - The Noise of Rain.osz");
        make_osz(&osz, &osu_text("40mP feat. Hatsune Miku", "The Noise of Rain", "SupaV", 1884717, "Hard"));
        assert_eq!(import_osz(&osz, &songs).unwrap(), existing, "must reuse osu!'s folder");
        assert!(!songs.join("40mP feat. Hatsune Miku - The Noise of Rain").exists(), "no duplicate unpacked");

        // Unsubmitted map (no set ID), same artist/title but a different mapper: unpack fresh.
        let osz2 = dir.join("My Own - Version.osz");
        make_osz(&osz2, &osu_text("40mP feat. Hatsune Miku", "The Noise of Rain", "me", -1, "Hard"));
        let fresh = import_osz(&osz2, &songs).unwrap();
        assert_eq!(fresh, songs.join("My Own - Version"));
        fs::remove_dir_all(&dir).ok();
    }
}
