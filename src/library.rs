//! Helpers for finding osu! folders. There is no library scan: the user picks
//! a .osu file themselves, which keeps startup instant.

use std::fs;
use std::path::{Path, PathBuf};

/// A sensible starting folder for the "Open beatmap" dialog.
pub fn guess_songs_dir() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        candidates.push(PathBuf::from(local).join("osu!").join("Songs"));
    }
    for drive in ['C', 'D', 'E', 'F'] {
        candidates.push(PathBuf::from(format!("{drive}:\\osu!\\Songs")));
        candidates.push(PathBuf::from(format!("{drive}:\\Games\\osu!\\Songs")));
    }
    candidates.into_iter().find(|p| p.is_dir())
}

/// The folder of the skin selected in osu!'s user config, if the mapset sits in
/// a standard `<osu!>/Songs/<mapset>` layout.
pub fn current_skin_dir(mapset_dir: &Path) -> Option<PathBuf> {
    let osu_root = mapset_dir.parent()?.parent()?;
    for entry in fs::read_dir(osu_root).ok()?.filter_map(|e| e.ok()) {
        let name = entry.file_name().to_string_lossy().to_lowercase();
        if name.starts_with("osu!.") && name.ends_with(".cfg") && name != "osu!.cfg" {
            let text = fs::read_to_string(entry.path()).ok()?;
            for line in text.lines() {
                if let Some((k, v)) = line.split_once('=') {
                    if k.trim() == "Skin" {
                        let skin = v.trim();
                        if skin.is_empty() || skin.contains(['/', '\\']) || skin.contains("..") {
                            return None;
                        }
                        let dir = osu_root.join("Skins").join(skin);
                        return dir.is_dir().then_some(dir);
                    }
                }
            }
        }
    }
    None
}
