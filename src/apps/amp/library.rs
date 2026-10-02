// The playlist, favourite stations and settings, kept in ~/.config/win95-tui/amp.
use super::radio::Station;
use crate::theme::home;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc::{channel, Receiver, Sender},
    thread,
};

#[derive(Clone, Debug)]
pub struct Entry {
    /// a file path or a stream URL
    pub loc: String,
    pub title: String,
    pub dur: Option<f64>,
    pub stream: bool,
}

impl Entry {
    pub fn file(p: &Path) -> Entry {
        let title = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| p.display().to_string());
        Entry { loc: p.display().to_string(), title, dur: None, stream: false }
    }

    pub fn stream(url: &str, name: &str) -> Entry {
        Entry { loc: url.into(), title: if name.is_empty() { url.into() } else { name.into() }, dur: None, stream: true }
    }

    pub fn station(s: &Station) -> Entry {
        Entry::stream(&s.url, &s.name)
    }
}

pub fn is_url(s: &str) -> bool {
    s.contains("://") && !s.starts_with("file://")
}

const AUDIO: [&str; 14] = ["mp3", "flac", "ogg", "oga", "opus", "m4a", "aac", "wav", "wma", "aiff", "aif", "ape", "mka", "mpc"];

pub fn is_audio(p: &Path) -> bool {
    p.extension().is_some_and(|e| AUDIO.contains(&e.to_string_lossy().to_lowercase().as_str()))
}

/// Every audio file in a folder and the folders inside it, in name order.
pub fn scan(dir: &Path) -> Vec<PathBuf> {
    let mut out = vec![];
    let Ok(rd) = fs::read_dir(dir) else { return out };
    let mut items: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| !hidden(p)).collect();
    items.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()));
    for p in items {
        if p.is_dir() {
            out.extend(scan(&p));
        } else if is_audio(&p) {
            out.push(p);
        }
    }
    out
}

pub fn hidden(p: &Path) -> bool {
    p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.'))
}

/// What a dropped or pasted bit of text stands for: files, folders and URLs.
pub fn entries_from(text: &str) -> Vec<Entry> {
    let mut out = vec![];
    for word in split_words(text) {
        let word = word.strip_prefix("file://").map(unescape_uri).unwrap_or(word);
        if is_url(&word) {
            out.push(Entry::stream(&word, ""));
            continue;
        }
        let p = PathBuf::from(&word);
        if p.is_dir() {
            out.extend(scan(&p).iter().map(|f| Entry::file(f)));
        } else if p.is_file() && (is_audio(&p) || is_playlist(&p)) {
            if is_playlist(&p) {
                out.extend(load_m3u(&p));
            } else {
                out.push(Entry::file(&p));
            }
        }
    }
    out
}

fn is_playlist(p: &Path) -> bool {
    p.extension().is_some_and(|e| matches!(e.to_string_lossy().to_lowercase().as_str(), "m3u" | "m3u8"))
}

/// Splits on whitespace and newlines like a shell, honouring quotes and
/// backslashes, which is how terminals paste dragged files.
fn split_words(s: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut any = false;
    let mut it = s.chars();
    while let Some(c) = it.next() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => cur.push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                any = true;
            }
            (None, '\\') => {
                if let Some(n) = it.next() {
                    cur.push(n);
                    any = true;
                }
            }
            (None, c) if c.is_whitespace() => {
                if any || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
                any = false;
            }
            (None, c) => cur.push(c),
        }
    }
    if any || !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn unescape_uri(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = vec![];
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Some(v) = std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into()
}

fn dir() -> PathBuf {
    home().join(".config/win95-tui/amp")
}

fn playlist_path() -> PathBuf {
    dir().join("playlist.m3u")
}

pub fn load_playlist() -> Vec<Entry> {
    load_m3u(&playlist_path())
}

pub fn load_m3u(p: &Path) -> Vec<Entry> {
    let base = p.parent().map(Path::to_path_buf).unwrap_or_default();
    let mut out = vec![];
    let mut info: Option<(Option<f64>, String)> = None;
    for line in fs::read_to_string(p).unwrap_or_default().lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("#EXTINF:") {
            let (d, t) = rest.split_once(',').unwrap_or((rest, ""));
            let d = d.trim().parse::<f64>().ok().filter(|d| *d >= 0.0);
            info = Some((d, t.trim().to_string()));
        } else if !line.is_empty() && !line.starts_with('#') {
            let mut e = if is_url(line) {
                Entry::stream(line, "")
            } else {
                let path = if Path::new(line).is_absolute() { PathBuf::from(line) } else { base.join(line) };
                Entry::file(&path)
            };
            if let Some((d, t)) = info.take() {
                e.dur = d;
                if !t.is_empty() {
                    e.title = t;
                }
            }
            out.push(e);
        }
    }
    out
}

pub fn save_playlist(list: &[Entry]) {
    let _ = fs::create_dir_all(dir());
    let mut body = String::from("#EXTM3U\n");
    for e in list {
        let d = e.dur.map(|d| d.round() as i64).unwrap_or(-1);
        body += &format!("#EXTINF:{d},{}\n{}\n", e.title, e.loc);
    }
    let _ = fs::write(playlist_path(), body);
}

pub fn load_favs() -> Vec<Station> {
    fs::read_to_string(dir().join("favourites.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

pub fn save_favs(f: &[Station]) {
    let _ = fs::create_dir_all(dir());
    let _ = fs::write(dir().join("favourites.json"), serde_json::to_string_pretty(f).unwrap_or_default());
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub volume: u8,
    pub balance: f32,
    pub eq_on: bool,
    pub preamp: f32,
    pub eq: [f32; 10],
    pub shuffle: bool,
    pub repeat: bool,
    pub show_eq: bool,
    pub show_pl: bool,
    pub compact: bool,
    pub radio_tab: bool,
    pub vis: u8,
    pub remaining: bool,
    pub browse: PathBuf,
}

impl Default for Settings {
    fn default() -> Self {
        let music = home().join("Music");
        Settings {
            volume: 70,
            balance: 0.0,
            eq_on: true,
            preamp: 0.0,
            eq: [0.0; 10],
            shuffle: false,
            repeat: true,
            show_eq: true,
            show_pl: true,
            compact: false,
            radio_tab: false,
            vis: 0,
            remaining: false,
            browse: if music.is_dir() { music } else { home() },
        }
    }
}

pub fn load_settings() -> Settings {
    fs::read_to_string(dir().join("settings.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

pub fn save_settings(s: &Settings) {
    let _ = fs::create_dir_all(dir());
    let _ = fs::write(dir().join("settings.json"), serde_json::to_string_pretty(s).unwrap_or_default());
}

/// Reads track lengths and tags with ffprobe in the background.
pub struct Prober {
    tx: Sender<String>,
    pub rx: Receiver<(String, Option<String>, Option<f64>)>,
}

impl Prober {
    pub fn new() -> Prober {
        let (tx, jobs) = channel::<String>();
        let (done, rx) = channel();
        thread::spawn(move || {
            for loc in jobs {
                let (t, d) = probe(&loc);
                if done.send((loc, t, d)).is_err() {
                    return;
                }
            }
        });
        Prober { tx, rx }
    }

    pub fn ask(&self, loc: &str) {
        let _ = self.tx.send(loc.into());
    }
}

fn probe(loc: &str) -> (Option<String>, Option<f64>) {
    let Ok(out) = Command::new("ffprobe")
        .args(["-v", "quiet", "-print_format", "json", "-show_format", "-i", loc])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
    else {
        return (None, None);
    };
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&out.stdout) else { return (None, None) };
    let f = &v["format"];
    let dur = f["duration"].as_str().and_then(|s| s.parse().ok());
    let tag = |k: &str| {
        f["tags"].as_object().and_then(|m| m.iter().find(|(n, _)| n.eq_ignore_ascii_case(k))).and_then(|(_, v)| v.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
    };
    let title = match (tag("artist"), tag("title")) {
        (Some(a), Some(t)) => Some(format!("{a} - {t}")),
        (None, Some(t)) => Some(t),
        _ => None,
    };
    (title, dur)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_pasted_paths() {
        assert_eq!(split_words("'/a b/c.mp3' /d\\ e.mp3\nhttp://x"), vec!["/a b/c.mp3", "/d e.mp3", "http://x"]);
        assert_eq!(unescape_uri("/a%20b.mp3"), "/a b.mp3");
    }
}
