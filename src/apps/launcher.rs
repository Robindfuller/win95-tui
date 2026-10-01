// The app launcher for the tiling desktop: type to filter, Enter to start.
// Anything typed that matches nothing runs as a command, like Run... does.
use super::{dialogs::parse_run, Action, App, Launch};
use crate::{draw::Canvas, icons::Icon, theme::Theme};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::style::{Modifier, Style};

pub struct Launcher {
    items: Vec<(String, Icon, Launch)>,
    query: String,
    sel: usize,
    /// first row shown, when the list is longer than the box
    top: usize,
}

const ROWS: usize = 12;

impl Launcher {
    pub fn new(items: Vec<(String, Icon, Launch)>) -> Launcher {
        Launcher { items, query: String::new(), sel: 0, top: 0 }
    }

    /// Indexes of the items that match, best first: names that start with
    /// what you typed, then ones that contain it, then ones that have its
    /// letters in order.
    fn matches(&self) -> Vec<usize> {
        let q = self.query.trim().to_lowercase();
        if q.is_empty() {
            return (0..self.items.len()).collect();
        }
        let mut scored: Vec<(u8, usize)> = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(i, (name, _, _))| {
                let n = name.to_lowercase();
                let score = if n.starts_with(&q) {
                    0
                } else if n.contains(&q) {
                    1
                } else {
                    let mut it = n.chars();
                    if q.chars().all(|c| it.any(|x| x == c)) { 2 } else { return None }
                };
                Some((score, i))
            })
            .collect();
        scored.sort();
        scored.into_iter().map(|(_, i)| i).collect()
    }

    fn go(&self) -> Action {
        let m = self.matches();
        let launch = match m.get(self.sel) {
            Some(&i) => self.items[i].2.clone(),
            None if !self.query.trim().is_empty() => parse_run(self.query.trim()),
            None => return Action::None,
        };
        Action::Many(vec![Action::Close, Action::Launch(launch)])
    }

    fn step(&mut self, d: i32) {
        let n = self.matches().len() as i32;
        if n == 0 {
            return;
        }
        self.sel = (self.sel as i32 + d).rem_euclid(n) as usize;
        if self.sel < self.top {
            self.top = self.sel;
        } else if self.sel >= self.top + ROWS {
            self.top = self.sel + 1 - ROWS;
        }
    }
}

impl App for Launcher {
    fn title(&self) -> String {
        "Launch".into()
    }
    fn icon(&self) -> Icon {
        Icon::Run
    }
    fn size_hint(&self) -> (u16, u16) {
        (46, ROWS as u16 + 2)
    }
    fn resizable(&self) -> bool {
        false
    }
    fn dialog(&self) -> bool {
        true
    }
    fn cursor(&self) -> Option<(u16, u16)> {
        Some((3 + self.query.chars().count() as u16, 0))
    }
    fn render(&mut self, c: &mut Canvas, th: &Theme, _f: bool) {
        let w = c.clip.width as i32;
        let base = Style::new().fg(th.text).bg(th.client);
        c.fill(0, 0, w, ROWS as i32 + 2, base);
        c.text(1, 0, "❯", Style::new().fg(th.accent).bg(th.client).add_modifier(Modifier::BOLD));
        if self.query.is_empty() {
            c.text(3, 0, "Search apps, or type a command", Style::new().fg(th.dim).bg(th.client));
        } else {
            c.text_max(3, 0, &self.query, base, w - 1);
        }
        for x in 0..w {
            c.put(x, 1, "─", Style::new().fg(th.dim).bg(th.client));
        }
        let m = self.matches();
        if m.is_empty() {
            let hint = if self.query.trim().is_empty() { "" } else { "Enter runs it as a command" };
            c.text(2, 2, hint, Style::new().fg(th.dim).bg(th.client));
        }
        for (row, &i) in m.iter().skip(self.top).take(ROWS).enumerate() {
            let y = 2 + row as i32;
            let on = self.top + row == self.sel;
            let s = if on { th.sel() } else { base };
            c.fill(0, y, w, 1, s);
            let (name, icon, _) = &self.items[i];
            let (g, col) = icon.glyph(th);
            c.put_c(2, y, g, if on { s } else { Style::new().fg(col).bg(th.client) });
            c.text_max(4, y, name, s, w - 1);
        }
    }
    fn key(&mut self, k: KeyEvent) -> Action {
        match k.code {
            KeyCode::Esc => return Action::Close,
            KeyCode::Enter => return self.go(),
            KeyCode::Up => self.step(-1),
            KeyCode::Down | KeyCode::Tab => self.step(1),
            KeyCode::BackTab => self.step(-1),
            KeyCode::Char('p') if k.modifiers.contains(KeyModifiers::CONTROL) => self.step(-1),
            KeyCode::Char('n') if k.modifiers.contains(KeyModifiers::CONTROL) => self.step(1),
            KeyCode::Char('u') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                self.query.clear();
                (self.sel, self.top) = (0, 0);
            }
            KeyCode::Backspace => {
                self.query.pop();
                (self.sel, self.top) = (0, 0);
            }
            KeyCode::Char(ch) if !k.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {
                self.query.push(ch);
                (self.sel, self.top) = (0, 0);
            }
            _ => {}
        }
        Action::None
    }
    fn paste(&mut self, s: &str) {
        self.query.push_str(s.lines().next().unwrap_or(""));
        (self.sel, self.top) = (0, 0);
    }
    fn mouse(&mut self, kind: MouseEventKind, _x: i32, y: i32, _mods: KeyModifiers) -> Action {
        match kind {
            MouseEventKind::ScrollDown => self.step(1),
            MouseEventKind::ScrollUp => self.step(-1),
            MouseEventKind::Down(MouseButton::Left) if y >= 2 => {
                let i = self.top + (y - 2) as usize;
                if i < self.matches().len() {
                    self.sel = i;
                    return self.go();
                }
            }
            _ => {}
        }
        Action::None
    }
}
