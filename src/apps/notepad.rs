use super::{Action, App, Launch};
use crate::{clip, draw::{st, Canvas}, icons::Icon, menu::{Cmd, Item, Sys}, theme::{home, Theme}};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind};
use std::{fs, path::PathBuf, time::Instant};

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
    /// where a selection started (line, column); it runs to the cursor
    anchor: Option<(usize, usize)>,
    last_click: Option<(Instant, usize, usize)>,
}

type Span = ((usize, usize), (usize, usize));

fn wordy(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
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
        Notepad { lines, cx: 0, cy: 0, top: 0, left: 0, path, modified: false, size: (60, 18), prompt: None, status, anchor: None, last_click: None }
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

    /// The selected span, start first, if there's anything in it.
    fn sel(&self) -> Option<Span> {
        let (a, b) = (self.anchor?, (self.cy, self.cx));
        (a != b).then(|| if a < b { (a, b) } else { (b, a) })
    }

    fn selected(&self) -> Option<String> {
        let ((y0, x0), (y1, x1)) = self.sel()?;
        let mut s = String::new();
        for y in y0..=y1 {
            let l = &self.lines[y];
            s.extend(&l[if y == y0 { x0 } else { 0 }..if y == y1 { x1 } else { l.len() }]);
            if y < y1 {
                s.push('\n');
            }
        }
        Some(s)
    }

    /// Remove the selected text, if any. Returns whether there was some.
    fn delete_sel(&mut self) -> bool {
        let Some(((y0, x0), (y1, x1))) = self.sel() else {
            self.anchor = None;
            return false;
        };
        let tail = self.lines[y1].split_off(x1);
        self.lines[y0].truncate(x0);
        self.lines[y0].extend(tail);
        self.lines.drain(y0 + 1..=y1);
        (self.cy, self.cx) = (y0, x0);
        self.anchor = None;
        self.modified = true;
        true
    }

    fn copy(&mut self, cut: bool) {
        if let Some(t) = self.selected() {
            clip::copy_text(&t);
            if cut {
                self.delete_sel();
            }
        }
    }

    fn select_all(&mut self) {
        self.anchor = Some((0, 0));
        self.cy = self.lines.len() - 1;
        self.cx = self.lines[self.cy].len();
    }

    /// The text position under a point in the window, scrolling a line when
    /// a drag goes past the top or bottom.
    fn at(&self, x: i32, y: i32) -> (usize, usize) {
        let cy = if y < 0 { self.top.saturating_sub(1) } else { (self.top + y as usize).min(self.top + self.text_rows()) };
        let cy = cy.min(self.lines.len() - 1);
        (cy, (self.left + x.max(0) as usize).min(self.lines[cy].len()))
    }

    fn insert(&mut self, ch: char) {
        self.delete_sel();
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
    fn tab_title(&self) -> String {
        let name = self.path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or("Untitled".into());
        format!("{}{}", name, if self.modified { "*" } else { "" })
    }
    fn size_hint(&self) -> (u16, u16) {
        (64, 18)
    }

    fn render(&mut self, c: &mut Canvas, th: &Theme, _focused: bool) {
        let s = st(th.text, th.client);
        c.fill(0, 0, self.size.0 as i32, self.size.1 as i32, s);
        let rows = self.text_rows();
        let sel = self.sel();
        for (i, line) in self.lines.iter().skip(self.top).take(rows).enumerate() {
            let shown: String = line.iter().skip(self.left).take(self.size.0 as usize).map(|&ch| if ch == '\t' { ' ' } else { ch }).collect();
            c.text_max(0, i as i32, &shown, s, self.size.0 as i32);
            // selected text, and a cell past the end where the line break is selected
            let li = self.top + i;
            if let Some(((y0, x0), (y1, x1))) = sel.filter(|((y0, _), (y1, _))| (*y0..=*y1).contains(&li)) {
                let (a, b) = (if li == y0 { x0 } else { 0 }, if li == y1 { x1 } else { line.len() + 1 });
                for ci in a.max(self.left)..b.min(self.left + self.size.0 as usize) {
                    let ch = match line.get(ci) {
                        Some('\t') | None => ' ',
                        Some(&ch) => ch,
                    };
                    c.put_c((ci - self.left) as i32, i as i32, ch, th.sel());
                }
            }
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
        let shift = k.modifiers.contains(KeyModifiers::SHIFT);
        let moving = matches!(k.code, KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down | KeyCode::Home | KeyCode::End | KeyCode::PageUp | KeyCode::PageDown);
        if moving && shift {
            self.anchor.get_or_insert((self.cy, self.cx));
        } else if moving {
            self.anchor = None;
        }
        match k.code {
            KeyCode::Char('s') if ctrl => return self.save(),
            KeyCode::Char('c') | KeyCode::Insert if ctrl => self.copy(false),
            KeyCode::Char('x') if ctrl => self.copy(true),
            KeyCode::Delete if shift => self.copy(true),
            KeyCode::Char('v') if ctrl => return self.command("paste"),
            KeyCode::Insert if shift => return self.command("paste"),
            KeyCode::Char('a') if ctrl => self.select_all(),
            KeyCode::Char('n') if ctrl => return Action::Launch(Launch::Notepad(None)),
            KeyCode::F(5) => {
                for ch in chrono::Local::now().format("%H:%M %d/%m/%Y").to_string().chars() {
                    self.insert(ch);
                }
            }
            KeyCode::Char(ch) if !ctrl => self.insert(ch),
            KeyCode::Tab => self.insert('\t'),
            KeyCode::Enter => self.insert('\n'),
            KeyCode::Backspace if self.delete_sel() => {}
            KeyCode::Delete if self.delete_sel() => {}
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

    fn mouse(&mut self, kind: MouseEventKind, x: i32, y: i32, mods: KeyModifiers) -> Action {
        match kind {
            MouseEventKind::Down(MouseButton::Left) if self.prompt.is_none() => {
                if y >= 0 && (y as usize) < self.text_rows() {
                    let (cy, cx) = self.at(x, y);
                    let double = self.last_click.is_some_and(|(t, ly, lx)| (ly, lx) == (cy, cx) && t.elapsed().as_millis() < 450);
                    self.last_click = if double { None } else { Some((Instant::now(), cy, cx)) };
                    if mods.contains(KeyModifiers::SHIFT) {
                        self.anchor.get_or_insert((self.cy, self.cx));
                    } else {
                        self.anchor = Some((cy, cx));
                    }
                    (self.cy, self.cx) = (cy, cx);
                    if double {
                        // a double click picks the word under it
                        let l = &self.lines[cy];
                        let start = (0..cx).rev().take_while(|&i| wordy(l[i])).last().unwrap_or(cx);
                        let end = (cx..l.len()).take_while(|&i| wordy(l[i])).last().map(|i| i + 1).unwrap_or(cx);
                        self.anchor = Some((cy, start));
                        self.cx = end;
                    }
                    self.clamp();
                }
            }
            MouseEventKind::Drag(MouseButton::Left) if self.prompt.is_none() && self.anchor.is_some() => {
                (self.cy, self.cx) = self.at(x, y);
                self.clamp();
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
            ("Edit", vec![
                Item::new("Cut", Cmd::App("cut")).key("Ctrl+X"),
                Item::new("Copy", Cmd::App("copy")).key("Ctrl+C"),
                Item::new("Paste", Cmd::App("paste")).key("Ctrl+V"),
                Item::new("Delete", Cmd::App("del")).key("Del"),
                Item::sep(),
                Item::new("Select All", Cmd::App("all")).key("Ctrl+A"),
                Item::new("Time/Date", Cmd::App("date")).key("F5"),
            ]),
            ("Help", vec![Item::new("About Notes", Cmd::App("about")).icon(Icon::Info)]),
        ]
    }

    fn new_tab(&self) -> Option<Launch> {
        Some(Launch::Notepad(None))
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
            "cut" | "copy" => {
                self.copy(cmd == "cut");
                self.clamp();
                Action::None
            }
            "paste" => {
                match clip::paste_text() {
                    Some(t) => self.paste(&t),
                    None => self.status = Some("Can't read the clipboard here. Use your terminal's paste key.".into()),
                }
                Action::None
            }
            "del" => {
                self.delete_sel();
                self.clamp();
                Action::None
            }
            "all" => {
                self.select_all();
                self.clamp();
                Action::None
            }
            "about" => Action::Launch(Launch::Msg { title: "About Notes".into(), text: "Notes\nA plain text editor.\n\nCtrl+S save, F5 inserts the time and date.\nDrag or Shift+arrows to select, Ctrl+C/X/V to copy, cut and paste.".into() }),
            _ => Action::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pad(txt: &str) -> Notepad {
        let mut n = Notepad::new(None);
        n.paste(txt);
        n
    }

    fn text(n: &Notepad) -> String {
        n.lines.iter().map(|l| l.iter().collect::<String>()).collect::<Vec<_>>().join("\n")
    }

    fn key(n: &mut Notepad, code: KeyCode, m: KeyModifiers) {
        n.key(KeyEvent::new(code, m));
    }

    #[test]
    fn shift_arrows_select_across_lines() {
        let mut n = pad("hello\nworld");
        (n.cy, n.cx) = (0, 3);
        key(&mut n, KeyCode::Down, KeyModifiers::SHIFT);
        key(&mut n, KeyCode::Right, KeyModifiers::SHIFT);
        assert_eq!(n.selected().as_deref(), Some("lo\nworl"));
        key(&mut n, KeyCode::Char('X'), KeyModifiers::SHIFT);
        assert_eq!(text(&n), "helXd");
        assert_eq!((n.cy, n.cx, n.sel()), (0, 4, None));
    }

    #[test]
    fn backwards_selection_and_paste_replace() {
        let mut n = pad("one two three");
        n.cx = 7;
        key(&mut n, KeyCode::Home, KeyModifiers::SHIFT);
        assert_eq!(n.selected().as_deref(), Some("one two"));
        n.paste("a\nb");
        assert_eq!(text(&n), "a\nb three");
        key(&mut n, KeyCode::Char('a'), KeyModifiers::CONTROL);
        key(&mut n, KeyCode::Backspace, KeyModifiers::NONE);
        assert_eq!(text(&n), "");
    }

    #[test]
    fn click_drag_and_double_click() {
        let mut n = pad("alpha beta_2 gamma");
        n.mouse(MouseEventKind::Down(MouseButton::Left), 2, 0, KeyModifiers::NONE);
        n.mouse(MouseEventKind::Drag(MouseButton::Left), 9, 0, KeyModifiers::NONE);
        assert_eq!(n.selected().as_deref(), Some("pha bet"));
        // a plain click drops the selection
        n.mouse(MouseEventKind::Down(MouseButton::Left), 15, 0, KeyModifiers::NONE);
        assert_eq!(n.selected(), None);
        n.last_click = None;
        n.mouse(MouseEventKind::Down(MouseButton::Left), 8, 0, KeyModifiers::NONE);
        n.mouse(MouseEventKind::Down(MouseButton::Left), 8, 0, KeyModifiers::NONE);
        assert_eq!(n.selected().as_deref(), Some("beta_2"));
        // shift+click stretches it
        n.mouse(MouseEventKind::Down(MouseButton::Left), 18, 0, KeyModifiers::SHIFT);
        assert_eq!(n.selected().as_deref(), Some("beta_2 gamma"));
        key(&mut n, KeyCode::Delete, KeyModifiers::NONE);
        assert_eq!(text(&n), "alpha ");
    }
}
