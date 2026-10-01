use super::{Action, App};
use crate::{draw::{st, Canvas}, icons::Icon, menu::{Cmd, Item, Sys}, theme::Theme};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::style::Modifier;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[derive(PartialEq, Clone, Copy)]
enum State {
    Ready,
    Playing,
    Won,
    Lost,
}

pub struct Mines {
    w: usize,
    h: usize,
    n: usize,
    mine: Vec<bool>,
    open: Vec<bool>,
    flag: Vec<bool>,
    state: State,
    start: Option<Instant>,
    secs: u64,
    cur: (usize, usize),
    kb: bool,
    boom: Option<usize>,
    rng: u64,
    size: (u16, u16),
}

const LEVELS: [(usize, usize, usize); 3] = [(9, 9, 10), (16, 16, 40), (30, 16, 99)];

impl Mines {
    pub fn new() -> Mines {
        let seed = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(42) | 1;
        let mut m = Mines { w: 9, h: 9, n: 10, mine: vec![], open: vec![], flag: vec![], state: State::Ready, start: None, secs: 0, cur: (0, 0), kb: false, boom: None, rng: seed, size: (0, 0) };
        m.reset(0);
        m
    }

    fn reset(&mut self, level: usize) {
        let (w, h, n) = LEVELS[level];
        (self.w, self.h, self.n) = (w, h, n);
        self.mine = vec![false; w * h];
        self.open = vec![false; w * h];
        self.flag = vec![false; w * h];
        self.state = State::Ready;
        self.start = None;
        self.secs = 0;
        self.boom = None;
        self.cur = (w / 2, h / 2);
    }

    fn rand(&mut self) -> u64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        self.rng
    }

    fn neighbours(&self, i: usize) -> Vec<usize> {
        let (x, y) = ((i % self.w) as i32, (i / self.w) as i32);
        let mut v = vec![];
        for dy in -1..=1 {
            for dx in -1..=1 {
                let (nx, ny) = (x + dx, y + dy);
                if (dx, dy) != (0, 0) && nx >= 0 && ny >= 0 && (nx as usize) < self.w && (ny as usize) < self.h {
                    v.push(ny as usize * self.w + nx as usize);
                }
            }
        }
        v
    }

    fn count(&self, i: usize) -> usize {
        self.neighbours(i).into_iter().filter(|&j| self.mine[j]).count()
    }

    fn place(&mut self, safe: usize) {
        let mut keep = self.neighbours(safe);
        keep.push(safe);
        let mut placed = 0;
        while placed < self.n {
            let i = (self.rand() % (self.w * self.h) as u64) as usize;
            if !self.mine[i] && !keep.contains(&i) {
                self.mine[i] = true;
                placed += 1;
            }
        }
    }

    fn reveal(&mut self, i: usize) {
        if matches!(self.state, State::Won | State::Lost) || self.flag[i] {
            return;
        }
        if self.state == State::Ready {
            self.place(i);
            self.state = State::Playing;
            self.start = Some(Instant::now());
        }
        if self.open[i] {
            // Chord: open the neighbours when the flags around a number add up.
            let nb = self.neighbours(i);
            if nb.iter().filter(|&&j| self.flag[j]).count() == self.count(i) {
                for j in nb {
                    if !self.open[j] && !self.flag[j] {
                        self.reveal(j);
                    }
                }
            }
            return;
        }
        if self.mine[i] {
            self.boom = Some(i);
            self.state = State::Lost;
            return;
        }
        let mut stack = vec![i];
        while let Some(j) = stack.pop() {
            if self.open[j] || self.flag[j] {
                continue;
            }
            self.open[j] = true;
            if self.count(j) == 0 {
                stack.extend(self.neighbours(j).into_iter().filter(|&k| !self.open[k]));
            }
        }
        if self.open.iter().filter(|&&o| o).count() == self.w * self.h - self.n {
            self.state = State::Won;
            for k in 0..self.mine.len() {
                if self.mine[k] {
                    self.flag[k] = true;
                }
            }
        }
    }

    fn toggle_flag(&mut self, i: usize) {
        if self.state == State::Playing && !self.open[i] {
            self.flag[i] = !self.flag[i];
        }
    }

    fn origin(&self) -> (i32, i32) {
        (((self.size.0 as i32) - self.w as i32 * 2) / 2, 2)
    }

    fn face(&self) -> &'static str {
        match self.state {
            State::Won => "8)",
            State::Lost => "X(",
            _ => ":)",
        }
    }

    fn hint(&self) -> (u16, u16) {
        ((self.w * 2 + 4).max(26) as u16, (self.h + 3) as u16)
    }
}

impl App for Mines {
    fn title(&self) -> String {
        "Minesweeper".into()
    }
    fn icon(&self) -> Icon {
        Icon::Mines
    }
    fn size_hint(&self) -> (u16, u16) {
        self.hint()
    }
    fn resizable(&self) -> bool {
        false
    }

    fn render(&mut self, c: &mut Canvas, th: &Theme, _focused: bool) {
        let (cw, ch) = (self.size.0 as i32, self.size.1 as i32);
        c.fill(0, 0, cw, ch, st(th.text, th.face));
        let led = st(th.red, th.shadow).add_modifier(Modifier::BOLD);
        let left = self.n as i64 - self.flag.iter().filter(|&&f| f).count() as i64;
        c.text(1, 0, &format!("{:03}", left.clamp(-99, 999)), led);
        c.text(cw - 4, 0, &format!("{:03}", self.secs.min(999)), led);
        let fx = cw / 2 - 2;
        c.button(fx, 0, 4, self.face(), th, false, false);
        let (ox, oy) = self.origin();
        let nums = [th.blue, th.green, th.red, th.magenta, th.orange, th.cyan, th.text, th.dim];
        for y in 0..self.h {
            for x in 0..self.w {
                let i = y * self.w + x;
                let (px, py) = (ox + x as i32 * 2, oy + y as i32);
                let lost = self.state == State::Lost;
                let (sym, style) = if self.open[i] {
                    let n = self.count(i);
                    if n == 0 { ("  ".to_string(), st(th.text, th.client)) } else { (format!("{n} "), st(nums[n - 1], th.client).add_modifier(Modifier::BOLD)) }
                } else if lost && self.mine[i] && !self.flag[i] {
                    let bg = if self.boom == Some(i) { th.red } else { th.client };
                    ("● ".into(), st(th.text, bg))
                } else if lost && self.flag[i] && !self.mine[i] {
                    ("✗ ".into(), st(th.red, th.client))
                } else {
                    let bg = if (x + y) % 2 == 0 { th.tile_a } else { th.tile_b };
                    if self.flag[i] { ("⚑ ".into(), st(th.red, bg).add_modifier(Modifier::BOLD)) } else { ("  ".into(), st(th.text, bg)) }
                };
                let style = if self.kb && self.cur == (x, y) { style.add_modifier(Modifier::REVERSED) } else { style };
                c.text(px, py, &sym, style);
            }
        }
    }

    fn key(&mut self, k: KeyEvent) -> Action {
        self.kb = true;
        let (x, y) = self.cur;
        match k.code {
            KeyCode::Left => self.cur.0 = x.saturating_sub(1),
            KeyCode::Right => self.cur.0 = (x + 1).min(self.w - 1),
            KeyCode::Up => self.cur.1 = y.saturating_sub(1),
            KeyCode::Down => self.cur.1 = (y + 1).min(self.h - 1),
            KeyCode::Char(' ') | KeyCode::Enter => self.reveal(y * self.w + x),
            KeyCode::Char('f') => self.toggle_flag(y * self.w + x),
            KeyCode::F(2) | KeyCode::Char('n') => return self.command("new"),
            _ => {}
        }
        Action::None
    }

    fn mouse(&mut self, kind: MouseEventKind, x: i32, y: i32, _mods: KeyModifiers) -> Action {
        let MouseEventKind::Down(b) = kind else { return Action::None };
        self.kb = false;
        let fx = self.size.0 as i32 / 2 - 2;
        if y == 0 && x >= fx && x < fx + 4 {
            return self.command("new");
        }
        let (ox, oy) = self.origin();
        let (gx, gy) = ((x - ox).div_euclid(2), y - oy);
        if gx < 0 || gy < 0 || gx as usize >= self.w || gy as usize >= self.h {
            return Action::None;
        }
        let i = gy as usize * self.w + gx as usize;
        match b {
            MouseButton::Left | MouseButton::Middle => self.reveal(i),
            MouseButton::Right => self.toggle_flag(i),
        }
        Action::None
    }

    fn resize(&mut self, w: u16, h: u16) {
        self.size = (w, h);
    }

    fn poll(&mut self) -> (bool, Action) {
        if self.state == State::Playing {
            let s = self.start.map(|t| t.elapsed().as_secs()).unwrap_or(0);
            if s != self.secs {
                self.secs = s;
                return (true, Action::None);
            }
        }
        (false, Action::None)
    }

    fn menubar(&self) -> Vec<(&'static str, Vec<Item>)> {
        let lvl = LEVELS.iter().position(|&(w, h, n)| (w, h, n) == (self.w, self.h, self.n)).unwrap_or(0);
        vec![
            ("Game", vec![
                Item::new("New", Cmd::App("new")).key("F2"),
                Item::sep(),
                Item::new("Beginner", Cmd::App("l0")).checked(lvl == 0),
                Item::new("Intermediate", Cmd::App("l1")).checked(lvl == 1),
                Item::new("Expert", Cmd::App("l2")).checked(lvl == 2),
                Item::sep(),
                Item::new("Exit", Cmd::Sys(Sys::Close)),
            ]),
            ("Help", vec![Item::new("How to play", Cmd::App("help")).icon(Icon::Help)]),
        ]
    }

    fn command(&mut self, cmd: &str) -> Action {
        let lvl = LEVELS.iter().position(|&(w, h, n)| (w, h, n) == (self.w, self.h, self.n)).unwrap_or(0);
        match cmd {
            "new" => self.reset(lvl),
            "l0" | "l1" | "l2" => {
                self.reset(cmd[1..].parse().unwrap());
                let (w, h) = self.hint();
                return Action::Resize(w, h);
            }
            "help" => {
                return Action::Launch(super::Launch::Msg {
                    title: "Minesweeper".into(),
                    text: "Left click opens a square, right click flags it.\nClick a number with enough flags round it to open its neighbours.\n\nKeys: arrows move, Space opens, F flags, F2 new game.".into(),
                })
            }
            _ => {}
        }
        Action::None
    }
}
