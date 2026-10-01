// Programs you add yourself: a name and the command that starts it, kept in
// ~/.config/win95-tui/programs as "Name = command" lines, one per program.
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

/// Adds a program, or changes the command of one with the same name.
pub fn add(name: &str, cmd: &str) {
    let mut list = load();
    list.retain(|(n, _)| !n.eq_ignore_ascii_case(name));
    list.push((name.to_string(), cmd.to_string()));
    save(&list);
}

pub fn remove(name: &str) {
    let mut list = load();
    list.retain(|(n, _)| n != name);
    save(&list);
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
