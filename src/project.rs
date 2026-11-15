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
            let mapset_label = load(&json)
                .ok()
                .and_then(|p| p.mapset_dir.file_name().map(|n| n.to_string_lossy().into_owned()))
                .unwrap_or_else(|| "(unreadable project)".into());
            ProjectEntry { name: e.file_name().to_string_lossy().into_owned(), dir, modified, mapset_label }
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
    let entry = ProjectEntry { name: name.trim().to_string(), dir, modified: SystemTime::now(), mapset_label: String::new() };
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
