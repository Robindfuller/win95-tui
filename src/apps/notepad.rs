use super::{Action, App, Launch};
use crate::{draw::{st, Canvas}, icons::Icon, menu::{Cmd, Item, Sys}, theme::{home, Theme}};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind};
use std::{fs, path::PathBuf};

pub struct Notepad {
    lines: Vec<Vec<char>>,
    cx: usize,
    cy: usize,
    top: usize,
    left: usize,
    path: Option<PathBuf>,
    modified: bool,
    size: (u16, u16),
    prompt: Option<String>,
    status: Option<String>,
}

pub fn expand(p: &str) -> PathBuf {
    match p.strip_prefix("~/") {
        Some(rest) => home().join(rest),
        None if p == "~" => home(),
        None => PathBuf::from(p),
    }
}

impl Notepad {
    pub fn new(path: Option<PathBuf>) -> Notepad {
        let mut lines = vec![vec![]];
        let mut status = None;
        if let Some(p) = &path {
            match fs::read(p) {
                Ok(bytes) => {
                    let txt = String::from_utf8_lossy(&bytes);
                    lines = txt.split('\n').map(|l| l.trim_end_matches('\r').chars().collect()).collect();
                    if lines.len() > 1 && lines.last().is_some_and(|l| l.is_empty()) {
                        lines.pop();
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => status = Some("New file".into()),
                Err(e) => status = Some(format!("Cannot open: {e}")),
            }
        }
        Notepad { lines, cx: 0, cy: 0, top: 0, left: 0, path, modified: false, size: (60, 18), prompt: None, status }
    }

    fn text_rows(&self) -> usize {
        (self.size.1 as usize).saturating_sub(if self.prompt.is_some() || self.status.is_some() { 1 } else { 0 }).max(1)
    }

    fn clamp(&mut self) {
        self.cy = self.cy.min(self.lines.len() - 1);
        self.cx = self.cx.min(self.lines[self.cy].len());
        let rows = self.text_rows();
        if self.cy < self.top {
            self.top = self.cy;
        }
        if self.cy >= self.top + rows {
            self.top = self.cy + 1 - rows;
        }
        let w = self.size.0 as usize;
        if self.cx < self.left {
            self.left = self.cx;
        }
        if self.cx >= self.left + w {
            self.left = self.cx + 1 - w;
        }
    }

    fn insert(&mut self, ch: char) {
        if ch == '\n' {
            let rest = self.lines[self.cy].split_off(self.cx);
            self.lines.insert(self.cy + 1, rest);
            self.cy += 1;
            self.cx = 0;
        } else {
            self.lines[self.cy].insert(self.cx, ch);
            self.cx += 1;
        }
        self.modified = true;
    }

    fn save(&mut self) -> Action {
        let Some(p) = self.path.clone() else {
            self.prompt = Some("~/untitled.txt".into());
            return Action::None;
        };
        let mut out: String = self.lines.iter().map(|l| l.iter().collect::<String>()).collect::<Vec<_>>().join("\n");
        out.push('\n');
        self.status = Some(match fs::write(&p, out) {
            Ok(_) => {
                self.modified = false;
                format!("Saved {}", p.display())
            }
            Err(e) => format!("Cannot save: {e}"),
        });
        Action::None
    }
}

impl App for Notepad {
    fn title(&self) -> String {
        let name = self.path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or("Untitled".into());
        format!("{}{} - Notes", name, if self.modified { "*" } else { "" })
    }
    fn icon(&self) -> Icon {
        Icon::Notepad
    }
    fn size_hint(&self) -> (u16, u16) {
        (64, 18)
    }

    fn render(&mut self, c: &mut Canvas, th: &Theme, _focused: bool) {
        let s = st(th.text, th.client);
        c.fill(0, 0, self.size.0 as i32, self.size.1 as i32, s);
        let rows = self.text_rows();
        for (i, line) in self.lines.iter().skip(self.top).take(rows).enumerate() {
            let shown: String = line.iter().skip(self.left).take(self.size.0 as usize).map(|&ch| if ch == '\t' { ' ' } else { ch }).collect();
            c.text_max(0, i as i32, &shown, s, self.size.0 as i32);
        }
        let y = self.size.1 as i32 - 1;
        if let Some(p) = &self.prompt {
            c.fill(0, y, self.size.0 as i32, 1, st(th.text, th.face));
            let x = c.text(1, y, "Save as:", st(th.text, th.face));
            c.field(x + 1, y, self.size.0 as i32 - x - 2, p, th);
        } else if let Some(m) = &self.status {
            c.fill(0, y, self.size.0 as i32, 1, st(th.text, th.face));
            c.text_max(1, y, m, st(th.text, th.face), self.size.0 as i32);
        }
    }

    fn cursor(&self) -> Option<(u16, u16)> {
        if let Some(p) = &self.prompt {
            let x = 11 + p.chars().count().min(self.size.0 as usize - 13);
            return Some((x as u16, self.size.1 - 1));
        }
        let (x, y) = (self.cx.checked_sub(self.left)?, self.cy.checked_sub(self.top)?);
        (x < self.size.0 as usize && y < self.text_rows()).then_some((x as u16, y as u16))
    }

    fn key(&mut self, k: KeyEvent) -> Action {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        if let Some(p) = &mut self.prompt {
            match k.code {
                KeyCode::Enter => {
                    self.path = Some(expand(p.trim()));
                    self.prompt = None;
                    return self.save();
                }
                KeyCode::Esc => self.prompt = None,
                KeyCode::Backspace => {
                    p.pop();
                }
                KeyCode::Char(ch) if !ctrl => p.push(ch),
                _ => {}
            }
            return Action::None;
        }
        self.status = None;
        let rows = self.text_rows();
        match k.code {
            KeyCode::Char('s') if ctrl => return self.save(),
            KeyCode::Char('n') if ctrl => return Action::Launch(Launch::Notepad(None)),
            KeyCode::F(5) => {
                for ch in chrono::Local::now().format("%H:%M %d/%m/%Y").to_string().chars() {
                    self.insert(ch);
                }
            }
            KeyCode::Char(ch) if !ctrl => self.insert(ch),
            KeyCode::Tab => self.insert('\t'),
            KeyCode::Enter => self.insert('\n'),
            KeyCode::Backspace => {
                if self.cx > 0 {
                    self.cx -= 1;
                    self.lines[self.cy].remove(self.cx);
                    self.modified = true;
                } else if self.cy > 0 {
                    let line = self.lines.remove(self.cy);
                    self.cy -= 1;
                    self.cx = self.lines[self.cy].len();
                    self.lines[self.cy].extend(line);
                    self.modified = true;
                }
            }
            KeyCode::Delete => {
                if self.cx < self.lines[self.cy].len() {
                    self.lines[self.cy].remove(self.cx);
                    self.modified = true;
                } else if self.cy + 1 < self.lines.len() {
                    let line = self.lines.remove(self.cy + 1);
                    self.lines[self.cy].extend(line);
                    self.modified = true;
                }
            }
            KeyCode::Left => {
                if self.cx > 0 {
                    self.cx -= 1;
                } else if self.cy > 0 {
                    self.cy -= 1;
                    self.cx = self.lines[self.cy].len();
                }
            }
            KeyCode::Right => {
                if self.cx < self.lines[self.cy].len() {
                    self.cx += 1;
                } else if self.cy + 1 < self.lines.len() {
                    self.cy += 1;
                    self.cx = 0;
                }
            }
            KeyCode::Up => self.cy = self.cy.saturating_sub(1),
            KeyCode::Down => self.cy += 1,
            KeyCode::Home => self.cx = 0,
            KeyCode::End => self.cx = usize::MAX,
            KeyCode::PageUp => self.cy = self.cy.saturating_sub(rows),
            KeyCode::PageDown => self.cy += rows,
            _ => {}
        }
        self.clamp();
        Action::None
    }

    fn mouse(&mut self, kind: MouseEventKind, x: i32, y: i32, _mods: KeyModifiers) -> Action {
        match kind {
            MouseEventKind::Down(MouseButton::Left) | MouseEventKind::Drag(MouseButton::Left) if self.prompt.is_none() => {
                if y >= 0 && (y as usize) < self.text_rows() {
                    self.cy = self.top + y as usize;
                    self.cx = self.left + x.max(0) as usize;
                    self.clamp();
                }
            }
            MouseEventKind::ScrollUp => {
                self.top = self.top.saturating_sub(3);
                self.cy = self.cy.min(self.top + self.text_rows() - 1);
                self.clamp();
            }
            MouseEventKind::ScrollDown => {
                self.top = (self.top + 3).min(self.lines.len().saturating_sub(1));
                self.cy = self.cy.max(self.top);
                self.clamp();
            }
            _ => {}
        }
        Action::None
    }

    fn paste(&mut self, s: &str) {
        for ch in s.replace("\r\n", "\n").chars() {
            self.insert(ch);
        }
        self.clamp();
    }

    fn resize(&mut self, w: u16, h: u16) {
        self.size = (w.max(1), h.max(1));
        self.clamp();
    }

    fn menubar(&self) -> Vec<(&'static str, Vec<Item>)> {
        vec![
            ("File", vec![
                Item::new("New", Cmd::App("new")).key("Ctrl+N"),
                Item::new("Open...", Cmd::App("open")),
                Item::new("Save", Cmd::App("save")).key("Ctrl+S"),
                Item::new("Save As...", Cmd::App("saveas")),
                Item::sep(),
                Item::new("Exit", Cmd::Sys(Sys::Close)),
            ]),
            ("Edit", vec![Item::new("Time/Date", Cmd::App("date")).key("F5")]),
            ("Help", vec![Item::new("About Notes", Cmd::App("about")).icon(Icon::Info)]),
        ]
    }

    fn command(&mut self, cmd: &str) -> Action {
        match cmd {
            "new" => Action::Launch(Launch::Notepad(None)),
            "open" => {
                let dir = self.path.as_ref().and_then(|p| p.parent().map(|d| d.to_path_buf())).unwrap_or_else(home);
                Action::Launch(Launch::Explorer(dir))
            }
            "save" => self.save(),
            "saveas" => {
                self.prompt = Some(self.path.as_ref().map(|p| p.display().to_string()).unwrap_or("~/untitled.txt".into()));
                Action::None
            }
            "date" => self.key(KeyEvent::new(KeyCode::F(5), KeyModifiers::NONE)),
            "about" => Action::Launch(Launch::Msg { title: "About Notes".into(), text: "Notes\nA plain text editor.\n\nCtrl+S save, F5 inserts the time and date.".into() }),
            _ => Action::None,
        }
    }
}
