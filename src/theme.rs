// Colours come from the live Omarchy theme, so the desktop follows theme switches.
use ratatui::style::{Color, Style};
use std::{collections::HashMap, fs, path::PathBuf, time::SystemTime};

#[derive(Clone, Debug)]
pub struct Theme {
    pub name: String,
    pub fg_hex: String,
    pub bg_hex: String,
    pub desk: Color,
    /// the desktop colour as RGB, even in the Lite look where `desk` is the terminal's own
    pub desk_rgb: Rgb,
    pub face: Color,
    pub hilite: Color,
    pub shadow: Color,
    pub client: Color,
    pub text: Color,
    pub dim: Color,
    pub accent: Color,
    pub on_accent: Color,
    pub inactive: Color,
    pub on_inactive: Color,
    pub button: Color,
    pub tile_a: Color,
    pub tile_b: Color,
    pub red: Color,
    pub yellow: Color,
    pub green: Color,
    pub cyan: Color,
    pub blue: Color,
    pub magenta: Color,
    pub orange: Color,
}

pub fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/".into()))
}

fn state_dir() -> PathBuf {
    home().join(".local/state/omarchy/current")
}

fn settings_path() -> PathBuf {
    home().join(".config/win95-tui/settings")
}

/// A saved setting from ~/.config/win95-tui/settings ("key=value" lines).
pub fn setting(key: &str) -> Option<String> {
    let txt = fs::read_to_string(settings_path()).ok()?;
    txt.lines().filter_map(|l| l.split_once('=')).find(|(k, _)| k.trim() == key).map(|(_, v)| v.trim().to_string())
}

pub fn save_setting(key: &str, val: &str) {
    let p = settings_path();
    let _ = fs::create_dir_all(p.parent().unwrap());
    let mut lines: Vec<String> = fs::read_to_string(&p)
        .unwrap_or_default()
        .lines()
        .filter(|l| l.split_once('=').is_none_or(|(k, _)| k.trim() != key))
        .map(String::from)
        .collect();
    lines.push(format!("{key}={val}"));
    let _ = fs::write(p, lines.join("\n") + "\n");
}

/// The saved desktop: the tiling Omarchy one, or overlapping Windows ones.
pub fn saved_tiling() -> bool {
    setting("desktop").as_deref() == Some("omarchy")
}

/// Whether the mouse shows as a block; on unless turned off.
pub fn saved_pointer() -> bool {
    setting("pointer").as_deref() != Some("off")
}

pub fn save_pointer(on: bool) {
    save_setting("pointer", if on { "on" } else { "off" });
}

pub fn save_tiling(tiling: bool) {
    save_setting("desktop", if tiling { "omarchy" } else { "windows" });
}

pub fn colors_path() -> PathBuf {
    state_dir().join("theme/colors.toml")
}

pub fn mtime() -> Option<SystemTime> {
    let a = fs::metadata(colors_path()).and_then(|m| m.modified()).ok();
    let b = fs::metadata(state_dir().join("theme.name")).and_then(|m| m.modified()).ok();
    a.max(b)
}

pub type Rgb = (u8, u8, u8);

pub fn hex(s: &str) -> Option<Rgb> {
    let s = s.trim().trim_matches('"').trim_start_matches('#');
    if s.len() < 6 {
        return None;
    }
    let p = |i: usize| u8::from_str_radix(&s[i..i + 2], 16).ok();
    Some((p(0)?, p(2)?, p(4)?))
}

pub fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    (m(a.0, b.0), m(a.1, b.1), m(a.2, b.2))
}

fn lum(c: Rgb) -> f32 {
    let f = |v: u8| {
        let v = v as f32 / 255.0;
        if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * f(c.0) + 0.7152 * f(c.1) + 0.0722 * f(c.2)
}

pub fn contrast(a: Rgb, b: Rgb) -> f32 {
    let (x, y) = (lum(a), lum(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

fn c(v: Rgb) -> Color {
    Color::Rgb(v.0, v.1, v.2)
}

impl Theme {
    /// Style for a selected row or item.
    pub fn sel(&self) -> Style {
        Style::new().fg(self.on_accent).bg(self.accent)
    }

    pub fn load() -> Theme {
        let mut map: HashMap<String, Rgb> = HashMap::new();
        if let Ok(txt) = fs::read_to_string(colors_path()) {
            for line in txt.lines() {
                if let Some((k, v)) = line.split_once('=') {
                    if let Some(rgb) = hex(v) {
                        map.insert(k.trim().to_string(), rgb);
                    }
                }
            }
        }
        let get = |k: &str, def: &str| map.get(k).copied().unwrap_or_else(|| hex(def).unwrap());
        let bg = get("background", "#222222");
        let dark = map.get("dark_background").copied().unwrap_or(mix(bg, (0, 0, 0), 0.25));
        let darker = map.get("darker_background").copied().unwrap_or(mix(bg, (0, 0, 0), 0.45));
        let lighter = map.get("lighter_background").copied().unwrap_or(mix(bg, (255, 255, 255), 0.06));
        let fg = get("foreground", "#c2c2b0");
        let bright = map.get("bright_foreground").copied().unwrap_or(fg);
        let lightfg = map.get("light_foreground").copied().unwrap_or(mix(fg, bg, 0.3));
        let muted = map.get("muted").copied().unwrap_or(mix(fg, bg, 0.6));
        let sel = map.get("selection").copied().unwrap_or(mix(lighter, fg, 0.12));
        let accent = map.get("accent").copied().unwrap_or(get("blue", "#78824b"));
        let on_accent = if contrast(accent, darker) >= contrast(accent, bright) { darker } else { bright };
        let name = fs::read_to_string(state_dir().join("theme.name"))
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|_| "default".into());
        let hx = |v: Rgb| format!("{:02x}{:02x}/{:02x}{:02x}/{:02x}{:02x}", v.0, v.0, v.1, v.1, v.2, v.2);
        Theme {
            name,
            fg_hex: hx(fg),
            bg_hex: hx(bg),
            desk: c(darker),
            desk_rgb: darker,
            face: c(lighter),
            hilite: c(mix(lighter, fg, 0.35)),
            shadow: c(dark),
            client: c(bg),
            text: c(fg),
            dim: c(muted),
            accent: c(accent),
            on_accent: c(on_accent),
            inactive: c(sel),
            on_inactive: c(lightfg),
            button: c(mix(lighter, fg, 0.12)),
            tile_a: c(mix(lighter, fg, 0.28)),
            tile_b: c(mix(lighter, fg, 0.20)),
            red: c(get("red", "#685742")),
            yellow: c(get("yellow", "#b36d43")),
            green: c(get("green", "#5f875f")),
            cyan: c(get("cyan", "#c9a554")),
            blue: c(get("blue", "#78824b")),
            magenta: c(get("magenta", "#bb7744")),
            orange: c(get("orange", "#8d6242")),
        }
    }
}
