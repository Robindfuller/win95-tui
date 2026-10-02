// Tabs for any window that can open another of itself: Terminal, Files and
// Notes. A strip of tabs shows along the top once there are two or more.
use super::{explorer::Explorer, notepad::Notepad, term::TermApp, Action, App, Launch};
use crate::{
    draw::{st, Canvas},
    icons::Icon,
    menu::{Cmd, Item},
    theme::Theme,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::style::Modifier;

pub struct Tabs {
    tabs: Vec<Box<dyn App>>,
    cur: usize,
    th: Theme,
    size: (u16, u16),
}

/// Builds one tab's app, for the kinds that can have tabs.
fn build(l: Launch, th: &Theme) -> Option<Box<dyn App>> {
    Some(match l {
        Launch::Shell { cmd, cwd, title, icon, keep_open } => Box::new(TermApp::new(cmd.as_deref(), cwd, &title, icon, keep_open, th).ok()?),
        Launch::Notepad(p) => Box::new(Notepad::new(p)),
        Launch::Explorer(p) => Box::new(Explorer::new(p)),
        _ => return None,
    })
}

impl Tabs {
    pub fn new(first: Box<dyn App>, th: &Theme) -> Tabs {
        Tabs { tabs: vec![first], cur: 0, th: th.clone(), size: (0, 0) }
    }

    /// Rows the tab strip takes: none with a single tab.
    fn strip(&self) -> u16 {
        (self.tabs.len() > 1) as u16
    }

    fn relayout(&mut self) {
        let (w, h) = (self.size.0, self.size.1.saturating_sub(self.strip()));
        for t in &mut self.tabs {
            t.resize(w, h);
        }
    }

    fn open(&mut self, l: Launch) -> Action {
        let Some(mut t) = build(l, &self.th) else { return Action::None };
        t.theme_changed(&self.th);
        self.tabs.insert(self.cur + 1, t);
        self.cur += 1;
        self.relayout();
        Action::None
    }

    fn close(&mut self, i: usize) -> Action {
        if self.tabs.len() <= 1 {
            return Action::Close;
        }
        self.tabs.remove(i);
        if self.cur >= i && self.cur > 0 {
            self.cur -= 1;
        }
        self.relayout();
        Action::None
    }

    fn step(&mut self, by: i32) {
        let n = self.tabs.len() as i32;
        self.cur = (self.cur as i32 + by).rem_euclid(n) as usize;
    }

    /// Each tab's x and width on the strip, then where the + goes.
    fn layout(&self) -> (Vec<(i32, i32)>, i32) {
        let n = self.tabs.len() as i32;
        let tw = ((self.size.0 as i32 - 4) / n).clamp(6, 26);
        ((0..n).map(|i| (i * tw, tw)).collect(), n * tw + 1)
    }

    /// What one of tab i's actions means for the window: a tab that closes
    /// takes just itself away, and the rest pass through.
    fn handle(&mut self, i: usize, a: Action) -> Action {
        match a {
            Action::Close => self.close(i),
            Action::OpenTab(l) => self.open(l),
            Action::Menu(items, x, y) => Action::Menu(items, x, y + self.strip() as i32),
            Action::Many(v) => {
                let v: Vec<Action> = v.into_iter().map(|a| self.handle(i, a)).collect();
                Action::Many(v)
            }
            a => a,
        }
    }
}

impl App for Tabs {
    fn title(&self) -> String {
        self.tabs[self.cur].title()
    }
    fn icon(&self) -> Icon {
        self.tabs[self.cur].icon()
    }
    fn size_hint(&self) -> (u16, u16) {
        self.tabs[self.cur].size_hint()
    }
    fn render(&mut self, c: &mut Canvas, th: &Theme, focused: bool) {
        let off = self.strip() as i32;
        if off > 0 {
            let w = self.size.0 as i32;
            let face = st(th.text, th.face);
            c.fill(0, 0, w, 1, face);
            let (tabs, plus) = self.layout();
            for (i, (x, tw)) in tabs.into_iter().enumerate() {
                let on = i == self.cur;
                let s = if on { st(th.text, th.client).add_modifier(Modifier::BOLD) } else { st(th.dim, th.face) };
                c.fill(x, 0, tw - 1, 1, s);
                let (g, col) = self.tabs[i].icon().glyph(th);
                c.put_c(x + 1, 0, g, s.fg(col));
                let t = self.tabs[i].tab_title();
                c.text_max(x + 3, 0, &t, s, x + tw - 3);
                c.put(x + tw - 2, 0, "×", s);
            }
            c.text(plus, 0, " + ", face);
        }
        // the tab draws in its own space below the strip
        let mut buf = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, self.size.0, self.size.1.saturating_sub(off as u16)));
        {
            let mut inner = Canvas::new(&mut buf);
            self.tabs[self.cur].render(&mut inner, th, focused);
        }
        c.blit(&buf, 0, off);
    }
    fn cursor(&self) -> Option<(u16, u16)> {
        self.tabs[self.cur].cursor().map(|(x, y)| (x, y + self.strip()))
    }
    fn key(&mut self, k: KeyEvent) -> Action {
        let (ctrl, shift) = (k.modifiers.contains(KeyModifiers::CONTROL), k.modifiers.contains(KeyModifiers::SHIFT));
        match k.code {
            KeyCode::Char('t' | 'T') if ctrl && shift => return self.command("tab-new"),
            KeyCode::Char('w' | 'W') if ctrl && shift => return self.command("tab-close"),
            KeyCode::PageDown if ctrl && !shift => return self.command("tab-next"),
            KeyCode::PageUp if ctrl && !shift => return self.command("tab-prev"),
            _ => {}
        }
        let a = self.tabs[self.cur].key(k);
        self.handle(self.cur, a)
    }
    fn mouse(&mut self, kind: MouseEventKind, x: i32, y: i32, mods: KeyModifiers) -> Action {
        let off = self.strip() as i32;
        if off > 0 && y == 0 {
            if let MouseEventKind::Down(b) = kind {
                let (tabs, plus) = self.layout();
                if let Some(i) = tabs.iter().position(|&(tx, tw)| x >= tx && x < tx + tw - 1) {
                    let on_x = x == tabs[i].0 + tabs[i].1 - 2;
                    if b == MouseButton::Middle || on_x {
                        return self.close(i);
                    }
                    self.cur = i;
                } else if x >= plus && x < plus + 3 {
                    return self.command("tab-new");
                }
            }
            return Action::None;
        }
        let a = self.tabs[self.cur].mouse(kind, x, y - off, mods);
        self.handle(self.cur, a)
    }
    fn paste(&mut self, s: &str) {
        self.tabs[self.cur].paste(s);
    }
    fn resize(&mut self, w: u16, h: u16) {
        self.size = (w, h);
        self.relayout();
    }
    fn poll(&mut self) -> (bool, Action) {
        let mut dirty = false;
        let mut acts = vec![];
        for (i, t) in self.tabs.iter_mut().enumerate() {
            let (d, a) = t.poll();
            dirty |= d && i == self.cur;
            if !matches!(a, Action::None) {
                acts.push((i, a));
            }
        }
        // later tabs first, so closing one doesn't shift the rest
        let mut out = vec![];
        for (i, a) in acts.into_iter().rev() {
            dirty = true;
            out.push(self.handle(i, a));
        }
        (dirty, if out.is_empty() { Action::None } else { Action::Many(out) })
    }
    fn menubar(&self) -> Vec<(&'static str, Vec<Item>)> {
        let mut bar = self.tabs[self.cur].menubar();
        let many = self.tabs.len() > 1;
        let tabs = vec![
            Item::new("New Tab", Cmd::App("tab-new")).key("Ctrl+Shift+T"),
            Item::new("Close Tab", Cmd::App("tab-close")).key("Ctrl+Shift+W").enabled(many),
            Item::new("Next Tab", Cmd::App("tab-next")).key("Ctrl+PgDn").enabled(many),
            Item::new("Previous Tab", Cmd::App("tab-prev")).key("Ctrl+PgUp").enabled(many),
            Item::sep(),
        ];
        match bar.iter_mut().find(|(n, _)| *n == "File") {
            Some((_, items)) => {
                let at = items.iter().position(|i| i.sep).unwrap_or(items.len());
                for (k, it) in tabs.into_iter().enumerate() {
                    items.insert(at + k, it);
                }
            }
            None => bar.insert(0, ("File", tabs)),
        }
        bar
    }
    fn command(&mut self, cmd: &str) -> Action {
        match cmd {
            "tab-new" => match self.tabs[self.cur].new_tab() {
                Some(l) => self.open(l),
                None => Action::None,
            },
            "tab-close" => self.close(self.cur),
            "tab-next" => {
                self.step(1);
                Action::None
            }
            "tab-prev" => {
                self.step(-1);
                Action::None
            }
            _ => {
                let a = self.tabs[self.cur].command(cmd);
                self.handle(self.cur, a)
            }
        }
    }
    fn resizable(&self) -> bool {
        self.tabs[self.cur].resizable()
    }
    fn dialog(&self) -> bool {
        self.tabs[self.cur].dialog()
    }
    fn theme_changed(&mut self, th: &Theme) {
        self.th = th.clone();
        for t in &mut self.tabs {
            t.theme_changed(th);
        }
    }
    fn new_tab(&self) -> Option<Launch> {
        self.tabs[self.cur].new_tab()
    }
}
