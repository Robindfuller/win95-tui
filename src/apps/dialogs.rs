// Small fixed-size dialogs: Run, About, Shut Down, Close Program and message boxes.
use super::{notepad::expand, Action, App, Launch};
use crate::{draw::{st, Canvas}, icons::{palette, Icon}, theme::{home, Theme}};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::style::Modifier;
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
        "notepad" | "notepad.exe" => Launch::Notepad(arg.map(expand)),
        "winmine" | "winmine.exe" | "minesweeper" => Launch::Mines,
        "explorer" | "explorer.exe" => Launch::Explorer(arg.map(expand).unwrap_or_else(home)),
        "winver" => Launch::About,
        "taskman" => Launch::TaskList,
        "command" | "command.com" | "cmd" => Launch::shell("MS-DOS Prompt", Icon::Terminal, None),
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
        c.text(12, 1, "Type the name of a program, folder, or", s);
        c.text(12, 2, "document, and Windows will open it for you.", s);
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
    let commas = |n: u64| {
        let s = n.to_string();
        let mut out = String::new();
        for (i, ch) in s.chars().enumerate() {
            if i > 0 && (s.len() - i) % 3 == 0 {
                out.push(',');
            }
            out.push(ch);
        }
        out
    };
    let user = std::env::var("USER").unwrap_or_default();
    let host = fs::read_to_string("/etc/hostname").unwrap_or_default().trim().to_string();
    let free = if total > 0 { avail * 100 / total } else { 0 };
    format!(
        "Windows 95, TUI Edition\nRunning in your terminal, themed by Omarchy.\n\nThis product is licensed to:\n    {user}\n    {host}\n\nPhysical memory available to Windows: {} KB\nSystem resources: {}% Free\nTheme: {}",
        commas(total),
        free,
        th.name
    )
}

// ---------------------------------------------------------------- Shut Down

pub struct ShutDown {
    choice: usize,
    buttons: Buttons,
    size: (u16, u16),
}

const CHOICES: [&str; 3] = ["Shut down the computer?", "Restart the computer?", "Close all programs and log on as a different user?"];

impl ShutDown {
    pub fn new() -> ShutDown {
        ShutDown { choice: 0, buttons: Buttons::new(&["Yes", "No"]), size: (0, 0) }
    }
    fn go(&self) -> Action {
        match self.choice {
            0 => Action::Quit,
            _ => Action::Restart,
        }
    }
}

impl App for ShutDown {
    fn title(&self) -> String {
        "Shut Down Windows".into()
    }
    fn icon(&self) -> Icon {
        Icon::Shutdown
    }
    fn size_hint(&self) -> (u16, u16) {
        (68, 8)
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
        icon_art(c, th, 2, 1, Icon::Computer);
        c.text(12, 1, "Are you sure you want to:", s);
        for (i, l) in CHOICES.iter().enumerate() {
            let on = i == self.choice;
            let y = 2 + i as i32;
            c.text(13, y, if on { "(•)" } else { "( )" }, s);
            c.text(17, y, l, if on { s.add_modifier(Modifier::BOLD) } else { s });
        }
        self.buttons.render(c, th, w, h - 1);
    }
    fn key(&mut self, k: KeyEvent) -> Action {
        match k.code {
            KeyCode::Esc => return Action::Close,
            KeyCode::Up => self.choice = self.choice.saturating_sub(1),
            KeyCode::Down => self.choice = (self.choice + 1).min(2),
            KeyCode::Char('y') => return self.go(),
            KeyCode::Char('n') => return Action::Close,
            _ => match self.buttons.key(&k) {
                Some(0) => return self.go(),
                Some(_) => return Action::Close,
                None => {}
            },
        }
        Action::None
    }
    fn mouse(&mut self, kind: MouseEventKind, x: i32, y: i32, _m: KeyModifiers) -> Action {
        if let MouseEventKind::Down(MouseButton::Left) = kind {
            if (2..5).contains(&y) && x >= 12 {
                self.choice = (y - 2) as usize;
            } else if y == self.size.1 as i32 - 1 {
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

// ---------------------------------------------------------------- Close Program (Ctrl+Alt+Del)

pub struct TaskList {
    tasks: Vec<(u64, String)>,
    sel: usize,
    buttons: Buttons,
    size: (u16, u16),
}

impl TaskList {
    pub fn new(tasks: Vec<(u64, String)>) -> TaskList {
        TaskList { tasks, sel: 0, buttons: Buttons::new(&["End Task", "Shut Down", "Cancel"]), size: (0, 0) }
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
        "Close Program".into()
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
            let ss = if on { st(th.on_accent, th.accent) } else { list };
            c.fill(1, i as i32, w - 2, 1, ss);
            c.text_max(2, i as i32, t, ss, w - 2);
        }
        c.text(1, h - 4, "WARNING: Ctrl+Alt+Del again will not restart", st(th.dim, th.face));
        c.text(1, h - 3, "your computer. It just opens this box.", st(th.dim, th.face));
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
