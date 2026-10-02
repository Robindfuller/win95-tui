// Klondike, as it came with Windows: green felt, drag the cards about,
// double-click to send one home, right-click to send everything that can go,
// and the cards bounce off the table when you win.
use super::{Action, App};
use crate::{
    draw::{st, Canvas},
    icons::Icon,
    menu::{Cmd, Item, Sys},
    theme::{save_setting, setting, Theme},
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier},
};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

// The classic look whatever the theme.
const FELT: Color = Color::Rgb(0, 128, 0);
const SLOT: Color = Color::Rgb(0, 80, 0);
const FACE: Color = Color::Rgb(255, 255, 255);
const PICKED: Color = Color::Rgb(255, 255, 170);
const INK: Color = Color::Rgb(0, 0, 0);
const RED: Color = Color::Rgb(200, 0, 0);
const BACK: Color = Color::Rgb(0, 0, 160);
const BACK_PAT: Color = Color::Rgb(70, 110, 230);
const POINTER: Color = Color::Rgb(255, 255, 0);
/// a card the held ones may go on, shown inverted
const LIT: Color = Color::Rgb(0, 0, 0);
const LIT_INK: Color = Color::Rgb(255, 255, 255);
const LIT_RED: Color = Color::Rgb(55, 255, 255);

/// Card size, and the room each pile takes across.
const CW: i32 = 7;
const CH: i32 = 5;
const STEP: i32 = 9;
const BOARD_W: i32 = STEP * 6 + CW;
/// Rows from the top of the window to the tableau.
const TOP: i32 = 1;
const TAB: i32 = TOP + CH + 1;

#[derive(Clone, Copy, PartialEq, Debug)]
struct Card {
    /// 1 (ace) to 13 (king)
    rank: u8,
    /// spades, hearts, diamonds, clubs
    suit: u8,
    up: bool,
}

impl Card {
    fn red(self) -> bool {
        self.suit == 1 || self.suit == 2
    }
    fn label(self) -> String {
        let r = match self.rank {
            1 => "A".to_string(),
            11 => "J".into(),
            12 => "Q".into(),
            13 => "K".into(),
            n => n.to_string(),
        };
        format!("{r}{}", SUITS[self.suit as usize])
    }
}

const SUITS: [char; 4] = ['♠', '♥', '♦', '♣'];

#[derive(Clone, Copy, PartialEq, Debug)]
enum Pile {
    Stock,
    Waste,
    Found(usize),
    Tab(usize),
}

/// Cursor stops for the keyboard, left to right, top row then tableau.
const STOPS: [Pile; 13] = [
    Pile::Stock,
    Pile::Waste,
    Pile::Found(0),
    Pile::Found(1),
    Pile::Found(2),
    Pile::Found(3),
    Pile::Tab(0),
    Pile::Tab(1),
    Pile::Tab(2),
    Pile::Tab(3),
    Pile::Tab(4),
    Pile::Tab(5),
    Pile::Tab(6),
];

#[derive(Clone)]
struct Table {
    stock: Vec<Card>,
    waste: Vec<Card>,
    found: [Vec<Card>; 4],
    tab: [Vec<Card>; 7],
    score: i64,
}

impl Table {
    fn pile(&self, p: Pile) -> &Vec<Card> {
        match p {
            Pile::Stock => &self.stock,
            Pile::Waste => &self.waste,
            Pile::Found(i) => &self.found[i],
            Pile::Tab(i) => &self.tab[i],
        }
    }
    fn pile_mut(&mut self, p: Pile) -> &mut Vec<Card> {
        match p {
            Pile::Stock => &mut self.stock,
            Pile::Waste => &mut self.waste,
            Pile::Found(i) => &mut self.found[i],
            Pile::Tab(i) => &mut self.tab[i],
        }
    }
}

/// Cards held: picked up with a click, or being dragged.
#[derive(Clone, Copy)]
struct Held {
    from: Pile,
    /// index of the first held card in its pile
    at: usize,
    /// where the mouse grabbed it, from the card's top left, while dragging
    grab: Option<(i32, i32)>,
    /// where the mouse is now
    pos: (i32, i32),
    moved: bool,
}

/// A card leaping off the table at the end.
struct Bouncer {
    card: Card,
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
}

#[derive(PartialEq, Clone, Copy)]
enum Look {
    Plain,
    /// held, to be put down somewhere
    Picked,
    /// the held cards may go here: inverted, as Windows did
    Lit,
}

#[derive(PartialEq, Clone, Copy)]
enum State {
    Ready,
    Playing,
    Won,
}

pub struct Solitaire {
    t: Table,
    undo: Vec<Table>,
    draw3: bool,
    state: State,
    start: Option<Instant>,
    secs: u64,
    held: Option<Held>,
    /// last click, to spot a double-click on the same card
    last_click: Option<(Instant, Pile, usize)>,
    cur: usize,
    /// how far down a tableau column the cursor is, counted up from the top card
    depth: usize,
    kb: bool,
    /// where the mouse is, for lighting up where held cards could go
    hover: Option<(i32, i32)>,
    rng: u64,
    size: (u16, u16),
    bounce: Vec<Bouncer>,
    /// the cards the bounce leaves behind
    trail: Buffer,
    next_off: usize,
    last_tick: Instant,
}

impl Solitaire {
    pub fn new() -> Solitaire {
        let seed = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(42) | 1;
        let empty = Table { stock: vec![], waste: vec![], found: Default::default(), tab: Default::default(), score: 0 };
        let mut s = Solitaire {
            t: empty,
            undo: vec![],
            draw3: setting("solitaire-draw").as_deref() == Some("3"),
            state: State::Ready,
            start: None,
            secs: 0,
            held: None,
            last_click: None,
            cur: 0,
            depth: 0,
            kb: false,
            hover: None,
            rng: seed,
            size: (BOARD_W as u16 + 2, 30),
            bounce: vec![],
            trail: Buffer::empty(Rect::new(0, 0, 1, 1)),
            next_off: 0,
            last_tick: Instant::now(),
        };
        s.deal();
        s
    }

    fn rand(&mut self) -> u64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        self.rng
    }

    fn deal(&mut self) {
        let mut deck: Vec<Card> = (0..52).map(|i| Card { rank: (i % 13 + 1) as u8, suit: (i / 13) as u8, up: false }).collect();
        for i in (1..deck.len()).rev() {
            let j = (self.rand() % (i as u64 + 1)) as usize;
            deck.swap(i, j);
        }
        let mut t = Table { stock: vec![], waste: vec![], found: Default::default(), tab: Default::default(), score: 0 };
        for col in 0..7 {
            for row in 0..=col {
                let mut c = deck.pop().unwrap();
                c.up = row == col;
                t.tab[col].push(c);
            }
        }
        t.stock = deck;
        self.t = t;
        self.undo.clear();
        self.state = State::Ready;
        self.start = None;
        self.secs = 0;
        self.held = None;
        self.bounce.clear();
        self.trail = Buffer::empty(Rect::new(0, 0, 1, 1));
    }

    fn begin(&mut self) {
        if self.state == State::Ready {
            self.state = State::Playing;
            self.start = Some(Instant::now());
        }
    }

    fn save(&mut self) {
        self.undo.push(self.t.clone());
        if self.undo.len() > 500 {
            self.undo.remove(0);
        }
    }

    fn add(&mut self, n: i64) {
        self.t.score = (self.t.score + n).max(0);
    }

    /// Turns over cards from the stock, or puts the waste back when it's empty.
    fn draw(&mut self) {
        if self.state == State::Won {
            return;
        }
        self.held = None;
        if self.t.stock.is_empty() {
            if self.t.waste.is_empty() {
                return;
            }
            self.save();
            self.begin();
            let mut w = std::mem::take(&mut self.t.waste);
            w.reverse();
            for c in &mut w {
                c.up = false;
            }
            self.t.stock = w;
            self.add(if self.draw3 { -20 } else { -100 });
            return;
        }
        self.save();
        self.begin();
        for _ in 0..if self.draw3 { 3 } else { 1 } {
            if let Some(mut c) = self.t.stock.pop() {
                c.up = true;
                self.t.waste.push(c);
            }
        }
    }

    /// Whether cards from `at` down in `from` may be moved, as one run.
    fn movable(&self, from: Pile, at: usize) -> bool {
        let p = self.t.pile(from);
        if at >= p.len() || !p[at].up {
            return false;
        }
        match from {
            Pile::Stock => false,
            Pile::Waste | Pile::Found(_) => at == p.len() - 1,
            Pile::Tab(_) => p[at..].windows(2).all(|w| w[0].red() != w[1].red() && w[0].rank == w[1].rank + 1),
        }
    }

    fn fits(&self, card: Card, n: usize, to: Pile) -> bool {
        match to {
            Pile::Found(i) => {
                n == 1
                    && match self.t.found[i].last() {
                        None => card.rank == 1,
                        Some(top) => top.suit == card.suit && card.rank == top.rank + 1,
                    }
            }
            Pile::Tab(i) => match self.t.tab[i].last() {
                None => card.rank == 13,
                Some(top) => top.up && top.red() != card.red() && top.rank == card.rank + 1,
            },
            _ => false,
        }
    }

    /// Moves the run starting at `at` in `from` onto `to`, if the rules allow it.
    fn try_move(&mut self, from: Pile, at: usize, to: Pile) -> bool {
        if from == to || !self.movable(from, at) {
            return false;
        }
        let n = self.t.pile(from).len() - at;
        let card = self.t.pile(from)[at];
        if !self.fits(card, n, to) {
            return false;
        }
        self.save();
        self.begin();
        let run = self.t.pile_mut(from).split_off(at);
        self.t.pile_mut(to).extend(run);
        let pts = match (from, to) {
            (Pile::Waste, Pile::Tab(_)) => 5,
            (Pile::Waste | Pile::Tab(_), Pile::Found(_)) => 10,
            (Pile::Found(_), Pile::Tab(_)) => -15,
            _ => 0,
        };
        self.add(pts);
        // turn over the card the run was sitting on
        if let Pile::Tab(i) = from {
            if let Some(c) = self.t.tab[i].last_mut() {
                if !c.up {
                    c.up = true;
                    self.add(5);
                }
            }
        }
        self.check_won();
        true
    }

    /// Sends the top card of a pile to whichever foundation takes it.
    fn send_home(&mut self, from: Pile) -> bool {
        let len = self.t.pile(from).len();
        if len == 0 || matches!(from, Pile::Found(_) | Pile::Stock) {
            return false;
        }
        (0..4).any(|i| self.try_move(from, len - 1, Pile::Found(i)))
    }

    /// Everything that can go home goes home (the right-click).
    fn auto_play(&mut self) {
        let piles: Vec<Pile> = std::iter::once(Pile::Waste).chain((0..7).map(Pile::Tab)).collect();
        while piles.iter().any(|&p| self.send_home(p)) {}
    }

    /// Turns over a face-down card left on top of a column.
    fn flip(&mut self, i: usize) -> bool {
        match self.t.tab[i].last() {
            Some(c) if !c.up => {
                self.save();
                self.begin();
                self.t.tab[i].last_mut().unwrap().up = true;
                self.add(5);
                true
            }
            _ => false,
        }
    }

    fn check_won(&mut self) {
        if self.t.found.iter().all(|f| f.len() == 13) {
            self.state = State::Won;
            self.held = None;
            if self.secs >= 30 {
                self.t.score += 700_000 / self.secs as i64;
            }
            self.fresh_trail();
            self.next_off = 0;
            self.last_tick = Instant::now();
        }
    }

    // ------------------------------------------------------------ layout

    fn ox(&self) -> i32 {
        ((self.size.0 as i32 - BOARD_W) / 2).max(1)
    }

    fn col_x(&self, col: usize) -> i32 {
        self.ox() + col as i32 * STEP
    }

    /// How many waste cards show, fanned out.
    fn waste_shown(&self) -> usize {
        if self.draw3 { self.t.waste.len().min(3) } else { self.t.waste.len().min(1) }
    }

    /// Top-left corner of card `i` in a pile.
    fn card_pos(&self, p: Pile, i: usize) -> (i32, i32) {
        match p {
            Pile::Stock => (self.col_x(0), TOP),
            Pile::Waste => {
                let shown = self.waste_shown();
                let first = self.t.waste.len() - shown;
                (self.col_x(1) + 2 * i.saturating_sub(first) as i32, TOP)
            }
            Pile::Found(f) => (self.col_x(3 + f), TOP),
            Pile::Tab(c) => (self.col_x(c), TAB + i as i32),
        }
    }

    /// The pile and card under a point. The card is None on an empty pile.
    fn hit(&self, x: i32, y: i32) -> Option<(Pile, Option<usize>)> {
        if (TOP..TOP + CH).contains(&y) {
            for p in [Pile::Stock, Pile::Found(0), Pile::Found(1), Pile::Found(2), Pile::Found(3)] {
                let (px, _) = self.card_pos(p, 0);
                if x >= px && x < px + CW {
                    let n = self.t.pile(p).len();
                    return Some((p, n.checked_sub(1)));
                }
            }
            let n = self.t.waste.len();
            if n > 0 {
                let (px, _) = self.card_pos(Pile::Waste, n - 1);
                if x >= px && x < px + CW {
                    return Some((Pile::Waste, Some(n - 1)));
                }
            }
            if x >= self.col_x(1) && x < self.col_x(1) + CW {
                return Some((Pile::Waste, None));
            }
            return None;
        }
        if y < TAB {
            return None;
        }
        let col = (0..7).find(|&c| x >= self.col_x(c) && x < self.col_x(c) + CW)?;
        let n = self.t.tab[col].len();
        let k = (y - TAB) as usize;
        if n == 0 {
            return (k < CH as usize).then_some((Pile::Tab(col), None));
        }
        if k < n {
            return Some((Pile::Tab(col), Some(k)));
        }
        (k < n - 1 + CH as usize).then_some((Pile::Tab(col), Some(n - 1)))
    }

    /// Where dropped cards would land: the pile under the mouse, by column.
    fn drop_target(&self, x: i32, y: i32) -> Option<Pile> {
        let col = (0..7).find(|&c| x >= self.col_x(c) - 1 && x < self.col_x(c) + CW + 1)?;
        if y < TAB - 1 {
            return (col >= 3).then_some(Pile::Found(col - 3));
        }
        Some(Pile::Tab(col))
    }

    // ------------------------------------------------------------ drawing

    fn draw_card(c: &mut Canvas, x: i32, y: i32, card: Card, look: Look) {
        if !card.up {
            return Self::draw_back(c, x, y);
        }
        let (bg, fg, red) = match look {
            Look::Plain => (FACE, INK, RED),
            Look::Picked => (PICKED, INK, RED),
            Look::Lit => (LIT, LIT_INK, LIT_RED),
        };
        let line = st(fg, bg);
        let ink = st(if card.red() { red } else { fg }, bg);
        c.fill(x, y, CW, CH, line);
        for xx in x + 1..x + CW - 1 {
            c.put(xx, y, "─", line);
            c.put(xx, y + CH - 1, "─", line);
        }
        for yy in y + 1..y + CH - 1 {
            c.put(x, yy, "│", line);
            c.put(x + CW - 1, yy, "│", line);
        }
        Self::corners(c, x, y, fg);
        let l = card.label();
        c.text(x + 1, y, &l, ink.add_modifier(Modifier::BOLD));
        c.text(x + CW - 1 - l.chars().count() as i32, y + CH - 1, &l, ink.add_modifier(Modifier::BOLD));
        let mid = match card.rank {
            11 => "♞",
            12 => "♛",
            13 => "♚",
            _ => "",
        };
        let suit = SUITS[card.suit as usize].to_string();
        let mid = if mid.is_empty() { suit.as_str() } else { mid };
        c.text(x + CW / 2, y + CH / 2, mid, ink.add_modifier(Modifier::BOLD));
    }

    fn draw_back(c: &mut Canvas, x: i32, y: i32) {
        let line = st(FACE, BACK);
        c.fill(x, y, CW, CH, line);
        for xx in x + 1..x + CW - 1 {
            c.put(xx, y, "─", line);
            c.put(xx, y + CH - 1, "─", line);
            for yy in y + 1..y + CH - 1 {
                c.put(xx, yy, "╳", st(BACK_PAT, BACK));
            }
        }
        for yy in y + 1..y + CH - 1 {
            c.put(x, yy, "│", line);
            c.put(x + CW - 1, yy, "│", line);
        }
        Self::corners(c, x, y, FACE);
    }

    /// Rounded corners, over whatever the card sits on.
    fn corners(c: &mut Canvas, x: i32, y: i32, fg: Color) {
        for (cx, cy, s) in [(x, y, "╭"), (x + CW - 1, y, "╮"), (x, y + CH - 1, "╰"), (x + CW - 1, y + CH - 1, "╯")] {
            let under = c.bg_at(cx, cy);
            let under = if under == Color::Reset { FELT } else { under };
            c.put(cx, cy, s, st(fg, under));
        }
    }

    fn draw_slot(c: &mut Canvas, x: i32, y: i32, mark: &str, lit: bool) {
        let s = if lit { st(LIT_INK, LIT) } else { st(SLOT, FELT) };
        if lit {
            c.fill(x, y, CW, CH, s);
        }
        c.frame(x, y, CW, CH, s);
        c.text(x + CW / 2, y + CH / 2, mark, s.add_modifier(Modifier::BOLD));
    }

    /// Whether this card is held and should be drawn where the mouse is instead.
    fn dragging(&self, p: Pile, i: usize) -> bool {
        self.held.is_some_and(|h| h.from == p && i >= h.at && h.grab.is_some() && h.moved)
    }

    fn look(&self, p: Pile, i: usize, lit: bool) -> Look {
        if self.held.is_some_and(|h| h.from == p && i >= h.at) {
            Look::Picked
        } else if lit && i + 1 == self.t.pile(p).len() {
            Look::Lit
        } else {
            Look::Plain
        }
    }

    fn draw_pile(&self, c: &mut Canvas, p: Pile) {
        let cards = self.t.pile(p);
        let (x, y) = self.card_pos(p, 0);
        let lit = self.aim() == Some(p);
        match p {
            Pile::Stock => match cards.last() {
                Some(&card) => Self::draw_card(c, x, y, card, Look::Plain),
                // a circle to deal again, or a cross when that's the end of it
                None => Self::draw_slot(c, x, y, if self.t.waste.is_empty() { "✕" } else { "○" }, false),
            },
            Pile::Waste => {
                let first = cards.len() - self.waste_shown();
                for i in first..cards.len() {
                    if self.dragging(p, i) {
                        continue;
                    }
                    let (cx, cy) = self.card_pos(p, i);
                    Self::draw_card(c, cx, cy, cards[i], self.look(p, i, false));
                }
            }
            Pile::Found(_) => {
                let n = cards.len();
                // the one beneath shows while the top is dragged away
                let i = if n > 0 && self.dragging(p, n - 1) { n.checked_sub(2) } else { n.checked_sub(1) };
                match i {
                    Some(i) => Self::draw_card(c, x, y, cards[i], self.look(p, i, lit)),
                    None => Self::draw_slot(c, x, y, "A", lit),
                }
            }
            Pile::Tab(_) => {
                if cards.is_empty() || self.dragging(p, 0) {
                    Self::draw_slot(c, x, y, "", lit && cards.is_empty());
                }
                for (i, &card) in cards.iter().enumerate() {
                    if self.dragging(p, i) {
                        break;
                    }
                    let (cx, cy) = self.card_pos(p, i);
                    Self::draw_card(c, cx, cy, card, self.look(p, i, lit));
                }
            }
        }
    }

    /// The felt with the four finished piles on it, for the cards to bounce over.
    fn fresh_trail(&mut self) {
        let (w, h) = self.size;
        let mut buf = Buffer::empty(Rect::new(0, 0, w, h));
        {
            let mut c = Canvas::new(&mut buf);
            c.fill(0, 0, w as i32, h as i32, st(INK, FELT));
            for f in 0..4 {
                self.draw_pile(&mut c, Pile::Found(f));
            }
        }
        self.trail = buf;
    }

    /// Redraws one foundation on the trail once its top card has leapt off.
    fn redraw_found(&mut self, f: usize) {
        let mut buf = std::mem::take(&mut self.trail);
        self.draw_pile(&mut Canvas::new(&mut buf), Pile::Found(f));
        self.trail = buf;
    }

    fn draw_bounce(&mut self) {
        let mut c = Canvas::new(&mut self.trail);
        for b in &self.bounce {
            Self::draw_card(&mut c, b.x.round() as i32, b.y.round() as i32, b.card, Look::Plain);
        }
    }

    /// Moves the bouncing cards on, and sends the next one off when they're gone.
    fn step_bounce(&mut self) -> bool {
        let (w, h) = (self.size.0 as f32, self.size.1 as f32);
        let floor = h - 1.0 - CH as f32;
        self.bounce.retain(|b| b.x > -(CW as f32) && b.x < w);
        if self.bounce.is_empty() {
            // the top card of each foundation in turn, kings first
            let f = self.next_off % 4;
            let Some(card) = self.t.found[f].pop() else {
                return false;
            };
            self.next_off += 1;
            self.redraw_found(f);
            let (x, y) = self.card_pos(Pile::Found(f), 0);
            let r = self.rand();
            let vx = (1.0 + (r % 100) as f32 / 50.0) * if r & 0x100 == 0 { -1.0 } else { 1.0 };
            let vy = -(((r >> 9) % 100) as f32) / 100.0;
            self.bounce.push(Bouncer { card, x: x as f32, y: y as f32, vx, vy });
        }
        for b in &mut self.bounce {
            b.x += b.vx;
            b.vy += 0.35;
            b.y += b.vy;
            if b.y > floor {
                b.y = floor;
                b.vy = -b.vy * 0.75;
            }
        }
        self.draw_bounce();
        true
    }

    // ------------------------------------------------------------ input

    fn click(&mut self, b: MouseButton, x: i32, y: i32) -> Action {
        if self.state == State::Won {
            return Action::None;
        }
        if b == MouseButton::Right {
            self.held = None;
            self.auto_play();
            return Action::None;
        }
        // a second click puts held cards down
        if let Some(h) = self.held.filter(|h| h.grab.is_none()) {
            self.held = None;
            if let Some(to) = self.drop_target(x, y) {
                if to != h.from {
                    self.try_move(h.from, h.at, to);
                    return Action::None;
                }
            }
        }
        let Some((p, i)) = self.hit(x, y) else { return Action::None };
        let now = Instant::now();
        let double = self.last_click.is_some_and(|(t, lp, li)| lp == p && Some(li) == i && now.duration_since(t).as_millis() < 450);
        self.last_click = i.map(|i| (now, p, i));
        match (p, i) {
            (Pile::Stock, _) => self.draw(),
            (Pile::Tab(c), Some(i)) if i == self.t.tab[c].len() - 1 && !self.t.tab[c][i].up => {
                self.flip(c);
            }
            (_, Some(i)) if double && i + 1 == self.t.pile(p).len() => {
                self.held = None;
                self.last_click = None;
                self.send_home(p);
            }
            (_, Some(i)) if self.movable(p, i) => {
                let (cx, cy) = self.card_pos(p, i);
                self.held = Some(Held { from: p, at: i, grab: Some((x - cx, y - cy)), pos: (x, y), moved: false });
            }
            _ => {}
        }
        Action::None
    }

    fn drag(&mut self, x: i32, y: i32) {
        if let Some(h) = &mut self.held {
            if h.grab.is_some() && (x, y) != h.pos {
                h.pos = (x, y);
                h.moved = true;
            }
        }
    }

    fn release(&mut self, x: i32, y: i32) {
        let Some(h) = self.held else { return };
        let Some((gx, gy)) = h.grab else { return };
        if !h.moved {
            // just a click: keep it picked up, to put down with the next click
            self.held = Some(Held { grab: None, ..h });
            return;
        }
        self.held = None;
        if let Some(to) = self.drag_target(x, y, (gx, gy)) {
            self.try_move(h.from, h.at, to);
        }
    }

    /// Where dragged cards would land, aiming with the middle of the top card's top edge.
    fn drag_target(&self, x: i32, y: i32, (gx, gy): (i32, i32)) -> Option<Pile> {
        self.drop_target(x - gx + CW / 2, y - gy).or_else(|| self.drop_target(x, y))
    }

    /// The pile the held cards are over, if they're allowed to go there.
    fn aim(&self) -> Option<Pile> {
        let h = self.held?;
        let to = match h.grab {
            Some(g) if h.moved => self.drag_target(h.pos.0, h.pos.1, g)?,
            Some(_) => return None,
            None if self.kb => self.stop_pile(),
            None => {
                let (x, y) = self.hover?;
                self.drop_target(x, y)?
            }
        };
        let n = self.t.pile(h.from).len() - h.at;
        (to != h.from && self.movable(h.from, h.at) && self.fits(self.t.pile(h.from)[h.at], n, to)).then_some(to)
    }

    fn stop_pile(&self) -> Pile {
        STOPS[self.cur]
    }

    /// The card the keyboard cursor is on, if the pile has one.
    fn cursor_card(&self) -> Option<usize> {
        let p = self.stop_pile();
        let n = self.t.pile(p).len();
        let top = n.checked_sub(1)?;
        Some(if matches!(p, Pile::Tab(_)) { top.saturating_sub(self.depth) } else { top })
    }

    fn key_select(&mut self) {
        if self.state == State::Won {
            return;
        }
        let p = self.stop_pile();
        if let Some(h) = self.held.take() {
            if h.from != p {
                self.try_move(h.from, h.at, p);
            }
            self.depth = 0;
            return;
        }
        match (p, self.cursor_card()) {
            (Pile::Stock, _) => self.draw(),
            (Pile::Tab(c), Some(i)) if !self.t.tab[c][i].up => {
                self.flip(c);
            }
            (_, Some(i)) if self.movable(p, i) => {
                self.held = Some(Held { from: p, at: i, grab: None, pos: (0, 0), moved: false });
            }
            _ => {}
        }
    }

    fn hint(&self) -> (u16, u16) {
        (BOARD_W as u16 + 2, (TAB + 6 + 12 + CH) as u16 + 1)
    }
}

impl App for Solitaire {
    fn title(&self) -> String {
        "Solitaire".into()
    }
    fn icon(&self) -> Icon {
        Icon::Solitaire
    }
    fn size_hint(&self) -> (u16, u16) {
        self.hint()
    }
    // the cards don't grow, so neither does the table
    fn resizable(&self) -> bool {
        false
    }

    fn render(&mut self, c: &mut Canvas, th: &Theme, _focused: bool) {
        let (w, h) = (self.size.0 as i32, self.size.1 as i32);
        if self.state == State::Won {
            c.blit(&self.trail, 0, 0);
        } else {
            c.fill(0, 0, w, h, st(INK, FELT));
            for p in [Pile::Stock, Pile::Waste, Pile::Found(0), Pile::Found(1), Pile::Found(2), Pile::Found(3)] {
                self.draw_pile(c, p);
            }
            for i in 0..7 {
                self.draw_pile(c, Pile::Tab(i));
            }
            // the cursor: an arrow beside the card it's on
            if self.kb {
                let p = self.stop_pile();
                let (x, y) = self.card_pos(p, self.cursor_card().unwrap_or(0));
                c.text(x - 1, y, "▶", st(POINTER, FELT).add_modifier(Modifier::BOLD));
            }
            if let Some(hd) = self.held {
                if let (Some((gx, gy)), true) = (hd.grab, hd.moved) {
                    let cards = self.t.pile(hd.from)[hd.at..].to_vec();
                    for (k, card) in cards.into_iter().enumerate() {
                        Self::draw_card(c, hd.pos.0 - gx, hd.pos.1 - gy + k as i32, card, Look::Plain);
                    }
                }
            }
        }
        let bar = st(th.text, th.face);
        c.fill(0, h - 1, w, 1, bar);
        let status = if self.state == State::Won {
            format!(" You won!  Score: {}  Time: {}   F2 for a new game", self.t.score, self.secs)
        } else {
            format!(" Score: {}  Time: {}", self.t.score, self.secs)
        };
        c.text(0, h - 1, &status, bar);
    }

    fn key(&mut self, k: KeyEvent) -> Action {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && k.code == KeyCode::Char('z') {
            return self.command("undo");
        }
        self.kb = true;
        let tab = self.cur >= 6;
        match k.code {
            KeyCode::Left => {
                self.cur = if tab { (self.cur - 1).max(6) } else { self.cur.saturating_sub(1) };
                self.depth = 0;
            }
            KeyCode::Right => {
                self.cur = if tab { (self.cur + 1).min(12) } else { (self.cur + 1).min(5) };
                self.depth = 0;
            }
            KeyCode::Up if tab => {
                // deeper into the column's face-up cards, then up to the top row
                let col = &self.t.tab[self.cur - 6];
                let up = col.iter().filter(|c| c.up).count();
                if self.depth + 1 < up {
                    self.depth += 1;
                } else {
                    self.cur = [0, 1, 1, 2, 3, 4, 5][self.cur - 6];
                    self.depth = 0;
                }
            }
            KeyCode::Down if tab => self.depth = self.depth.saturating_sub(1),
            KeyCode::Down => {
                self.cur = [6, 7, 9, 10, 11, 12][self.cur];
                self.depth = 0;
            }
            KeyCode::Char(' ') => self.key_select(),
            KeyCode::Enter => {
                self.held = None;
                let p = self.stop_pile();
                if p == Pile::Stock {
                    self.draw();
                } else {
                    self.send_home(p);
                }
                self.depth = 0;
            }
            KeyCode::Char('a') => {
                self.held = None;
                self.auto_play();
            }
            KeyCode::Esc => self.held = None,
            KeyCode::Char('u') | KeyCode::Backspace => return self.command("undo"),
            KeyCode::F(2) | KeyCode::Char('n') => return self.command("new"),
            _ => {}
        }
        Action::None
    }

    fn mouse(&mut self, kind: MouseEventKind, x: i32, y: i32, _mods: KeyModifiers) -> Action {
        match kind {
            MouseEventKind::Down(b) => {
                self.kb = false;
                return self.click(b, x, y);
            }
            MouseEventKind::Moved => self.hover = Some((x, y)),
            MouseEventKind::Drag(MouseButton::Left) => self.drag(x, y),
            MouseEventKind::Up(MouseButton::Left) => self.release(x, y),
            _ => {}
        }
        Action::None
    }

    fn resize(&mut self, w: u16, h: u16) {
        if (w, h) == self.size {
            return;
        }
        self.size = (w, h);
        if self.state == State::Won {
            self.fresh_trail();
        }
    }

    fn poll(&mut self) -> (bool, Action) {
        if self.state == State::Won {
            // about 30 frames a second
            if self.last_tick.elapsed().as_millis() < 33 {
                return (false, Action::None);
            }
            self.last_tick = Instant::now();
            return (self.step_bounce(), Action::None);
        }
        if self.state == State::Playing {
            let s = self.start.map(|t| t.elapsed().as_secs()).unwrap_or(0);
            if s != self.secs {
                // two points off every ten seconds, as it always was
                for t in self.secs + 1..=s {
                    if t % 10 == 0 {
                        self.add(-2);
                    }
                }
                self.secs = s;
                return (true, Action::None);
            }
        }
        (false, Action::None)
    }

    fn menubar(&self) -> Vec<(&'static str, Vec<Item>)> {
        vec![
            ("Game", vec![
                Item::new("Deal", Cmd::App("new")).key("F2"),
                Item::new("Undo", Cmd::App("undo")).key("Ctrl+Z"),
                Item::sep(),
                Item::new("Draw one", Cmd::App("draw1")).checked(!self.draw3),
                Item::new("Draw three", Cmd::App("draw3")).checked(self.draw3),
                Item::sep(),
                Item::new("Exit", Cmd::Sys(Sys::Close)),
            ]),
            ("Help", vec![Item::new("How to play", Cmd::App("help")).icon(Icon::Help)]),
        ]
    }

    fn command(&mut self, cmd: &str) -> Action {
        match cmd {
            "new" => self.deal(),
            "undo" => {
                if self.state != State::Won {
                    if let Some(t) = self.undo.pop() {
                        self.t = t;
                        self.held = None;
                    }
                }
            }
            "draw1" | "draw3" => {
                let d3 = cmd == "draw3";
                if d3 != self.draw3 {
                    self.draw3 = d3;
                    save_setting("solitaire-draw", if d3 { "3" } else { "1" });
                    self.deal();
                }
            }
            "help" => {
                return Action::Launch(super::Launch::Msg {
                    title: "Solitaire".into(),
                    text: "Build the four piles at the top up from Ace to King, one suit each.\nDown the table, put cards on the next one up in the other colour.\nOnly a King goes in an empty column.\n\nDrag cards about, or click one then click where it goes.\nClick the pack to turn cards over. Double-click a card to send it\nup top, right-click to send everything that can go.\n\nKeys: arrows move, Space picks up and puts down, Enter sends a\ncard up top, A sends everything, U undoes, F2 deals again.".into(),
                })
            }
            _ => {}
        }
        Action::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn show(s: &mut Solitaire) -> String {
        let (w, h) = s.hint();
        s.resize(w, h);
        let mut buf = Buffer::empty(Rect::new(0, 0, w, h));
        s.render(&mut Canvas::new(&mut buf), &crate::theme::Theme::load(), true);
        (0..h).map(|y| (0..w).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>() + "\n").collect()
    }

    fn card(rank: u8, suit: u8) -> Card {
        Card { rank, suit, up: true }
    }

    #[test]
    fn deals_and_draws() {
        let mut s = Solitaire::new();
        assert_eq!(s.t.stock.len(), 24);
        assert!((0..7).all(|c| s.t.tab[c].len() == c + 1 && s.t.tab[c][c].up));
        println!("{}", show(&mut s));
    }

    #[test]
    fn drag_a_run_onto_another_column() {
        let mut s = Solitaire::new();
        show(&mut s);
        s.t.tab[0] = vec![card(9, 0)];
        s.t.tab[1] = vec![Card { up: false, ..card(2, 2) }, card(10, 1)];
        let (fx, fy) = s.card_pos(Pile::Tab(0), 0);
        let (tx, ty) = s.card_pos(Pile::Tab(1), 1);
        s.mouse(MouseEventKind::Down(MouseButton::Left), fx + 2, fy, KeyModifiers::NONE);
        // over the empty foundation: nothing lights up
        let (ax, ay) = s.card_pos(Pile::Found(0), 0);
        s.mouse(MouseEventKind::Drag(MouseButton::Left), ax + 2, ay, KeyModifiers::NONE);
        assert_eq!(s.aim(), None);
        // over the red 10: it does
        s.mouse(MouseEventKind::Drag(MouseButton::Left), tx + 2, ty + 1, KeyModifiers::NONE);
        assert_eq!(s.aim(), Some(Pile::Tab(1)));
        let mut buf = Buffer::empty(Rect::new(0, 0, s.size.0, s.size.1));
        s.render(&mut Canvas::new(&mut buf), &crate::theme::Theme::load(), true);
        assert_eq!(buf[((tx + 1) as u16, ty as u16)].bg, LIT);
        assert_eq!(buf[((tx + 1) as u16, ty as u16)].fg, LIT_RED);
        println!("{}", show(&mut s));
        s.mouse(MouseEventKind::Up(MouseButton::Left), tx + 2, ty + 1, KeyModifiers::NONE);
        assert!(s.t.tab[0].is_empty());
        assert_eq!(s.t.tab[1].len(), 3);
        // the King of nothing: a 9 can't go into the now empty column
        assert!(!s.try_move(Pile::Tab(1), 2, Pile::Tab(0)));
        println!("{}", show(&mut s));
    }

    #[test]
    fn click_then_click_and_double_click_home() {
        let mut s = Solitaire::new();
        show(&mut s);
        s.t.tab[2] = vec![card(1, 3)];
        let (x, y) = s.card_pos(Pile::Tab(2), 0);
        let l = MouseButton::Left;
        s.mouse(MouseEventKind::Down(l), x + 1, y + 1, KeyModifiers::NONE);
        s.mouse(MouseEventKind::Up(l), x + 1, y + 1, KeyModifiers::NONE);
        s.mouse(MouseEventKind::Down(l), x + 1, y + 1, KeyModifiers::NONE);
        s.mouse(MouseEventKind::Up(l), x + 1, y + 1, KeyModifiers::NONE);
        assert_eq!(s.t.found.iter().map(|f| f.len()).sum::<usize>(), 1);
        assert!(s.t.tab[2].is_empty());
    }

    #[test]
    fn winning_bounces() {
        let mut s = Solitaire::new();
        show(&mut s);
        s.t.stock.clear();
        s.t.waste.clear();
        s.t.tab = Default::default();
        for f in 0..4 {
            s.t.found[f] = (1..=13).map(|r| card(r, f as u8)).collect();
        }
        let k = s.t.found[3].pop().unwrap();
        s.t.waste.push(k);
        assert!(s.send_home(Pile::Waste));
        assert!(s.state == State::Won);
        for _ in 0..12 {
            s.step_bounce();
        }
        println!("{}", show(&mut s));
    }
}
