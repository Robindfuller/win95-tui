// What opens what: folders in Files, bitmaps in Paint, video in Media Player,
// music in Amp, text in Notes, and anything else with whatever the system uses
// for it.
use crate::{apps::Launch, icons::Icon};
use std::{fs, io::Read, path::Path};

fn ext(p: &Path) -> String {
    p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}

fn is_bitmap(p: &Path) -> bool {
    ext(p) == "bmp"
}

/// Readable text: no zero bytes near the start, and not huge.
pub fn looks_text(p: &Path) -> bool {
    let mut buf = [0u8; 4096];
    match fs::File::open(p).and_then(|mut f| f.read(&mut buf)) {
        Ok(n) => !buf[..n].contains(&0) && fs::metadata(p).map(|m| m.len() < 4_000_000).unwrap_or(false),
        Err(_) => matches!(ext(p).as_str(), "txt" | "md" | "log"),
    }
}

pub fn launch(p: &Path) -> Launch {
    if p.is_dir() {
        Launch::Explorer(p.to_path_buf())
    } else if is_bitmap(p) {
        Launch::Paint(Some(p.to_path_buf()))
    } else if crate::apps::mplayer::plays(p) {
        Launch::MediaPlayer(Some(p.to_path_buf()))
    } else if crate::apps::amp::plays(p) {
        Launch::Amp(vec![p.to_path_buf()])
    } else if looks_text(p) {
        Launch::Notepad(Some(p.to_path_buf()))
    } else {
        Launch::External(p.to_path_buf())
    }
}

pub fn icon(p: &Path) -> Icon {
    if p.is_dir() {
        Icon::Folder
    } else if is_bitmap(p) {
        Icon::Picture
    } else if crate::apps::mplayer::plays(p) {
        Icon::MediaPlayer
    } else if crate::apps::amp::plays(p) {
        Icon::Amp
    } else if looks_text(p) {
        Icon::Notepad
    } else {
        Icon::File
    }
}

/// What a shortcut to `p` is called on the desktop.
pub fn label(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| p.display().to_string())
}
