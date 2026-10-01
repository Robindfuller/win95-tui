// Small fixed-size dialogs: Run, About, Quit, the task list and message boxes.
use super::{notepad::expand, Action, App, Launch};
use crate::{draw::{st, Canvas}, icons::{palette, Icon, PAINT}, theme::{home, Theme}};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind};
use std::{fs, path::PathBuf};

/// A row of right-aligned buttons along the bottom of a dialog.
struct Buttons {
    labels: Vec<&'static str>,
    focus: usize,
    pressed: Option<usize>,
}

impl Buttons {
    fn new(labels: &[&'static str]) -> Buttons {
        Buttons { labels: labels.to_vec(), focus: 0, pressed: None }
    }
    fn layout(&self, w: i32) -> Vec<(i32, i32)> {
        let bw = 12;
        let n = self.labels.len() as i32;
        let mut x = w - n * (bw + 1) - 1;
        (0..n)
            .map(|_| {
                let r = (x, bw);
                x += bw + 1;
                r
            })
            .collect()
    }
    fn render(&self, c: &mut Canvas, th: &Theme, w: i32, y: i32) {
        for (i, (x, bw)) in self.layout(w).into_iter().enumerate() {
            c.button(x, y, bw, self.labels[i], th, i == self.focus, self.pressed == Some(i));
        }
    }
    fn hit(&self, w: i32, x: i32) -> Option<usize> {
        self.layout(w).into_iter().position(|(bx, bw)| x >= bx && x < bx + bw)
    }
    /// Shared key handling. Returns a button index when one is activated.
    fn key(&mut self, k: &KeyEvent) -> Option<usize> {
        match k.code {
            KeyCode::Tab | KeyCode::Right => self.focus = (self.focus + 1) % self.labels.len(),
            KeyCode::BackTab | KeyCode::Left => self.focus = (self.focus + self.labels.len() - 1) % self.labels.len(),
            KeyCode::Enter => return Some(self.focus),
            _ => {}
        }
        None
    }
}

fn icon_art(c: &mut Canvas, th: &Theme, x: i32, y: i32, icon: Icon) {
    if th.lite && !matches!(icon, Icon::Custom(_)) {
        let (_, col) = icon.glyph(th);
        for (j, line) in icon.lines().iter().enumerate() {
            c.text(x, y + j as i32, line, ratatui::style::Style::new().fg(col));
        }
        return;
    }
    let pal = palette(th);
    c.pixels(x, y, icon.art(), &pal);
}

// ---------------------------------------------------------------- Run

pub struct Run {
    input: String,
    history: Vec<String>,
    hpos: usize,
    buttons: Buttons,
    size: (u16, u16),
}

fn history_path() -> PathBuf {
    home().join(".local/state/win95-tui/run_history")
}

impl Run {
    pub fn new() -> Run {
        let history: Vec<String> = fs::read_to_string(history_path()).map(|s| s.lines().map(String::from).collect()).unwrap_or_default();
        let input = history.last().cloned().unwrap_or_default();
        let hpos = history.len().saturating_sub(1);
        Run { input, history, hpos, buttons: Buttons::new(&["OK", "Cancel"]), size: (56, 7) }
    }

    fn go(&mut self) -> Action {
        let cmd = self.input.trim().to_string();
        if cmd.is_empty() {
            return Action::None;
        }
        self.history.retain(|h| h != &cmd);
        self.history.push(cmd.clone());
        let keep: Vec<_> = self.history.iter().rev().take(50).rev().cloned().collect();
        let _ = fs::create_dir_all(history_path().parent().unwrap());
        let _ = fs::write(history_path(), keep.join("\n") + "\n");
        Action::Many(vec![Action::Launch(parse_run(&cmd)), Action::Close])
    }
}

pub fn parse_run(s: &str) -> Launch {
    let (prog, arg) = match s.split_once(' ') {
        Some((p, a)) => (p, Some(a.trim())),
        None => (s, None),
    };
    match prog.to_lowercase().as_str() {
        "notepad" | "notes" => Launch::Notepad(arg.map(expand)),
        "mines" | "minesweeper" => Launch::Mines,
        "files" | "explorer" => Launch::Explorer(arg.map(expand).unwrap_or_else(home)),
        "about" => Launch::About,
        "tasks" => Launch::TaskList,
        "terminal" | "term" => Launch::shell("Terminal", Icon::Terminal, None),
        _ => {
            let p = expand(s);
            if arg.is_none() && p.is_dir() {
                return Launch::Explorer(p);
            }
            Launch::Shell { cmd: Some(s.into()), cwd: None, title: prog.into(), icon: Icon::Terminal, keep_open: true }
        }
    }
}

impl App for Run {
    fn title(&self) -> String {
        "Run".into()
    }
    fn icon(&self) -> Icon {
        Icon::Run
    }
    fn size_hint(&self) -> (u16, u16) {
        (56, 7)
    }
    fn resizable(&self) -> bool {
        false
    }
    fn dialog(&self) -> bool {
        true
    }
    fn render(&mut self, c: &mut Canvas, th: &Theme, _f: bool) {
        let (w, h) = (self.size.0 as i32, self.size.1 as i32);
        let s = st(th.text, th.face);
        c.fill(0, 0, w, h, s);
        icon_art(c, th, 2, 1, Icon::Run);
        c.text(12, 1, "Type a command, folder or file", s);
        c.text(12, 2, "and it opens in its own window.", s);
        let x = c.text(2, 4, "Open:", s);
        c.field(x + 2, 4, w - x - 4, &self.input, th);
        self.buttons.render(c, th, w, h - 1);
    }
    fn cursor(&self) -> Option<(u16, u16)> {
        let room = self.size.0 as usize - 13;
        Some((10 + self.input.chars().count().min(room) as u16, 4))
    }
    fn key(&mut self, k: KeyEvent) -> Action {
        match k.code {
            KeyCode::Esc => return Action::Close,
            KeyCode::Enter => return if self.buttons.focus == 1 { Action::Close } else { self.go() },
            KeyCode::Tab | KeyCode::BackTab => {
                self.buttons.key(&k);
            }
            KeyCode::Backspace => {
                self.input.pop();
            }
            KeyCode::Up if !self.history.is_empty() => {
                self.hpos = self.hpos.saturating_sub(1);
                self.input = self.history[self.hpos].clone();
            }
            KeyCode::Down if !self.history.is_empty() => {
                self.hpos = (self.hpos + 1).min(self.history.len() - 1);
                self.input = self.history[self.hpos].clone();
            }
            KeyCode::Char('u') if k.modifiers.contains(KeyModifiers::CONTROL) => self.input.clear(),
            KeyCode::Char(ch) if !k.modifiers.contains(KeyModifiers::CONTROL) => self.input.push(ch),
            _ => {}
        }
        Action::None
    }
    fn paste(&mut self, s: &str) {
        self.input.push_str(s.lines().next().unwrap_or(""));
    }
    fn mouse(&mut self, kind: MouseEventKind, x: i32, y: i32, _m: KeyModifiers) -> Action {
        if let MouseEventKind::Down(MouseButton::Left) = kind {
            if y == self.size.1 as i32 - 1 {
                match self.buttons.hit(self.size.0 as i32, x) {
                    Some(0) => return self.go(),
                    Some(_) => return Action::Close,
                    None => {}
                }
            }
        }
        Action::None
    }
    fn resize(&mut self, w: u16, h: u16) {
        self.size = (w, h);
    }
}

// ---------------------------------------------------------------- Add Program

const GRID: (i32, i32) = (12, 7);
const SWATCH: (i32, i32) = (32, 7);

/// Type a command, paint it an icon, and it joins the Apps menu and the desktop.
pub struct AddProgram {
    cmd: String,
    /// 6 rows of 8 palette letters, '.' for nothing
    pix: Vec<Vec<char>>,
    ink: char,
    buttons: Buttons,
    size: (u16, u16),
}

impl AddProgram {
    pub fn new() -> AddProgram {
        AddProgram { cmd: String::new(), pix: vec![vec!['.'; 8]; 6], ink: PAINT[8], buttons: Buttons::new(&["Add", "Cancel"]), size: (64, 15) }
    }

    fn rows(&self) -> Vec<String> {
        self.pix.iter().map(|r| r.iter().collect()).collect()
    }

    fn go(&mut self) -> Action {
        let cmd = self.cmd.trim().to_string();
        if cmd.is_empty() {
            return Action::None;
        }
        crate::programs::add(&cmd, Some(&self.rows()));
        Action::Many(vec![Action::Refresh, Action::Close])
    }

    /// Paints (or with the right button rubs out) the pixel under x, y.
    fn paint(&mut self, x: i32, y: i32, rub: bool) -> bool {
        let (px, py) = ((x - GRID.0) / 2, y - GRID.1);
        if x < GRID.0 || !(0..8).contains(&px) || !(0..6).contains(&py) {
            return false;
        }
        self.pix[py as usize][px as usize] = if rub { '.' } else { self.ink };
        true
    }
}

impl App for AddProgram {
    fn title(&self) -> String {
        "Add Program".into()
    }
    fn icon(&self) -> Icon {
        Icon::Programs
    }
    fn size_hint(&self) -> (u16, u16) {
        (64, 15)
    }
    fn resizable(&self) -> bool {
        false
    }
    fn dialog(&self) -> bool {
        true
    }
    fn render(&mut self, c: &mut Canvas, th: &Theme, _f: bool) {
        let (w, h) = (self.size.0 as i32, self.size.1 as i32);
        let s = st(th.text, th.face);
        let dim = st(th.dim, th.face);
        c.fill(0, 0, w, h, s);
        c.text(2, 1, "Type the command that starts it, and paint it an icon.", s);
        c.text(2, 3, "Command:", s);
        c.field(GRID.0, 3, w - GRID.0 - 2, &self.cmd, th);
        let cmd = self.cmd.trim();
        let (hint, hs) = match crate::programs::missing(cmd) {
            _ if cmd.is_empty() => ("Anything you'd type in a terminal, like htop".to_string(), dim),
            Some(p) => (format!("{p} isn't installed yet: sudo pacman -S {p}"), st(th.yellow, th.face)),
            None => (format!("Shows as {} on the Apps menu and the desktop", crate::programs::name_of(cmd)), dim),
        };
        c.text_max(GRID.0, 4, &hint, hs, w - 1);

        // the painting grid: each pixel two cells wide
        c.text(2, 6, "Icon:", s);
        c.frame(GRID.0 - 1, GRID.1 - 1, 18, 8, dim);
        let pal = palette(th);
        for (y, row) in self.pix.iter().enumerate() {
            for (x, &p) in row.iter().enumerate() {
                let (cx, cy) = (GRID.0 + 2 * x as i32, GRID.1 + y as i32);
                match pal(p) {
                    Some(col) => c.text(cx, cy, "██", ratatui::style::Style::new().fg(col).bg(th.face)),
                    None => c.text(cx, cy, "· ", dim),
                };
            }
        }
        // the colours, the one in use in brackets
        for (i, &p) in PAINT.iter().enumerate() {
            let (x, y) = (SWATCH.0 + (i as i32 % 6) * 4, SWATCH.1 + i as i32 / 6);
            if let Some(col) = pal(p) {
                c.text(x + 1, y, "██", ratatui::style::Style::new().fg(col).bg(th.face));
            }
            if p == self.ink {
                c.text(x, y, "[", s);
                c.text(x + 3, y, "]", s);
            }
        }
        let rows = self.rows();
        let refs: Vec<&str> = rows.iter().map(|r| r.as_str()).collect();
        c.pixels(SWATCH.0 + 1, 10, &refs, &pal);
        c.text(SWATCH.0 + 12, 10, "[ Clear ]", s);
        c.text(SWATCH.0 + 12, 11, "right-click rubs out", dim);
        self.buttons.render(c, th, w, h - 1);
    }
    fn cursor(&self) -> Option<(u16, u16)> {
        let room = self.size.0 as usize - GRID.0 as usize - 4;
        Some((GRID.0 as u16 + 1 + self.cmd.chars().count().min(room) as u16, 3))
    }
    fn key(&mut self, k: KeyEvent) -> Action {
        match k.code {
            KeyCode::Esc => return Action::Close,
            KeyCode::Enter => return if self.buttons.focus == 1 { Action::Close } else { self.go() },
            KeyCode::Tab | KeyCode::BackTab => {
                self.buttons.key(&k);
            }
            KeyCode::Backspace => {
                self.cmd.pop();
            }
            KeyCode::Char('u') if k.modifiers.contains(KeyModifiers::CONTROL) => self.cmd.clear(),
            KeyCode::Char(ch) if !k.modifiers.contains(KeyModifiers::CONTROL) => self.cmd.push(ch),
            _ => {}
        }
        Action::None
    }
    fn paste(&mut self, s: &str) {
        self.cmd.push_str(s.lines().next().unwrap_or(""));
    }
    fn mouse(&mut self, kind: MouseEventKind, x: i32, y: i32, _m: KeyModifiers) -> Action {
        match kind {
            MouseEventKind::Down(b) | MouseEventKind::Drag(b) if b != MouseButton::Middle => {
                let rub = b == MouseButton::Right;
                if self.paint(x, y, rub) || !matches!(kind, MouseEventKind::Down(MouseButton::Left)) {
                    return Action::None;
                }
                let (sx, sy) = (x - SWATCH.0, y - SWATCH.1);
                if (0..24).contains(&sx) && (0..2).contains(&sy) {
                    self.ink = PAINT[(sy * 6 + sx / 4) as usize];
                } else if y == 10 && (SWATCH.0 + 12..SWATCH.0 + 21).contains(&x) {
                    self.pix = vec![vec!['.'; 8]; 6];
                } else if y == self.size.1 as i32 - 1 {
                    match self.buttons.hit(self.size.0 as i32, x) {
                        Some(0) => return self.go(),
                        Some(_) => return Action::Close,
                        None => {}
                    }
                }
            }
            _ => {}
        }
        Action::None
    }
    fn resize(&mut self, w: u16, h: u16) {
        self.size = (w, h);
    }
}

// ---------------------------------------------------------------- Message box

pub struct MsgBox {
    title: String,
    lines: Vec<String>,
    size: (u16, u16),
    buttons: Buttons,
}

impl MsgBox {
    pub fn new(title: String, text: String) -> MsgBox {
        let lines: Vec<String> = text.lines().map(String::from).collect();
        MsgBox { title, lines, size: (0, 0), buttons: Buttons::new(&["OK"]) }
    }
}

impl App for MsgBox {
    fn title(&self) -> String {
        self.title.clone()
    }
    fn icon(&self) -> Icon {
        Icon::Info
    }
    fn size_hint(&self) -> (u16, u16) {
        let w = self.lines.iter().map(|l| l.chars().count()).max().unwrap_or(0) + 16;
        (w.clamp(34, 90) as u16, (self.lines.len() + 4).max(6) as u16)
    }
    fn resizable(&self) -> bool {
        false
    }
    fn dialog(&self) -> bool {
        true
    }
    fn render(&mut self, c: &mut Canvas, th: &Theme, _f: bool) {
        let (w, h) = (self.size.0 as i32, self.size.1 as i32);
        let s = st(th.text, th.face);
        c.fill(0, 0, w, h, s);
        icon_art(c, th, 2, 1, Icon::Info);
        for (i, l) in self.lines.iter().enumerate() {
            c.text_max(12, 1 + i as i32, l, s, w - 1);
        }
        self.buttons.render(c, th, w, h - 1);
    }
    fn key(&mut self, k: KeyEvent) -> Action {
        match k.code {
            KeyCode::Esc | KeyCode::Enter | KeyCode::Char(' ') => Action::Close,
            _ => Action::None,
        }
    }
    fn mouse(&mut self, kind: MouseEventKind, x: i32, y: i32, _m: KeyModifiers) -> Action {
        if matches!(kind, MouseEventKind::Down(MouseButton::Left)) && y == self.size.1 as i32 - 1 && self.buttons.hit(self.size.0 as i32, x).is_some() {
            return Action::Close;
        }
        Action::None
    }
    fn resize(&mut self, w: u16, h: u16) {
        self.size = (w, h);
    }
}

// ---------------------------------------------------------------- About

pub fn about_text(th: &Theme) -> String {
    let mem = fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let field = |k: &str| -> u64 {
        mem.lines().find(|l| l.starts_with(k)).and_then(|l| l.split_whitespace().nth(1)).and_then(|v| v.parse().ok()).unwrap_or(0)
    };
    let (total, avail) = (field("MemTotal:"), field("MemAvailable:"));
    let user = std::env::var("USER").unwrap_or_default();
    let host = fs::read_to_string("/etc/hostname").unwrap_or_default().trim().to_string();
    let free = if total > 0 { avail * 100 / total } else { 0 };
    format!(
        "Desktop TUI\nThemed by Omarchy ({}).\n\n{user}@{host}\nMemory: {:.1} GB, {}% free",
        th.name,
        total as f64 / 1048576.0,
        free
    )
}

// ---------------------------------------------------------------- Quit

pub struct ShutDown {
    buttons: Buttons,
    size: (u16, u16),
}

impl ShutDown {
    pub fn new() -> ShutDown {
        ShutDown { buttons: Buttons::new(&["Quit", "Cancel"]), size: (0, 0) }
    }
}

impl App for ShutDown {
    fn title(&self) -> String {
        "Quit".into()
    }
    fn icon(&self) -> Icon {
        Icon::Shutdown
    }
    fn size_hint(&self) -> (u16, u16) {
        (44, 5)
    }
    fn resizable(&self) -> bool {
        false
    }
    fn dialog(&self) -> bool {
        true
    }
    fn render(&mut self, c: &mut Canvas, th: &Theme, _f: bool) {
        let (w, h) = (self.size.0 as i32, self.size.1 as i32);
        let s = st(th.text, th.face);
        c.fill(0, 0, w, h, s);
        c.text(2, 1, "Close every window and quit?", s);
        self.buttons.render(c, th, w, h - 1);
    }
    fn key(&mut self, k: KeyEvent) -> Action {
        match k.code {
            KeyCode::Esc | KeyCode::Char('n') => Action::Close,
            KeyCode::Char('y') | KeyCode::Char('q') => Action::Quit,
            _ => match self.buttons.key(&k) {
                Some(0) => Action::Quit,
                Some(_) => Action::Close,
                None => Action::None,
            },
        }
    }
    fn mouse(&mut self, kind: MouseEventKind, x: i32, y: i32, _m: KeyModifiers) -> Action {
        if let MouseEventKind::Down(MouseButton::Left) = kind {
            if y == self.size.1 as i32 - 1 {
                match self.buttons.hit(self.size.0 as i32, x) {
                    Some(0) => return Action::Quit,
                    Some(_) => return Action::Close,
                    None => {}
                }
            }
        }
        Action::None
    }
    fn resize(&mut self, w: u16, h: u16) {
        self.size = (w, h);
    }
}

// ---------------------------------------------------------------- Task list (Ctrl+Alt+Del)

pub struct TaskList {
    tasks: Vec<(u64, String)>,
    sel: usize,
    buttons: Buttons,
    size: (u16, u16),
}

impl TaskList {
    pub fn new(tasks: Vec<(u64, String)>) -> TaskList {
        TaskList { tasks, sel: 0, buttons: Buttons::new(&["End Task", "Quit", "Cancel"]), size: (0, 0) }
    }
    fn act(&mut self, b: usize) -> Action {
        match b {
            0 => match self.tasks.get(self.sel) {
                Some(&(id, _)) => {
                    self.tasks.remove(self.sel);
                    self.sel = self.sel.min(self.tasks.len().saturating_sub(1));
                    Action::CloseWin(id)
                }
                None => Action::None,
            },
            1 => Action::Many(vec![Action::Launch(Launch::ShutDown), Action::Close]),
            _ => Action::Close,
        }
    }
}

impl App for TaskList {
    fn title(&self) -> String {
        "Tasks".into()
    }
    fn icon(&self) -> Icon {
        Icon::Monitor
    }
    fn size_hint(&self) -> (u16, u16) {
        (48, 14)
    }
    fn resizable(&self) -> bool {
        false
    }
    fn dialog(&self) -> bool {
        true
    }
    fn render(&mut self, c: &mut Canvas, th: &Theme, _f: bool) {
        let (w, h) = (self.size.0 as i32, self.size.1 as i32);
        let s = st(th.text, th.face);
        c.fill(0, 0, w, h, s);
        let list = st(th.text, th.client);
        c.fill(1, 0, w - 2, h - 5, list);
        for (i, (_, t)) in self.tasks.iter().enumerate().take((h - 5) as usize) {
            let on = i == self.sel;
            let ss = if on { th.sel() } else { list };
            c.fill(1, i as i32, w - 2, 1, ss);
            c.text_max(2, i as i32, t, ss, w - 2);
        }
        c.text(1, h - 3, "Pick a window and End Task to close it.", st(th.dim, th.face));
        self.buttons.render(c, th, w, h - 1);
    }
    fn key(&mut self, k: KeyEvent) -> Action {
        match k.code {
            KeyCode::Esc => Action::Close,
            KeyCode::Up => {
                self.sel = self.sel.saturating_sub(1);
                Action::None
            }
            KeyCode::Down => {
                self.sel = (self.sel + 1).min(self.tasks.len().saturating_sub(1));
                Action::None
            }
            _ => match self.buttons.key(&k) {
                Some(b) => self.act(b),
                None => Action::None,
            },
        }
    }
    fn mouse(&mut self, kind: MouseEventKind, x: i32, y: i32, _m: KeyModifiers) -> Action {
        if let MouseEventKind::Down(MouseButton::Left) = kind {
            let h = self.size.1 as i32;
            if y < h - 5 && (y as usize) < self.tasks.len() {
                self.sel = y as usize;
            } else if y == h - 1 {
                if let Some(b) = self.buttons.hit(self.size.0 as i32, x) {
                    return self.act(b);
                }
            }
        }
        Action::None
    }
    fn resize(&mut self, w: u16, h: u16) {
        self.size = (w, h);
    }
}
