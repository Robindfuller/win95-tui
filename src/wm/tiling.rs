// The Omarchy-style desktop: tiled windows on nine workspaces, a bar along
// the top and a dock that slides up from the bottom edge.
use super::*;

pub const WORKSPACES: usize = 9;
/// Columns left between tiles and at the screen's sides.
const GAP: i32 = 1;

pub type Rect4 = (i32, i32, i32, i32);

/// One split in the dwindle: tile `i` takes a share of `area`, the rest go
/// beside it (`side`) or below it.
#[derive(Clone, Copy, Debug)]
pub struct Split {
    pub i: usize,
    pub side: bool,
    pub area: Rect4,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum TopHit {
    Menu,
    Ws(usize),
    Clock,
    Power,
}

pub struct DockItem {
    pub label: String,
    pub icon: Icon,
    pub launch: Option<Launch>,
    pub key: String,
    pub pinned: bool,
}

pub struct DockLayout {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    /// big tiles with the line-art icons, or one glyph each
    pub big: bool,
    pub items: Vec<(DockItem, i32)>,
    pub divider: Option<i32>,
}

impl DockLayout {
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y - 1 && y < self.y + self.h
    }
    fn tile_w(&self) -> i32 {
        if self.big { 8 } else { 3 }
    }
    pub fn item_at(&self, x: i32, y: i32) -> Option<usize> {
        if y < self.y || y >= self.y + self.h {
            return None;
        }
        self.items.iter().position(|(_, ix)| x >= *ix && x < ix + self.tile_w())
    }
}

/// What a window counts as on the dock: the name of the thing that started it.
pub fn launch_key(l: &Launch) -> Option<String> {
    Some(match l {
        Launch::Shell { title, .. } => title.clone(),
        Launch::Notepad(_) => "Notes".into(),
        Launch::Explorer(_) => "Files".into(),
        Launch::Mines => "Mines".into(),
        _ => return None,
    })
}

fn battery() -> Option<String> {
    let dir = std::fs::read_dir("/sys/class/power_supply").ok()?;
    for e in dir.flatten() {
        if !e.file_name().to_string_lossy().starts_with("BAT") {
            continue;
        }
        let cap = std::fs::read_to_string(e.path().join("capacity")).ok()?;
        let charging = std::fs::read_to_string(e.path().join("status")).is_ok_and(|s| s.trim() == "Charging");
        return Some(format!("{}{}%", if charging { "⚡" } else { "" }, cap.trim()));
    }
    None
}

impl Desktop {
    pub fn set_tiling(&mut self, on: bool, save: bool) {
        if save {
            theme::save_tiling(on);
        }
        if self.tiling != on {
            self.tiling = on;
            self.menu = None;
            self.kbmode = None;
            self.snap = None;
            self.drag = Drag::None;
            let (sw, sh) = (self.w, self.work_h());
            let mut n = 0;
            for i in 0..self.wins.len() {
                let w = &mut self.wins[i];
                (w.max, w.snapped, w.full, w.restore, w.min) = (false, false, false, None, false);
                w.float = w.app.dialog() || !w.app.resizable();
                if on {
                    if w.float {
                        (w.x, w.y) = ((sw - w.w) / 2, ((sh - w.h) / 2).max(1));
                    }
                } else {
                    // back to overlapping windows, cascaded at their own sizes
                    let (cw, ch) = w.app.size_hint();
                    w.w = (cw as i32 + 2).min(sw);
                    w.h = (ch as i32 + 2 + w.has_bar as i32).min(sh);
                    if w.app.dialog() {
                        (w.x, w.y) = ((sw - w.w) / 2, (sh - w.h) / 2);
                    } else {
                        w.x = (if self.lite { 20 } else { 14 } + n * 3).min((sw - w.w).max(0));
                        w.y = (1 + n).min((sh - w.h).max(0));
                        n = (n + 1) % 8;
                    }
                    w.sync();
                }
            }
        }
        self.relayout();
    }

    /// The tiles of the current workspace. Dwindle, like Hyprland: each
    /// window takes a share of the space left and hands the rest on, split
    /// across whichever side is longer.
    pub fn tile_rects(&self) -> (Vec<(u64, Rect4)>, Vec<Split>) {
        let ids: Vec<u64> = self.order.iter().copied().filter(|id| self.wins.iter().any(|w| w.id == *id && w.ws == self.cur_ws && !w.float)).collect();
        let mut area = (GAP, 1, self.w - 2 * GAP, self.h - 1);
        let (mut out, mut splits) = (vec![], vec![]);
        for (i, id) in ids.iter().enumerate() {
            let (x, y, w, h) = area;
            // a terminal cell is about twice as tall as it is wide
            let side = w >= h * 2;
            let can = if side { w >= 2 * 12 + GAP } else { h >= 2 * 4 };
            if i + 1 == ids.len() || !can {
                out.push((*id, area));
                continue;
            }
            let r = self.ratios[self.cur_ws].get(i).copied().unwrap_or(0.5);
            splits.push(Split { i, side, area });
            if side {
                let lw = (((w - GAP) as f32) * r).round() as i32;
                let lw = lw.clamp(12, w - GAP - 12);
                out.push((*id, (x, y, lw, h)));
                area = (x + lw + GAP, y, w - lw - GAP, h);
            } else {
                let th = ((h as f32) * r).round() as i32;
                let th = th.clamp(4, h - 4);
                out.push((*id, (x, y, w, th)));
                area = (x, y + th, w, h - th);
            }
        }
        (out, splits)
    }

    /// Puts every window where the tiling says it goes and hides the ones on
    /// other workspaces.
    pub fn relayout(&mut self) {
        if !self.tiling {
            for w in &mut self.wins {
                (w.hidden, w.tiled) = (false, false);
            }
            return;
        }
        let cur = self.cur_ws;
        let full = self.full_win();
        let (rects, _) = self.tile_rects();
        let (sw, sh) = (self.w, self.h);
        for w in &mut self.wins {
            w.tiled = !w.float;
            w.hidden = w.ws != cur || (full.is_some() && full != Some(w.id) && !w.float);
            if w.hidden {
                continue;
            }
            let r = if full == Some(w.id) { Some((0, 0, sw, sh)) } else { rects.iter().find(|(id, _)| *id == w.id).map(|(_, r)| *r) };
            match r {
                Some((x, y, ww, hh)) if (w.x, w.y, w.w, w.h) != (x, y, ww, hh) => {
                    (w.x, w.y, w.w, w.h) = (x, y, ww, hh);
                    w.sync();
                }
                Some(_) => {}
                None => w.y = w.y.clamp(1, (sh - 1).max(1)),
            }
        }
    }

    /// The fullscreen window on this workspace, if there is one.
    pub fn full_win(&self) -> Option<u64> {
        self.wins.iter().find(|w| w.ws == self.cur_ws && w.full && !w.float).map(|w| w.id)
    }

    fn ws_empty(&self) -> bool {
        !self.wins.iter().any(|w| w.ws == self.cur_ws && !w.min)
    }

    pub fn switch_ws(&mut self, n: usize) {
        if n >= WORKSPACES {
            return;
        }
        self.cur_ws = n;
        self.menu = None;
        self.relayout();
        self.focus = self.wins.iter().rev().find(|w| !w.hidden && !w.min).map(|w| w.id);
    }

    pub fn move_to_ws(&mut self, id: u64, n: usize) {
        if n >= WORKSPACES {
            return;
        }
        let Some(w) = self.win(id) else { return };
        (w.ws, w.full) = (n, false);
        // it joins the end of the other workspace's tiles
        self.order.retain(|o| *o != id);
        self.order.push(id);
        self.relayout();
        if self.focus == Some(id) {
            self.focus = self.wins.iter().rev().find(|w| !w.hidden && !w.min).map(|w| w.id);
        }
    }

    pub fn toggle_full(&mut self, id: u64) {
        let Some(w) = self.win(id) else { return };
        if w.app.dialog() {
            return;
        }
        w.full = !w.full;
        if w.full {
            w.float = false;
        }
        self.relayout();
    }

    pub fn toggle_float(&mut self, id: u64) {
        let (sw, sh) = (self.w, self.h);
        let Some(w) = self.win(id) else { return };
        if w.app.dialog() || !w.app.resizable() {
            return;
        }
        w.float = !w.float;
        w.full = false;
        if w.float {
            // float it in the middle, at its own size
            let (cw, ch) = w.app.size_hint();
            w.w = (cw as i32 + 2).min(sw - 4);
            w.h = (ch as i32 + 2 + w.has_bar as i32).min(sh - 3);
            (w.x, w.y) = ((sw - w.w) / 2, ((sh - w.h) / 2).max(1));
            w.sync();
        }
        self.relayout();
    }

    /// The tiled window next to the focused one in a direction.
    fn neighbour(&self, dx: i32, dy: i32) -> Option<u64> {
        let f = self.wins.iter().find(|w| Some(w.id) == self.focus)?;
        let (cx, cy) = (f.x + f.w / 2, f.y + f.h / 2);
        self.wins
            .iter()
            .filter(|o| o.id != f.id && !o.hidden && !o.min && !o.float)
            .filter_map(|o| {
                let gap = match (dx, dy) {
                    (1, _) => o.x - (f.x + f.w),
                    (-1, _) => f.x - (o.x + o.w),
                    (_, 1) => o.y - (f.y + f.h),
                    _ => f.y - (o.y + o.h),
                };
                let across = if dx != 0 { (o.y + o.h / 2 - cy).abs() } else { (o.x + o.w / 2 - cx).abs() };
                (gap >= -1).then_some(((gap, across), o.id))
            })
            .min()
            .map(|(_, id)| id)
    }

    /// The tile under the pointer that a held window would swap with.
    pub fn swap_target(&self, id: u64, x: i32, y: i32) -> Option<u64> {
        self.wins.iter().rev().find(|w| w.id != id && w.tiled && w.contains(x, y)).map(|w| w.id)
    }

    pub fn swap(&mut self, a: u64, b: u64) {
        let (Some(i), Some(j)) = (self.order.iter().position(|o| *o == a), self.order.iter().position(|o| *o == b)) else { return };
        self.order.swap(i, j);
        self.relayout();
    }

    /// Keys for the tiling desktop. Super works where the terminal passes it
    /// through; Alt always does. Returns false for keys it doesn't use.
    pub fn tiling_key(&mut self, k: KeyEvent) -> bool {
        let m = k.modifiers;
        if !m.intersects(KeyModifiers::ALT | KeyModifiers::SUPER) || m.contains(KeyModifiers::CONTROL) {
            return false;
        }
        let shift = m.contains(KeyModifiers::SHIFT);
        let dir = match k.code {
            KeyCode::Left => Some((-1, 0)),
            KeyCode::Right => Some((1, 0)),
            KeyCode::Up => Some((0, -1)),
            KeyCode::Down => Some((0, 1)),
            _ => None,
        };
        if let Some((dx, dy)) = dir {
            if let Some(n) = self.neighbour(dx, dy) {
                match (shift, self.focus) {
                    (true, Some(f)) => self.swap(f, n),
                    _ => self.focus_win(n),
                }
            }
            return true;
        }
        match k.code {
            KeyCode::Enter => self.launch(Launch::shell("Terminal", Icon::Terminal, None)),
            KeyCode::Char(' ') => match self.wins.iter().find(|w| w.title == "Launch").map(|w| w.id) {
                Some(id) => self.close(id),
                None => self.launch(Launch::Launcher),
            },
            KeyCode::Char('w' | 'W') => {
                if let Some(id) = self.focus {
                    self.close(id);
                }
            }
            KeyCode::Char('f' | 'F') => {
                if let Some(id) = self.focus {
                    self.toggle_full(id);
                }
            }
            KeyCode::Char('t' | 'T') => {
                if let Some(id) = self.focus {
                    self.toggle_float(id);
                }
            }
            KeyCode::Char(c) => {
                // Shift+number arrives as its symbol on most layouts
                let n = c.to_digit(10).map(|d| (d as usize, shift)).or_else(|| ["!", "\"@", "£#", "$", "%", "^", "&", "*", "("].iter().position(|s| s.contains(c)).map(|i| (i + 1, true)));
                match n {
                    Some((d, mv)) if (1..=WORKSPACES).contains(&d) => match (mv, self.focus) {
                        (true, Some(id)) => self.move_to_ws(id, d - 1),
                        (true, None) => {}
                        _ => self.switch_ws(d - 1),
                    },
                    _ => return false,
                }
            }
            _ => return false,
        }
        true
    }

    // ------------------------------------------------------------ top bar

    pub fn top_layout(&self) -> Vec<(i32, i32, TopHit)> {
        let mut v = vec![(0, 3, TopHit::Menu)];
        let mut x = 4;
        for n in 0..WORKSPACES {
            if n < 5 || n == self.cur_ws || self.wins.iter().any(|w| w.ws == n) {
                v.push((x, 3, TopHit::Ws(n)));
                x += 3;
            }
        }
        let cw = self.top_clock().chars().count() as i32;
        v.push(((self.w - cw) / 2, cw, TopHit::Clock));
        v.push((self.w - 3, 3, TopHit::Power));
        v
    }

    fn top_clock(&self) -> String {
        chrono::Local::now().format("%A %H:%M").to_string()
    }

    pub fn top_hit(&self, x: i32) -> Option<TopHit> {
        self.top_layout().into_iter().find(|(bx, bw, _)| x >= *bx && x < bx + bw).map(|(_, _, h)| h)
    }

    pub fn top_click(&mut self, h: TopHit) {
        match h {
            TopHit::Menu => self.open_start(),
            TopHit::Ws(n) => self.switch_ws(n),
            TopHit::Clock => self.launch(Launch::Msg { title: "Date".into(), text: chrono::Local::now().format("%A %-d %B %Y\n%H:%M:%S").to_string() }),
            TopHit::Power => self.launch(Launch::ShutDown),
        }
    }

    pub fn render_top(&self, c: &mut Canvas, th: &Theme) {
        let base = st(th.text, th.face);
        c.fill(0, 0, self.w, 1, base);
        let menu_open = matches!(&self.menu, Some((m, _)) if m.owner == Owner::Start);
        for (x, w, h) in self.top_layout() {
            match h {
                TopHit::Menu => {
                    let s = if menu_open { th.sel() } else { st(th.accent, th.face).add_modifier(Modifier::BOLD) };
                    c.fill(x, 0, w, 1, s);
                    c.text(x + 1, 0, "❖", s);
                }
                TopHit::Ws(n) => {
                    let s = if n == self.cur_ws {
                        st(th.accent, th.face).add_modifier(Modifier::BOLD)
                    } else if self.wins.iter().any(|w| w.ws == n) {
                        base
                    } else {
                        st(th.dim, th.face)
                    };
                    c.text(x + 1, 0, &(n + 1).to_string(), s);
                }
                TopHit::Clock => {
                    c.text(x, 0, &self.top_clock(), base);
                }
                TopHit::Power => {
                    c.text(x + 1, 0, "⏻", st(th.text, th.face));
                    if let Some(b) = &self.battery {
                        let bw = b.chars().count() as i32;
                        c.text(x - bw - 1, 0, b, base);
                    }
                }
            }
        }
    }

    pub fn tick_tiling(&mut self) {
        self.battery = battery();
    }

    // ------------------------------------------------------------ dock

    pub fn dock_items(&self) -> Vec<DockItem> {
        let item = |label: &str, icon: Icon, launch: Launch| DockItem { key: launch_key(&launch).unwrap_or(label.into()), label: label.into(), icon, launch: Some(launch), pinned: true };
        let mut v = vec![
            item("Terminal", Icon::Terminal, Launch::shell("Terminal", Icon::Terminal, None)),
            item("Files", Icon::Folder, Launch::Explorer(home())),
            item("Notes", Icon::Notepad, Launch::Notepad(None)),
        ];
        if let Some(b) = browser() {
            v.push(item("Web", Icon::Web, Launch::shell("Web", Icon::Web, Some(&b))));
        }
        if has("btop") {
            v.push(item("Monitor", Icon::Monitor, Launch::shell("Monitor", Icon::Monitor, Some("btop"))));
        }
        if has("lazygit") {
            v.push(item("Git", Icon::Git, Launch::shell("Git", Icon::Git, Some("lazygit"))));
        }
        v.push(item("Mines", Icon::Mines, Launch::Mines));
        for (name, cmd, icon) in &self.mine {
            v.push(item(name, *icon, Launch::shell(name, *icon, Some(cmd))));
        }
        // then whatever else is running
        let mut ids: Vec<&Win> = self.wins.iter().filter(|w| !w.app.dialog()).collect();
        ids.sort_by_key(|w| w.id);
        for w in ids {
            if !v.iter().any(|d| d.key == w.key) {
                v.push(DockItem { label: w.key.clone(), icon: w.app.icon(), launch: w.relaunch.clone(), key: w.key.clone(), pinned: false });
            }
        }
        v
    }

    pub fn dock_layout(&self) -> DockLayout {
        let items = self.dock_items();
        let n = items.len() as i32;
        let has_div = items.iter().any(|d| d.pinned) && items.iter().any(|d| !d.pinned);
        let inner = |tw: i32| n * (tw + 1) - 1 + if has_div { 2 } else { 0 };
        let big = inner(8) + 4 <= self.w - 2;
        let tw = if big { 8 } else { 3 };
        let (w, h) = (inner(tw) + 4, if big { 5 } else { 3 });
        let (x0, y0) = ((self.w - w) / 2, self.h - h);
        let mut x = x0 + 2;
        let mut divider = None;
        let mut out = vec![];
        for d in items {
            if !d.pinned && has_div && divider.is_none() {
                divider = Some(x);
                x += 2;
            }
            out.push((d, x));
            x += tw + 1;
        }
        DockLayout { x: x0, y: y0, w, h, big, items: out, divider }
    }

    pub fn dock_visible(&self) -> bool {
        self.tiling && self.full_win().is_none() && (self.dock_open || self.ws_empty())
    }

    /// Slides the dock up when the pointer reaches the bottom edge, and away
    /// again once it leaves.
    pub fn dock_hover(&mut self, x: i32, y: i32) {
        if !self.tiling {
            return;
        }
        if y >= self.h - 1 && self.full_win().is_none() {
            self.dock_open = true;
        } else if self.dock_open && !self.dock_layout().contains(x, y) {
            self.dock_open = false;
        }
    }

    pub fn dock_click(&mut self, b: MouseButton, x: i32, y: i32) {
        let d = self.dock_layout();
        let Some(i) = d.item_at(x, y) else { return };
        let item = &d.items[i].0;
        // the windows it has, most recent last
        let mine: Vec<u64> = self.wins.iter().filter(|w| w.key == item.key && !w.app.dialog()).map(|w| w.id).collect();
        if b == MouseButton::Right || mine.is_empty() {
            if let Some(l) = item.launch.clone() {
                self.dock_open = false;
                self.launch(l);
            }
            return;
        }
        // focused already: on to its next window
        let id = if self.focus.is_some_and(|f| mine.contains(&f)) { mine[0] } else { *mine.last().unwrap() };
        self.focus_win(id);
    }

    pub fn render_dock(&self, c: &mut Canvas, th: &Theme) {
        let d = self.dock_layout();
        let line = Style::new().fg(th.dim).bg(th.face);
        c.fill(d.x, d.y, d.w, d.h, st(th.text, th.face));
        c.frame(d.x, d.y, d.w, d.h, line);
        if let Some(x) = d.divider {
            for yy in d.y + 1..d.y + d.h - 1 {
                c.put(x - 1, yy, "│", line);
            }
        }
        let hover = self.hover.and_then(|(x, y)| d.item_at(x, y));
        let pal = palette(th);
        for (i, (item, x)) in d.items.iter().enumerate() {
            let on = hover == Some(i);
            let (g, col) = item.icon.glyph(th);
            let col = if on { th.accent } else { col };
            if d.big {
                if let Icon::Custom(rows) = item.icon {
                    c.pixels(*x, d.y + 1, rows, &pal);
                } else {
                    for (j, l) in item.icon.lines().iter().enumerate() {
                        c.text(*x, d.y + 1 + j as i32, l, st(col, th.face));
                    }
                }
            } else {
                c.put_c(x + 1, d.y + 1, g, st(col, th.face));
            }
            // a dot on the bottom edge for each window it has open
            let n = self.wins.iter().filter(|w| w.key == item.key && !w.app.dialog()).count().min(3) as i32;
            let cx = x + if d.big { 3 } else { 1 } - (n - 1) / 2;
            for k in 0..n {
                c.put(cx + k, d.y + d.h - 1, "•", st(th.accent, th.face));
            }
            if on {
                let label = format!(" {} ", item.label);
                let lw = label.chars().count() as i32;
                let lx = (x + if d.big { 4 } else { 1 } - lw / 2).clamp(0, (self.w - lw).max(0));
                c.text(lx, d.y - 1, &label, th.sel());
            }
        }
    }

    pub fn render_empty_hint(&self, c: &mut Canvas, th: &Theme) {
        let lines = ["Alt+Enter   Terminal", "Alt+Space   Launch an app", "Alt+1..9    Workspaces", "Alt+Arrows  Move between windows"];
        let w = lines.iter().map(|l| l.chars().count() as i32).max().unwrap_or(0);
        let (x0, y0) = ((self.w - w) / 2, (self.h - lines.len() as i32) / 2 - 2);
        for (i, l) in lines.iter().enumerate() {
            c.text(x0, y0 + i as i32, l, st(th.dim, th.desk));
        }
    }

    /// The split whose dividing line is under the pointer, for dragging.
    pub fn split_at(&self, x: i32, y: i32) -> Option<Split> {
        let (rects, splits) = self.tile_rects();
        splits.into_iter().find(|s| {
            let (ax, ay, aw, ah) = s.area;
            // tiles come out in order, so tile i is the one this split sized
            let Some(&(_, (_, _, lw, lh))) = rects.get(s.i) else { return false };
            if s.side {
                let b = ax + lw;
                (b - 1..=b + GAP).contains(&x) && y >= ay && y < ay + ah
            } else {
                let b = ay + lh;
                (b - 1..=b).contains(&y) && x >= ax && x < ax + aw
            }
        })
    }

    pub fn drag_split(&mut self, s: Split, x: i32, y: i32) {
        let (ax, ay, aw, ah) = s.area;
        let r = if s.side { (x - ax) as f32 / (aw - GAP).max(1) as f32 } else { (y - ay) as f32 / ah.max(1) as f32 };
        let rs = &mut self.ratios[self.cur_ws];
        if rs.len() <= s.i {
            rs.resize(s.i + 1, 0.5);
        }
        rs[s.i] = r.clamp(0.1, 0.9);
        self.relayout();
    }
}
