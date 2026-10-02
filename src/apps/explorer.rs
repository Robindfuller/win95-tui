use super::{Action, App, Launch};
use crate::{clip, draw::{st, Canvas}, icons::Icon, menu::{Cmd, Item, Sys}, theme::{home, Theme}};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::style::Modifier;
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc::{channel, Receiver},
    time::{Instant, SystemTime},
};

struct Entry {
    name: String,
    dir: bool,
    size: u64,
    modified: Option<SystemTime>,
}

pub struct Explorer {
    path: PathBuf,
    entries: Vec<Entry>,
    sel: usize,
    top: usize,
    hidden: bool,
    size: (u16, u16),
    last_click: Option<(Instant, usize)>,
    error: Option<String>,
    /// names picked with Space or Ctrl+click, for copying several at once
    marked: HashSet<String>,
    /// a line for the status bar, like "Copied 2 items"
    note: Option<String>,
    /// a paste still running: its errors arrive here when it's done
    job: Option<Receiver<Vec<String>>>,
    /// the folder's modified time when last read, to spot changes made elsewhere
    seen: Option<SystemTime>,
    checked: Instant,
    /// opened by another window's File > Open: the window, and whether it wants a folder
    pick: Option<(u64, bool)>,
    /// renaming the selected file: the name so far, the cursor, and whether
    /// the part before the extension is still selected, ready to type over
    rename: Option<(Vec<char>, usize, bool)>,
}

fn human(n: u64) -> String {
    let kb = n.div_ceil(1024);
    if kb < 10_000 { format!("{}KB", kb) } else { format!("{}MB", kb / 1024) }
}

fn items(n: usize) -> String {
    if n == 1 { "1 item".into() } else { format!("{n} items") }
}

fn uri(p: &Path) -> String {
    let mut s = String::from("file://");
    for b in p.as_os_str().as_encoded_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' => s.push(*b as char),
            _ => s.push_str(&format!("%{b:02X}")),
        }
    }
    s
}

fn from_uri(u: &str) -> Option<PathBuf> {
    let rest = u.strip_prefix("file://")?;
    let rest = &rest[rest.find('/')?..];
    let b = rest.as_bytes();
    let mut out = vec![];
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&rest[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    Some(PathBuf::from(String::from_utf8_lossy(&out).into_owned()))
}

/// Files on the system clipboard, and whether they were cut rather than
/// copied. Reads the format Nautilus uses, then a plain list of URIs or paths.
fn clip_files() -> Option<(bool, Vec<PathBuf>)> {
    if let Some(txt) = clip::wl_paste(Some("x-special/gnome-copied-files")) {
        let mut lines = txt.lines();
        let cut = lines.next()? == "cut";
        let v: Vec<PathBuf> = lines.filter_map(from_uri).collect();
        return (!v.is_empty()).then_some((cut, v));
    }
    let txt = clip::wl_paste(Some("text/uri-list")).or_else(clip::paste_text)?;
    paths_in(&txt).map(|v| (false, v))
}

/// Every line is a file:// URI or an absolute path that exists, or it's just text.
fn paths_in(txt: &str) -> Option<Vec<PathBuf>> {
    let mut v = vec![];
    for l in txt.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        let p = if l.starts_with("file://") { from_uri(l)? } else { PathBuf::from(l) };
        if !p.is_absolute() || !p.exists() {
            return None;
        }
        v.push(p);
    }
    (!v.is_empty()).then_some(v)
}

/// Put files on the system clipboard the way Nautilus does, so it can paste them.
fn set_clip(cut: bool, paths: &[PathBuf]) -> Result<(), String> {
    let mut body = String::from(if cut { "cut" } else { "copy" });
    for p in paths {
        body.push('\n');
        body.push_str(&uri(p));
    }
    clip::wl_copy(Some("x-special/gnome-copied-files"), &body)
}

/// `dir/name`, or `name (copy)`, `name (copy 2)` and so on if that's taken.
fn free_name(dir: &Path, name: &std::ffi::OsStr) -> PathBuf {
    let p = dir.join(name);
    if fs::symlink_metadata(&p).is_err() {
        return p;
    }
    let n = Path::new(name);
    let (stem, ext) = match (n.file_stem(), n.extension()) {
        (Some(s), Some(e)) => (s.to_string_lossy().to_string(), format!(".{}", e.to_string_lossy())),
        _ => (name.to_string_lossy().to_string(), String::new()),
    };
    (1..)
        .map(|i| dir.join(if i == 1 { format!("{stem} (copy){ext}") } else { format!("{stem} (copy {i}){ext}") }))
        .find(|p| fs::symlink_metadata(p).is_err())
        .unwrap()
}

/// Copy or move each file into `dir`, never over an existing one. Returns what went wrong.
fn paste_into(dir: &Path, cut: bool, paths: &[PathBuf]) -> Vec<String> {
    let mut errs = vec![];
    for src in paths {
        let Some(name) = src.file_name() else { continue };
        if cut && src.parent() == Some(dir) {
            continue;
        }
        if src.is_dir() && dir.starts_with(src) {
            errs.push(format!("{}: can't put a folder inside itself", name.to_string_lossy()));
            continue;
        }
        let to = free_name(dir, name);
        let out = if cut { Command::new("mv").arg("--").arg(src).arg(&to).output() } else { Command::new("cp").args(["-a", "--"]).arg(src).arg(&to).output() };
        match out {
            Ok(o) if o.status.success() => {}
            Ok(o) => errs.push(String::from_utf8_lossy(&o.stderr).trim().to_string()),
            Err(e) => errs.push(e.to_string()),
        }
    }
    errs
}

impl Explorer {
    pub fn new(path: PathBuf) -> Explorer {
        let mut e = Explorer {
            path,
            entries: vec![],
            sel: 0,
            top: 0,
            hidden: false,
            size: (60, 18),
            last_click: None,
            error: None,
            marked: HashSet::new(),
            note: None,
            job: None,
            seen: None,
            checked: Instant::now(),
            pick: None,
            rename: None,
        };
        e.load();
        e
    }

    /// A window for picking a file, or a folder, that goes back to window `to`.
    pub fn picker(path: PathBuf, to: u64, folder: bool) -> Explorer {
        let mut e = Explorer::new(home());
        e.pick = Some((to, folder));
        e.go(if path.is_dir() { path } else { home() });
        e
    }

    fn picking_folder(&self) -> bool {
        self.pick.is_some_and(|(_, f)| f)
    }

    /// Hand a path back to the window that asked, and close.
    fn hand_back(&self, p: PathBuf) -> Action {
        match self.pick {
            Some((to, _)) => Action::Many(vec![Action::Close, Action::Picked(to, p)]),
            None => Action::None,
        }
    }

    fn load(&mut self) {
        self.entries.clear();
        self.error = None;
        self.seen = fs::metadata(&self.path).and_then(|m| m.modified()).ok();
        if self.path.parent().is_some() {
            self.entries.push(Entry { name: "..".into(), dir: true, size: 0, modified: None });
        }
        match fs::read_dir(&self.path) {
            Ok(rd) => {
                let mut v: Vec<Entry> = rd
                    .flatten()
                    .filter_map(|d| {
                        let name = d.file_name().to_string_lossy().to_string();
                        if !self.hidden && name.starts_with('.') {
                            return None;
                        }
                        let md = fs::metadata(d.path()).ok();
                        // picking a folder shows only folders
                        if self.picking_folder() && !md.as_ref().is_some_and(|m| m.is_dir()) {
                            return None;
                        }
                        Some(Entry {
                            dir: md.as_ref().map(|m| m.is_dir()).unwrap_or(false),
                            size: md.as_ref().map(|m| m.len()).unwrap_or(0),
                            modified: md.and_then(|m| m.modified().ok()),
                            name,
                        })
                    })
                    .collect();
                v.sort_by(|a, b| b.dir.cmp(&a.dir).then(a.name.to_lowercase().cmp(&b.name.to_lowercase())));
                self.entries.extend(v);
            }
            Err(e) => self.error = Some(e.to_string()),
        }
        self.sel = self.sel.min(self.entries.len().saturating_sub(1));
        self.marked.retain(|n| self.entries.iter().any(|e| &e.name == n));
    }

    /// Read the folder again, keeping the same file selected.
    fn reload(&mut self) {
        let name = self.entries.get(self.sel).map(|e| e.name.clone());
        self.load();
        if let Some(i) = name.and_then(|n| self.entries.iter().position(|e| e.name == n)) {
            self.sel = i;
        }
        self.keep_visible();
    }

    /// What Copy and Cut work on: the marked files, or else the selected one.
    fn picked(&self) -> Vec<PathBuf> {
        if self.marked.is_empty() {
            self.entries.get(self.sel).filter(|e| e.name != "..").map(|e| vec![self.path.join(&e.name)]).unwrap_or_default()
        } else {
            self.entries.iter().filter(|e| self.marked.contains(&e.name)).map(|e| self.path.join(&e.name)).collect()
        }
    }

    fn copy(&mut self, cut: bool) -> Action {
        let v = self.picked();
        if v.is_empty() {
            return Action::None;
        }
        match set_clip(cut, &v) {
            Ok(()) => {
                self.note = Some(format!("{} {}", if cut { "Cut" } else { "Copied" }, items(v.len())));
                self.marked.clear();
                Action::None
            }
            Err(e) => Action::Launch(Launch::Msg { title: "Copy".into(), text: format!("Couldn't use the clipboard:\n{e}") }),
        }
    }

    /// Paste files from the clipboard into this folder. `text` is what the
    /// terminal pasted, used when the clipboard itself can't be read (over SSH).
    fn paste_files(&mut self, text: Option<&str>) -> Action {
        if self.job.is_some() {
            return Action::None;
        }
        let Some((cut, v)) = clip_files().or_else(|| text.and_then(paths_in).map(|v| (false, v))) else {
            return Action::Launch(Launch::Msg { title: "Paste".into(), text: "There are no files on the clipboard.".into() });
        };
        self.note = Some(format!("{} {}...", if cut { "Moving" } else { "Copying" }, items(v.len())));
        let (tx, rx) = channel();
        let dir = self.path.clone();
        std::thread::spawn(move || {
            let errs = paste_into(&dir, cut, &v);
            if cut && errs.is_empty() {
                let _ = Command::new("wl-copy").arg("--clear").stdout(Stdio::null()).stderr(Stdio::null()).status();
            }
            let _ = tx.send(errs);
        });
        self.job = Some(rx);
        Action::None
    }

    fn go(&mut self, p: PathBuf) {
        let from = self.path.file_name().map(|n| n.to_string_lossy().to_string());
        let up = self.path.parent() == Some(p.as_path());
        self.path = p;
        self.sel = 0;
        self.top = 0;
        self.marked.clear();
        self.load();
        if up {
            if let Some(n) = from {
                self.sel = self.entries.iter().position(|e| e.name == n).unwrap_or(0);
            }
        }
    }

    fn open(&mut self) -> Action {
        let Some(e) = self.entries.get(self.sel) else { return Action::None };
        if e.name == ".." {
            if let Some(p) = self.path.parent() {
                self.go(p.to_path_buf());
            }
            return Action::None;
        }
        let p = self.path.join(&e.name);
        if e.dir {
            self.go(p);
            Action::None
        } else if self.pick.is_some() {
            self.hand_back(p)
        } else {
            Action::Launch(crate::assoc::launch(&p))
        }
    }

    fn start_rename(&mut self) {
        if self.pick.is_some() {
            return;
        }
        if let Some(e) = self.entries.get(self.sel).filter(|e| e.name != "..") {
            let name: Vec<char> = e.name.chars().collect();
            // like Windows, the name without its extension is ready to type over
            let stem = match name.iter().rposition(|&c| c == '.') {
                Some(i) if i > 0 && !e.dir => i,
                _ => name.len(),
            };
            self.marked.clear();
            self.rename = Some((name, stem, true));
        }
    }

    /// Rename the selected file to what was typed. Returns a message if it can't.
    fn finish_rename(&mut self) -> Action {
        let Some((name, _, _)) = self.rename.take() else { return Action::None };
        let Some(old) = self.entries.get(self.sel).map(|e| e.name.clone()) else { return Action::None };
        let new: String = name.into_iter().collect::<String>().trim().to_string();
        if new.is_empty() || new == old {
            return Action::None;
        }
        let why = if new.contains('/') || new == "." || new == ".." {
            Some(format!("A name can't be {new:?} or have a / in it."))
        } else if fs::symlink_metadata(self.path.join(&new)).is_ok() && !new.eq_ignore_ascii_case(&old) {
            Some(format!("There's already something called {new} here."))
        } else {
            fs::rename(self.path.join(&old), self.path.join(&new)).err().map(|e| format!("Couldn't rename {old}:
{e}"))
        };
        if let Some(text) = why {
            return Action::Launch(Launch::Msg { title: "Rename".into(), text });
        }
        self.load();
        if let Some(i) = self.entries.iter().position(|e| e.name == new) {
            self.sel = i;
        }
        self.keep_visible();
        Action::None
    }

    /// Typing into the name being changed.
    fn rename_key(&mut self, k: KeyEvent) -> Action {
        let Some((name, cur, fresh)) = &mut self.rename else { return Action::None };
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        // typing or deleting while the old name is still selected replaces it
        let wipe = |name: &mut Vec<char>, cur: &mut usize| {
            name.drain(..*cur);
            *cur = 0;
        };
        match k.code {
            KeyCode::Enter => return self.finish_rename(),
            KeyCode::Esc => self.rename = None,
            KeyCode::Char(ch) if !ctrl => {
                if *fresh {
                    wipe(name, cur);
                }
                name.insert(*cur, ch);
                *cur += 1;
            }
            KeyCode::Backspace | KeyCode::Delete if *fresh => wipe(name, cur),
            KeyCode::Backspace if *cur > 0 => {
                *cur -= 1;
                name.remove(*cur);
            }
            KeyCode::Delete if *cur < name.len() => {
                name.remove(*cur);
            }
            KeyCode::Left => *cur = cur.saturating_sub(1),
            KeyCode::Right => *cur = (*cur + 1).min(name.len()),
            KeyCode::Home => *cur = 0,
            KeyCode::End => *cur = name.len(),
            _ => return Action::None,
        }
        if let Some((_, _, fresh)) = &mut self.rename {
            *fresh = false;
        }
        Action::None
    }

    /// Where the rename box goes: its x and width, and the first character shown.
    fn rename_box(&self) -> (i32, i32, usize) {
        let w = self.size.0 as i32;
        let bw = (if w > 40 { w - 26 } else { w - 1 }) - 2;
        let room = (bw - 2).max(1) as usize;
        let cur = self.rename.as_ref().map_or(0, |r| r.1);
        (2, bw, cur.saturating_sub(room))
    }

    fn list_rows(&self) -> usize {
        (self.size.1 as usize).saturating_sub(3).max(1)
    }

    fn keep_visible(&mut self) {
        let rows = self.list_rows();
        if self.sel < self.top {
            self.top = self.sel;
        }
        if self.sel >= self.top + rows {
            self.top = self.sel + 1 - rows;
        }
    }
}

impl App for Explorer {
    fn title(&self) -> String {
        if let Some((_, folder)) = self.pick {
            return if folder { "Open Folder".into() } else { "Open".into() };
        }
        let name = self.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or("/".into());
        if self.path == home() { "Home".into() } else if self.path == Path::new("/") { "Computer".into() } else { format!("Files - {}", name) }
    }
    fn icon(&self) -> Icon {
        if self.path == Path::new("/") { Icon::Computer } else { Icon::Folder }
    }
    fn tab_title(&self) -> String {
        if self.path == home() { "Home".into() } else { self.path.file_name().map_or("/".into(), |n| n.to_string_lossy().to_string()) }
    }
    fn size_hint(&self) -> (u16, u16) {
        (64, 18)
    }

    fn render(&mut self, c: &mut Canvas, th: &Theme, _focused: bool) {
        let (w, h) = (self.size.0 as i32, self.size.1 as i32);
        c.fill(0, 0, w, h, st(th.text, th.face));
        let x = c.text(1, 0, "Address", st(th.text, th.face));
        c.field(x + 1, 0, w - x - 2, &self.path.display().to_string(), th);
        let head = st(th.text, th.button);
        c.fill(0, 1, w, 1, head);
        let size_x = w - 25;
        let date_x = w - 16;
        c.text(2, 1, "Name", head);
        if w > 40 {
            c.text(size_x, 1, "   Size", head);
            c.text(date_x, 1, "Modified", head);
        }
        let body = st(th.text, th.client);
        c.fill(0, 2, w, h - 3, body);
        if let Some(err) = &self.error {
            c.text_max(2, 2, err, st(th.red, th.client), w);
        }
        for (row, (i, e)) in self.entries.iter().enumerate().skip(self.top).take(self.list_rows()).enumerate() {
            let y = 2 + row as i32;
            let on = i == self.sel || self.marked.contains(&e.name);
            let s = if on { th.sel() } else { body };
            if on {
                c.fill(0, y, w, 1, s);
            }
            if i == self.sel && !self.marked.is_empty() {
                c.text(0, y, ">", s);
            }
            let (g, col) = if e.dir { Icon::Folder.glyph(th) } else { Icon::File.glyph(th) };
            c.put_c(1, y, g, if on { s } else { st(col, th.client) });
            let max = if w > 40 { size_x - 1 } else { w };
            c.text_max(3, y, &e.name, if e.dir { s.add_modifier(Modifier::BOLD) } else { s }, max);
            if i == self.sel {
                if let Some((name, cur, fresh)) = &self.rename {
                    let (bx, bw, start) = self.rename_box();
                    c.field(bx, y, bw, "", th);
                    let field = st(th.text, th.client);
                    for (k, ch) in name.iter().enumerate().skip(start).take((bw - 2).max(0) as usize) {
                        c.put_c(bx + 1 + (k - start) as i32, y, *ch, if *fresh && k < *cur { th.sel() } else { field });
                    }
                }
            }
            if w > 40 && e.name != ".." {
                if !e.dir {
                    c.text(size_x, y, &format!("{:>7}", human(e.size)), s);
                }
                if let Some(m) = e.modified {
                    let dt: chrono::DateTime<chrono::Local> = m.into();
                    c.text(date_x, y, &dt.format("%d/%m/%y %H:%M").to_string(), s);
                }
            }
        }
        let n = self.entries.iter().filter(|e| e.name != "..").count();
        let free = st(th.text, th.face);
        c.fill(0, h - 1, w, 1, free);
        let status = match (&self.note, self.marked.len()) {
            (Some(t), _) => t.clone(),
            (None, 0) => format!("{} object(s)", n),
            (None, m) => format!("{} of {} selected", m, n),
        };
        if self.picking_folder() {
            c.text_max(1, h - 1, &status, free, w - 23);
            c.button(w - 22, h - 1, 21, "Open This Folder", th, true, false);
            return;
        }
        c.text_max(1, h - 1, &status, free, w - 11);
        if let Some(e) = self.entries.get(self.sel).filter(|e| !e.dir) {
            c.text(w - 10, h - 1, &format!("{:>8}", human(e.size)), free);
        }
    }

    fn cursor(&self) -> Option<(u16, u16)> {
        let (_, cur, fresh) = self.rename.as_ref()?;
        let row = self.sel.checked_sub(self.top).filter(|r| *r < self.list_rows())?;
        let (bx, _, start) = self.rename_box();
        (!fresh).then_some(((bx + 1) as u16 + (cur - start) as u16, 2 + row as u16))
    }

    fn key(&mut self, k: KeyEvent) -> Action {
        if self.rename.is_some() {
            return self.rename_key(k);
        }
        let n = self.entries.len();
        let rows = self.list_rows();
        self.note = None;
        let (ctrl, shift) = (k.modifiers.contains(KeyModifiers::CONTROL), k.modifiers.contains(KeyModifiers::SHIFT));
        match k.code {
            KeyCode::Char('o') if ctrl && self.picking_folder() => return self.command("pick-here"),
            KeyCode::Esc if self.pick.is_some() && self.marked.is_empty() => return Action::Close,
            KeyCode::F(2) => self.start_rename(),
            KeyCode::Char('c') | KeyCode::Insert if ctrl => return self.copy(false),
            KeyCode::Char('x') if ctrl => return self.copy(true),
            KeyCode::Char('v') if ctrl => return self.paste_files(None),
            KeyCode::Insert if shift => return self.paste_files(None),
            KeyCode::Char('a') if ctrl => return self.command("all"),
            KeyCode::Char(_) if ctrl => {}
            KeyCode::Char(' ') => {
                if let Some(e) = self.entries.get(self.sel).filter(|e| e.name != "..") {
                    if !self.marked.remove(&e.name) {
                        self.marked.insert(e.name.clone());
                    }
                }
                self.sel = (self.sel + 1).min(n.saturating_sub(1));
            }
            KeyCode::Esc => self.marked.clear(),
            KeyCode::Up => self.sel = self.sel.saturating_sub(1),
            KeyCode::Down => self.sel = (self.sel + 1).min(n.saturating_sub(1)),
            KeyCode::PageUp => self.sel = self.sel.saturating_sub(rows),
            KeyCode::PageDown => self.sel = (self.sel + rows).min(n.saturating_sub(1)),
            KeyCode::Home => self.sel = 0,
            KeyCode::End => self.sel = n.saturating_sub(1),
            KeyCode::Enter => return self.open(),
            KeyCode::Backspace => return self.command("up"),
            KeyCode::Char('.') => return self.command("hidden"),
            KeyCode::Char(ch) => {
                let ch = ch.to_ascii_lowercase();
                if let Some(i) = self.entries.iter().enumerate().skip(self.sel + 1).chain(self.entries.iter().enumerate()).find(|(_, e)| e.name.to_lowercase().starts_with(ch)).map(|(i, _)| i) {
                    self.sel = i;
                }
            }
            _ => {}
        }
        self.keep_visible();
        Action::None
    }

    fn mouse(&mut self, kind: MouseEventKind, x: i32, y: i32, mods: KeyModifiers) -> Action {
        // clicking anywhere else keeps the name typed, like Windows
        if matches!(kind, MouseEventKind::Down(_)) && self.rename.is_some() {
            let on_box = y == 2 + self.sel as i32 - self.top as i32 && matches!(kind, MouseEventKind::Down(MouseButton::Left));
            if on_box {
                return Action::None;
            }
            let a = self.finish_rename();
            if !matches!(a, Action::None) {
                return a;
            }
        }
        match kind {
            MouseEventKind::Down(MouseButton::Left) if self.picking_folder() && y == self.size.1 as i32 - 1 && x >= self.size.0 as i32 - 22 => {
                return self.command("pick-here");
            }
            MouseEventKind::Down(MouseButton::Left) => {
                self.note = None;
                if y >= 2 && y < 2 + self.list_rows() as i32 {
                    let i = self.top + (y - 2) as usize;
                    if mods.contains(KeyModifiers::CONTROL) && i < self.entries.len() && self.entries[i].name != ".." {
                        let name = self.entries[i].name.clone();
                        if !self.marked.remove(&name) {
                            self.marked.insert(name);
                        }
                        self.sel = i;
                        return Action::None;
                    }
                    if i < self.entries.len() {
                        self.marked.clear();
                        let double = self.last_click.is_some_and(|(t, j)| j == i && t.elapsed().as_millis() < 450);
                        self.sel = i;
                        self.last_click = Some((Instant::now(), i));
                        if double {
                            self.last_click = None;
                            return self.open();
                        }
                    }
                }
            }
            MouseEventKind::Down(MouseButton::Right) => {
                self.note = None;
                let i = self.top + (y - 2).max(0) as usize;
                let on = y >= 2 && y < 2 + self.list_rows() as i32 && i < self.entries.len() && self.entries[i].name != "..";
                if on {
                    if !self.marked.contains(&self.entries[i].name) {
                        self.marked.clear();
                    }
                    self.sel = i;
                    let dir = self.entries[i].dir;
                    return Action::Menu(
                        vec![
                            Item::new("Open", Cmd::App("open")),
                            Item::new("Open in New Tab", Cmd::App("opentab")).enabled(dir),
                            Item::new("Create Shortcut", Cmd::App("shortcut")),
                            Item::sep(),
                            Item::new("Cut", Cmd::App("cut")).key("Ctrl+X"),
                            Item::new("Copy", Cmd::App("copy")).key("Ctrl+C"),
                            Item::sep(),
                            Item::new("Rename", Cmd::App("rename")).key("F2").enabled(self.pick.is_none()),
                        ],
                        x,
                        y,
                    );
                }
                return Action::Menu(
                    vec![
                        Item::new("Paste", Cmd::App("paste")).key("Ctrl+V"),
                        Item::sep(),
                        Item::new("New Text Document", Cmd::App("newtxt")).icon(Icon::Notepad),
                        Item::new("New Bitmap Image", Cmd::App("newbmp")).icon(Icon::Picture),
                        Item::new("Terminal Here", Cmd::App("term")).icon(Icon::Terminal),
                        Item::sep(),
                        Item::new("Refresh", Cmd::App("refresh")),
                    ],
                    x,
                    y,
                );
            }
            MouseEventKind::ScrollUp => self.top = self.top.saturating_sub(3),
            MouseEventKind::ScrollDown => self.top = (self.top + 3).min(self.entries.len().saturating_sub(self.list_rows())),
            _ => {}
        }
        Action::None
    }

    fn paste(&mut self, s: &str) {
        if let Some((name, cur, fresh)) = &mut self.rename {
            if *fresh {
                name.drain(..*cur);
                *cur = 0;
                *fresh = false;
            }
            for ch in s.lines().next().unwrap_or("").chars() {
                name.insert(*cur, ch);
                *cur += 1;
            }
            return;
        }
        if let Action::Launch(Launch::Msg { text, .. }) = self.paste_files(Some(s)) {
            self.note = Some(text);
        }
    }

    fn resize(&mut self, w: u16, h: u16) {
        self.size = (w, h);
        self.keep_visible();
    }

    fn poll(&mut self) -> (bool, Action) {
        if let Some(rx) = &self.job {
            if let Ok(errs) = rx.try_recv() {
                self.job = None;
                self.note = None;
                self.reload();
                if !errs.is_empty() {
                    return (true, Action::Launch(Launch::Msg { title: "Paste".into(), text: errs.join("\n") }));
                }
                return (true, Action::None);
            }
        }
        // pick up changes made in other windows or other programs
        if self.checked.elapsed().as_millis() >= 1000 {
            self.checked = Instant::now();
            if fs::metadata(&self.path).and_then(|m| m.modified()).ok() != self.seen {
                self.reload();
                return (true, Action::None);
            }
        }
        (false, Action::None)
    }

    fn menubar(&self) -> Vec<(&'static str, Vec<Item>)> {
        if let Some((_, folder)) = self.pick {
            let mut file = vec![Item::new("Open", Cmd::App("open")).key("Enter")];
            if folder {
                file.push(Item::new("Open This Folder", Cmd::App("pick-here")).key("Ctrl+O"));
            }
            file.extend([Item::sep(), Item::new("Cancel", Cmd::Sys(Sys::Close)).key("Esc")]);
            return vec![
                ("File", file),
                ("View", vec![Item::new("Hidden Files", Cmd::App("hidden")).checked(self.hidden).key(".")]),
                ("Go", vec![
                    Item::new("Up One Level", Cmd::App("up")).key("Bksp"),
                    Item::new("Home", Cmd::App("home")).icon(Icon::Folder),
                    Item::new("Computer", Cmd::App("root")).icon(Icon::Computer),
                ]),
            ];
        }
        vec![
            ("File", vec![
                Item::new("Open", Cmd::App("open")).key("Enter"),
                Item::new("Rename", Cmd::App("rename")).key("F2"),
                Item::new("Terminal Here", Cmd::App("term")).icon(Icon::Terminal),
                Item::new("New Text Document", Cmd::App("newtxt")).icon(Icon::Notepad),
                Item::new("New Bitmap Image", Cmd::App("newbmp")).icon(Icon::Picture),
                Item::new("Create Shortcut", Cmd::App("shortcut")),
                Item::sep(),
                Item::new("Close", Cmd::Sys(Sys::Close)),
            ]),
            ("Edit", vec![
                Item::new("Cut", Cmd::App("cut")).key("Ctrl+X"),
                Item::new("Copy", Cmd::App("copy")).key("Ctrl+C"),
                Item::new("Paste", Cmd::App("paste")).key("Ctrl+V"),
                Item::sep(),
                Item::new("Select All", Cmd::App("all")).key("Ctrl+A"),
            ]),
            ("View", vec![
                Item::new("Hidden Files", Cmd::App("hidden")).checked(self.hidden).key("."),
                Item::new("Refresh", Cmd::App("refresh")),
            ]),
            ("Go", vec![
                Item::new("Up One Level", Cmd::App("up")).key("Bksp"),
                Item::new("Home", Cmd::App("home")).icon(Icon::Folder),
                Item::new("Computer", Cmd::App("root")).icon(Icon::Computer),
            ]),
        ]
    }

    fn new_tab(&self) -> Option<Launch> {
        self.pick.is_none().then(|| Launch::Explorer(self.path.clone()))
    }

    fn command(&mut self, cmd: &str) -> Action {
        match cmd {
            "open" => return self.open(),
            "rename" => self.start_rename(),
            "pick-here" => return self.hand_back(self.path.clone()),
            "opentab" => {
                if let Some(e) = self.entries.get(self.sel).filter(|e| e.dir && e.name != "..") {
                    return Action::OpenTab(Launch::Explorer(self.path.join(&e.name)));
                }
            }
            "term" => return Action::Launch(Launch::Shell { cmd: None, cwd: Some(self.path.clone()), title: "Terminal".into(), icon: Icon::Terminal, keep_open: false }),
            "newtxt" => {
                let mut i = 1;
                let mut p = self.path.join("New Text Document.txt");
                while p.exists() {
                    i += 1;
                    p = self.path.join(format!("New Text Document ({i}).txt"));
                }
                return Action::Launch(Launch::Notepad(Some(p)));
            }
            "newbmp" => {
                let mut i = 1;
                let mut p = self.path.join("New Bitmap Image.bmp");
                while p.exists() {
                    i += 1;
                    p = self.path.join(format!("New Bitmap Image ({i}).bmp"));
                }
                return Action::Launch(Launch::Paint(Some(p)));
            }
            "shortcut" => {
                let names: Vec<String> = if self.marked.is_empty() {
                    self.entries.get(self.sel).filter(|e| e.name != "..").map(|e| vec![e.name.clone()]).unwrap_or_default()
                } else {
                    self.marked.iter().cloned().collect()
                };
                if !names.is_empty() {
                    self.note = Some(if names.len() == 1 { "Shortcut on the desktop".into() } else { format!("{} shortcuts on the desktop", names.len()) });
                    return Action::Shortcut(names.iter().map(|n| self.path.join(n)).collect());
                }
            }
            "hidden" => {
                self.hidden = !self.hidden;
                self.load();
            }
            "cut" => return self.copy(true),
            "copy" => return self.copy(false),
            "paste" => return self.paste_files(None),
            "all" => self.marked = self.entries.iter().filter(|e| e.name != "..").map(|e| e.name.clone()).collect(),
            "refresh" => self.reload(),
            "up" => {
                if let Some(p) = self.path.parent() {
                    self.go(p.to_path_buf());
                }
            }
            "home" => self.go(home()),
            "root" => self.go(PathBuf::from("/")),
            _ => {}
        }
        Action::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uris_round_trip() {
        let p = PathBuf::from("/home/me/My Files/50% off & ünïcode.txt");
        assert_eq!(uri(&p), "file:///home/me/My%20Files/50%25%20off%20%26%20%C3%BCn%C3%AFcode.txt");
        assert_eq!(from_uri(&uri(&p)), Some(p));
        assert_eq!(from_uri("file://localhost/tmp/a"), Some(PathBuf::from("/tmp/a")));
    }

    #[test]
    fn f2_renames() {
        let root = std::env::temp_dir().join(format!("win95-rename-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("notes.txt"), "hi").unwrap();
        fs::write(root.join("taken.txt"), "x").unwrap();
        let mut e = Explorer::new(root.clone());
        let key = |e: &mut Explorer, c: KeyCode| e.key(KeyEvent::new(c, KeyModifiers::NONE));
        e.sel = e.entries.iter().position(|x| x.name == "notes.txt").unwrap();
        // typing replaces the name but keeps .txt
        key(&mut e, KeyCode::F(2));
        for ch in "plan".chars() {
            key(&mut e, KeyCode::Char(ch));
        }
        key(&mut e, KeyCode::Enter);
        assert!(root.join("plan.txt").exists() && !root.join("notes.txt").exists());
        assert_eq!(e.entries[e.sel].name, "plan.txt");
        // Esc leaves it be
        key(&mut e, KeyCode::F(2));
        key(&mut e, KeyCode::Char('z'));
        key(&mut e, KeyCode::Esc);
        assert!(root.join("plan.txt").exists());
        // never over another file
        key(&mut e, KeyCode::F(2));
        key(&mut e, KeyCode::End);
        for _ in 0..8 {
            key(&mut e, KeyCode::Backspace);
        }
        for ch in "taken.txt".chars() {
            key(&mut e, KeyCode::Char(ch));
        }
        assert!(matches!(key(&mut e, KeyCode::Enter), Action::Launch(Launch::Msg { .. })));
        assert_eq!(fs::read_to_string(root.join("taken.txt")).unwrap(), "x");
        assert!(root.join("plan.txt").exists());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn paste_never_overwrites() {
        let root = std::env::temp_dir().join(format!("win95-paste-{}", std::process::id()));
        let (a, b) = (root.join("a"), root.join("b"));
        fs::create_dir_all(a.join("folder")).unwrap();
        fs::create_dir_all(&b).unwrap();
        fs::write(a.join("note.txt"), "hi").unwrap();
        fs::write(a.join("folder/inner"), "x").unwrap();

        let src = [a.join("note.txt"), a.join("folder")];
        assert!(paste_into(&b, false, &src).is_empty());
        assert!(paste_into(&b, false, &src).is_empty());
        assert!(b.join("note.txt").exists() && b.join("note (copy).txt").exists());
        assert!(b.join("folder/inner").exists() && b.join("folder (copy)/inner").exists());

        // copying into the same folder makes a copy, cutting there does nothing
        assert!(paste_into(&a, false, &[a.join("note.txt")]).is_empty());
        assert!(a.join("note (copy).txt").exists());
        assert!(paste_into(&a, true, &[a.join("note.txt")]).is_empty());
        assert!(a.join("note.txt").exists());

        assert_eq!(paste_into(&a.join("folder"), false, &[a.join("folder")]).len(), 1);

        assert!(paste_into(&b, true, &[a.join("note.txt")]).is_empty());
        assert!(!a.join("note.txt").exists() && b.join("note (copy 2).txt").exists());
        fs::remove_dir_all(&root).unwrap();
    }
}
