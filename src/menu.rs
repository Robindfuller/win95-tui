// Cascading pop-up menus: the Apps menu, window menus and app menu bars.
use crate::{apps::Launch, draw::{st, Canvas}, icons::Icon, theme::Theme};

#[derive(Clone, Debug)]
pub enum Cmd {
    None,
    Launch(Launch),
    App(&'static str),
    Sys(Sys),
    Desk(&'static str),
    /// take a program you added off the Apps menu
    Forget(String),
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Sys {
    Restore,
    Move,
    Size,
    Minimize,
    Maximize,
    Close,
}

#[derive(Clone, Debug)]
pub struct Item {
    pub label: String,
    pub icon: Option<Icon>,
    pub cmd: Cmd,
    pub sub: Vec<Item>,
    pub sep: bool,
    pub key: &'static str,
    pub enabled: bool,
    pub checked: bool,
}

impl Item {
    pub fn new(label: impl Into<String>, cmd: Cmd) -> Item {
        Item { label: label.into(), icon: None, cmd, sub: vec![], sep: false, key: "", enabled: true, checked: false }
    }
    pub fn sep() -> Item {
        Item { sep: true, ..Item::new("", Cmd::None) }
    }
    pub fn sub(label: impl Into<String>, items: Vec<Item>) -> Item {
        Item { sub: items, ..Item::new(label, Cmd::None) }
    }
    pub fn icon(mut self, i: Icon) -> Item {
        self.icon = Some(i);
        self
    }
    pub fn key(mut self, k: &'static str) -> Item {
        self.key = k;
        self
    }
    pub fn enabled(mut self, e: bool) -> Item {
        self.enabled = e;
        self
    }
    pub fn checked(mut self, c: bool) -> Item {
        self.checked = c;
        self
    }
    fn selectable(&self) -> bool {
        !self.sep && self.enabled
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Owner {
    Start,
    Sys(u64),
    Bar(u64, usize),
    Desk,
}

pub struct Level {
    pub items: Vec<Item>,
    pub x: i32,
    pub y: i32,
    pub sel: Option<usize>,
    pub banner: bool,
}

impl Level {
    pub fn new(items: Vec<Item>, x: i32, y: i32, banner: bool) -> Level {
        Level { items, x, y, sel: None, banner }
    }
    fn label_w(&self) -> i32 {
        self.items.iter().map(|i| i.label.chars().count() + if i.key.is_empty() { 0 } else { i.key.len() + 3 }).max().unwrap_or(0) as i32
    }
    pub fn width(&self) -> i32 {
        self.label_w() + 8 + if self.banner { 2 } else { 0 }
    }
    pub fn height(&self) -> i32 {
        self.items.len() as i32 + 2
    }
    fn inner_x(&self) -> i32 {
        self.x + 1 + if self.banner { 2 } else { 0 }
    }
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.x + self.width() && y >= self.y && y < self.y + self.height()
    }
    pub fn item_at(&self, x: i32, y: i32) -> Option<usize> {
        if !self.contains(x, y) || x < self.inner_x() {
            return None;
        }
        let i = y - self.y - 1;
        (i >= 0 && (i as usize) < self.items.len() && self.items[i as usize].selectable()).then_some(i as usize)
    }
    pub fn step(&mut self, d: i32) {
        let n = self.items.len() as i32;
        if n == 0 {
            return;
        }
        let mut i = self.sel.map(|s| s as i32).unwrap_or(if d > 0 { -1 } else { n });
        for _ in 0..n {
            i = (i + d).rem_euclid(n);
            if self.items[i as usize].selectable() {
                self.sel = Some(i as usize);
                return;
            }
        }
    }

    pub fn render(&self, c: &mut Canvas, th: &Theme) {
        let (w, h) = (self.width(), self.height());
        c.fill(self.x, self.y, w, h, st(th.text, th.face));
        c.bevel(self.x, self.y, w, h, th, true);
        let ix = self.inner_x();
        let iw = self.x + w - 1 - ix;
        for (i, it) in self.items.iter().enumerate() {
            let y = self.y + 1 + i as i32;
            if it.sep {
                for x in ix..ix + iw {
                    c.put(x, y, "─", st(th.dim, th.face));
                }
                if th.lite {
                    c.put(self.x, y, "├", st(th.dim, th.face));
                    c.put(self.x + w - 1, y, "┤", st(th.dim, th.face));
                }
                continue;
            }
            let on = self.sel == Some(i);
            let (fg, bg) = if on && th.lite { (th.text, th.inactive) } else if on { (th.on_accent, th.accent) } else if it.enabled { (th.text, th.face) } else { (th.dim, th.face) };
            c.fill(ix, y, iw, 1, st(fg, bg));
            if it.checked {
                c.put(ix + 1, y, "✓", st(fg, bg));
            } else if let Some(icon) = it.icon {
                let (g, col) = icon.glyph(th);
                c.put_c(ix + 1, y, g, st(if on { fg } else { col }, bg));
            }
            c.text(ix + 3, y, &it.label, st(fg, bg));
            if !it.key.is_empty() {
                c.text(ix + iw - 2 - it.key.len() as i32, y, it.key, st(fg, bg));
            }
            if !it.sub.is_empty() {
                c.put(ix + iw - 2, y, "▸", st(fg, bg));
            }
        }
    }
}

pub struct MenuState {
    pub owner: Owner,
    pub levels: Vec<Level>,
}

impl MenuState {
    pub fn hit(&self, x: i32, y: i32) -> Option<(usize, Option<usize>)> {
        self.levels.iter().enumerate().rev().find(|(_, l)| l.contains(x, y)).map(|(i, l)| (i, l.item_at(x, y)))
    }

    /// Selects item `idx` on level `lvl`, opening its submenu if it has one.
    pub fn select(&mut self, lvl: usize, idx: usize, screen_w: i32, screen_h: i32) {
        self.levels.truncate(lvl + 1);
        let l = &mut self.levels[lvl];
        l.sel = Some(idx);
        let it = &l.items[idx];
        if !it.sub.is_empty() {
            let mut sub = Level::new(it.sub.clone(), l.x + l.width() - 1, l.y + idx as i32, false);
            if sub.x + sub.width() > screen_w {
                sub.x = (l.x - sub.width() + 1).max(0);
            }
            if sub.y + sub.height() > screen_h {
                sub.y = (screen_h - sub.height()).max(0);
            }
            self.levels.push(sub);
        }
    }

    pub fn render(&self, c: &mut Canvas, th: &Theme) {
        for l in &self.levels {
            l.render(c, th);
        }
    }
}
