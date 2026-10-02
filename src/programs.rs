// Programs you add yourself: the command that starts one, kept in
// ~/.config/win95-tui/programs as "name = command" lines, and the icon you
// painted for it in ~/.config/win95-tui/icons/<name>, six rows of letters.
use crate::theme::home;
use std::{fs, path::PathBuf};

pub fn path() -> PathBuf {
    home().join(".config/win95-tui/programs")
}

pub fn load() -> Vec<(String, String)> {
    fs::read_to_string(path())
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .filter_map(|l| l.split_once('='))
        .map(|(n, c)| (n.trim().to_string(), c.trim().to_string()))
        .filter(|(n, c)| !n.is_empty() && !c.is_empty())
        .collect()
}

fn save(list: &[(String, String)]) {
    let _ = fs::create_dir_all(path().parent().unwrap());
    let body: String = list.iter().map(|(n, c)| format!("{n} = {c}\n")).collect();
    let _ = fs::write(path(), body);
}

/// What a command is called on the menu: its program with a capital and
/// any "-tui" dropped, "Elinks" for "elinks https://...", "Scribe" for
/// "/usr/bin/scribe-tui".
pub fn name_of(cmd: &str) -> String {
    let prog = cmd.split_whitespace().next().unwrap_or("");
    let prog = prog.rsplit('/').next().unwrap_or(prog);
    let prog = prog.strip_suffix("-tui").filter(|p| !p.is_empty()).unwrap_or(prog);
    let mut c = prog.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

fn icon_path(name: &str) -> PathBuf {
    let safe: String = name.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    home().join(".config/win95-tui/icons").join(safe)
}

/// The icon painted for a program, if it has one.
pub fn icon(name: &str) -> Option<Vec<String>> {
    let rows: Vec<String> = fs::read_to_string(icon_path(name)).ok()?.lines().map(String::from).collect();
    rows.iter().any(|r| r.chars().any(|c| c != '.')).then_some(rows)
}

/// Adds a program, or changes the one with the same name. Returns its name.
pub fn add(cmd: &str, icon: Option<&[String]>) -> String {
    let name = name_of(cmd);
    let mut list = load();
    list.retain(|(n, _)| !n.eq_ignore_ascii_case(&name));
    list.push((name.clone(), cmd.to_string()));
    save(&list);
    let p = icon_path(&name);
    match icon {
        Some(rows) if rows.iter().any(|r| r.chars().any(|c| c != '.')) => {
            let _ = fs::create_dir_all(p.parent().unwrap());
            let _ = fs::write(&p, rows.join("\n") + "\n");
        }
        _ => {
            let _ = fs::remove_file(&p);
        }
    }
    name
}

pub fn remove(name: &str) {
    let mut list = load();
    list.retain(|(n, _)| n != name);
    save(&list);
    let _ = fs::remove_file(icon_path(name));
}

/// The program a command starts, if it isn't on the PATH.
pub fn missing(cmd: &str) -> Option<String> {
    let prog = cmd.split_whitespace().next()?;
    if prog.contains('/') {
        return (!crate::apps::notepad::expand(prog).exists()).then(|| prog.to_string());
    }
    let found = std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(prog).is_file()));
    (!found).then(|| prog.to_string())
}
