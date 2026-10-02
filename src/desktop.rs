// Where you've put the desktop icons, kept in ~/.config/win95-tui/desktop as
// "x y name" lines. No file yet means the usual icons in the usual places.
use crate::theme::home;
use std::{fs, path::PathBuf};

fn path() -> PathBuf {
    home().join(".config/win95-tui/desktop")
}

pub fn load() -> Option<Vec<(String, i32, i32)>> {
    let body = fs::read_to_string(path()).ok()?;
    Some(
        body.lines()
            .filter_map(|l| {
                let mut p = l.trim().splitn(3, ' ');
                let x = p.next()?.parse().ok()?;
                let y = p.next()?.parse().ok()?;
                let name = p.next()?.trim();
                (!name.is_empty()).then(|| (name.to_string(), x, y))
            })
            .collect(),
    )
}

pub fn save(icons: &[(String, i32, i32)]) {
    let _ = fs::create_dir_all(path().parent().unwrap());
    let body: String = icons.iter().map(|(n, x, y)| format!("{x} {y} {n}\n")).collect();
    let _ = fs::write(path(), body);
}
