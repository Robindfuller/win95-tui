use super::{Action, App, Launch};
use crate::{draw::{st, Canvas}, icons::Icon, menu::{Cmd, Item, Sys}, theme::{home, Theme}};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::style::Modifier;
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
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
}

fn human(n: u64) -> String {
    let kb = n.div_ceil(1024);
    if kb < 10_000 { format!("{}KB", kb) } else { format!("{}MB", kb / 1024) }
}

fn looks_text(p: &Path) -> bool {
    let mut buf = [0u8; 4096];
    match fs::File::open(p).and_then(|mut f| f.read(&mut buf)) {
        Ok(n) => !buf[..n].contains(&0) && fs::metadata(p).map(|m| m.len() < 4_000_000).unwrap_or(false),
        Err(_) => false,
    }
}

impl Explorer {
    pub fn new(path: PathBuf) -> Explorer {
        let mut e = Explorer { path, entries: vec![], sel: 0, top: 0, hidden: false, size: (60, 18), last_click: None, error: None };
        e.load();
        e
    }

    fn load(&mut self) {
        self.entries.clear();
        self.error = None;
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
    }

    fn go(&mut self, p: PathBuf) {
        let from = self.path.file_name().map(|n| n.to_string_lossy().to_string());
        let up = self.path.parent() == Some(p.as_path());
        self.path = p;
        self.sel = 0;
        self.top = 0;
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
        } else if looks_text(&p) {
            Action::Launch(Launch::Notepad(Some(p)))
        } else {
            let _ = std::process::Command::new("xdg-open")
                .arg(&p)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
            Action::None
        }
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
        let name = self.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or("/".into());
        if self.path == home() { "Home".into() } else if self.path == Path::new("/") { "Computer".into() } else { format!("Files - {}", name) }
    }
    fn icon(&self) -> Icon {
        if self.path == Path::new("/") { Icon::Computer } else { Icon::Folder }
    }
    fn size_hint(&self) -> (u16, u16) {
        (64, 18)
    }

    fn render(&mut self, c: &mut Canvas, th: &Theme, _focused: bool) {
        let (w, h) = (self.size.0 as i32, self.size.1 as i32);
        c.fill(0, 0, w, h, st(th.text, th.face));
        let x = c.text(1, 0, "Address", st(th.text, th.face));
        c.field(x + 1, 0, w - x - 2, &self.path.display().to_string(), th);
        let head = if th.lite { st(th.dim, th.button).add_modifier(Modifier::BOLD) } else { st(th.text, th.button) };
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
            let on = i == self.sel;
            let s = if on { th.sel() } else { body };
            if on {
                c.fill(0, y, w, 1, s);
            }
            let (g, col) = if e.dir { Icon::Folder.glyph(th) } else { Icon::File.glyph(th) };
            c.put_c(1, y, g, if on { s } else { st(col, th.client) });
            let max = if w > 40 { size_x - 1 } else { w };
            c.text_max(3, y, &e.name, if e.dir { s.add_modifier(Modifier::BOLD) } else { s }, max);
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
        c.text(1, h - 1, &format!("{} object(s)", n), free);
        if let Some(e) = self.entries.get(self.sel).filter(|e| !e.dir) {
            c.text(w - 10, h - 1, &format!("{:>8}", human(e.size)), free);
        }
    }

    fn key(&mut self, k: KeyEvent) -> Action {
        let n = self.entries.len();
        let rows = self.list_rows();
        match k.code {
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

    fn mouse(&mut self, kind: MouseEventKind, _x: i32, y: i32, _mods: KeyModifiers) -> Action {
        match kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if y >= 2 && y < 2 + self.list_rows() as i32 {
                    let i = self.top + (y - 2) as usize;
                    if i < self.entries.len() {
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
            MouseEventKind::ScrollUp => self.top = self.top.saturating_sub(3),
            MouseEventKind::ScrollDown => self.top = (self.top + 3).min(self.entries.len().saturating_sub(self.list_rows())),
            _ => {}
        }
        Action::None
    }

    fn resize(&mut self, w: u16, h: u16) {
        self.size = (w, h);
        self.keep_visible();
    }

    fn menubar(&self) -> Vec<(&'static str, Vec<Item>)> {
        vec![
            ("File", vec![
                Item::new("Open", Cmd::App("open")).key("Enter"),
                Item::new("Terminal Here", Cmd::App("term")).icon(Icon::Terminal),
                Item::new("New Text Document", Cmd::App("newtxt")).icon(Icon::Notepad),
                Item::sep(),
                Item::new("Close", Cmd::Sys(Sys::Close)),
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

    fn command(&mut self, cmd: &str) -> Action {
        match cmd {
            "open" => return self.open(),
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
            "hidden" => {
                self.hidden = !self.hidden;
                self.load();
            }
            "refresh" => self.load(),
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
