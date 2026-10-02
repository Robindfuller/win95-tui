// The volume control: a speaker in the tray (or on the top bar) that opens a
// little slider with a Mute box.
use super::*;

const POP_W: i32 = 11;
const POP_H: i32 = 15;
/// slider rows, 100% at the top
const ROWS: i32 = 10;

impl Desktop {
    /// Where the speaker sits on the taskbar, if there's a volume to show.
    pub(super) fn tray_x(&self) -> Option<i32> {
        self.vol.map(|_| self.w - 9 - 3)
    }

    /// The speaker on the top bar of the Omarchy desktop: x and width.
    pub(super) fn top_vol(&self) -> Option<(i32, i32)> {
        let label = self.vol_label()?;
        let bw = self.battery.as_ref().map_or(0, |b| b.chars().count() as i32 + 1);
        let w = label.chars().count() as i32;
        Some((self.w - 3 - bw - w - 1, w))
    }

    pub(super) fn vol_label(&self) -> Option<String> {
        let (v, muted) = self.vol?;
        Some(if muted { "♪ mute".into() } else { format!("♪ {v}%") })
    }

    fn vol_rect(&self) -> Option<(i32, i32, i32, i32)> {
        let (x, y) = self.vol_open?;
        Some((x, y, POP_W, POP_H))
    }

    pub(super) fn in_vol_popup(&self, x: i32, y: i32) -> bool {
        self.vol_rect().is_some_and(|(px, py, w, h)| x >= px && x < px + w && y >= py && y < py + h)
    }

    /// Whether x, y is on the speaker itself.
    pub(super) fn on_speaker(&self, x: i32, y: i32) -> bool {
        if self.tiling {
            return y == 0 && self.top_vol().is_some_and(|(vx, w)| x >= vx && x < vx + w);
        }
        y == self.h - 1 && self.tray_x().is_some_and(|t| x >= t && x < t + 3)
    }

    pub(super) fn toggle_vol(&mut self) {
        if self.vol_open.take().is_some() {
            return;
        }
        self.vol = crate::volume::get();
        self.menu = None;
        self.vol_open = if self.tiling {
            self.top_vol().map(|(x, w)| ((x + w / 2 - POP_W / 2).clamp(0, (self.w - POP_W).max(0)), 1))
        } else {
            self.tray_x().map(|t| ((t + 1 - POP_W / 2).min(self.w - POP_W).max(0), (self.work_h() - POP_H).max(0)))
        };
    }

    pub(super) fn set_vol(&mut self, pct: i32) {
        let Some((_, muted)) = self.vol else { return };
        let p = pct.clamp(0, 100) as u8;
        self.vol = Some((p, muted));
        crate::volume::set(p);
    }

    pub(super) fn nudge_vol(&mut self, by: i32) {
        if let Some((v, _)) = self.vol {
            self.set_vol(v as i32 + by);
        }
    }

    pub(super) fn toggle_mute(&mut self) {
        if let Some((v, muted)) = self.vol {
            self.vol = Some((v, !muted));
            crate::volume::set_mute(!muted);
        }
    }

    /// A click (or drag) inside the open popup. `drag`: only the slider counts.
    pub(super) fn vol_click(&mut self, x: i32, y: i32, drag: bool) {
        let Some((px, py, _, _)) = self.vol_rect() else { return };
        let r = y - py - 2;
        if drag || (0..ROWS).contains(&r) {
            let r = r.clamp(0, ROWS - 1);
            self.set_vol(((ROWS - 1 - r) * 100 + (ROWS - 1) / 2) / (ROWS - 1));
        } else if y == py + 2 + ROWS + 1 && x > px {
            self.toggle_mute();
        }
    }

    /// Keys while the popup is open. Returns whether it used the key.
    pub(super) fn vol_key(&mut self, k: KeyEvent) -> bool {
        if self.vol_open.is_none() {
            return false;
        }
        match k.code {
            KeyCode::Up | KeyCode::Right => self.nudge_vol(5),
            KeyCode::Down | KeyCode::Left => self.nudge_vol(-5),
            KeyCode::PageUp | KeyCode::Home => self.set_vol(100),
            KeyCode::PageDown | KeyCode::End => self.set_vol(0),
            KeyCode::Char('m') | KeyCode::Char(' ') => self.toggle_mute(),
            KeyCode::Esc | KeyCode::Enter => self.vol_open = None,
            _ => {}
        }
        true
    }

    pub(super) fn render_vol(&self, c: &mut Canvas, th: &Theme) {
        let (Some((x, y, w, h)), Some((v, muted))) = (self.vol_rect(), self.vol) else { return };
        let base = st(th.text, th.face);
        c.fill(x, y, w, h, base);
        c.bevel(x, y, w, h, th, true);
        c.text(x + 2, y + 1, "Volume", base);
        let thumb = ROWS - 1 - (v as i32 * (ROWS - 1) + 50) / 100;
        let mid = x + w / 2;
        let line = st(th.shadow, th.face);
        for r in 0..ROWS {
            let ry = y + 2 + r;
            if r == thumb {
                let s = if muted { st(th.dim, base.bg.unwrap_or(th.face)) } else { st(th.accent, base.bg.unwrap_or(th.face)) };
                c.text(mid - 1, ry, "███", s);
            } else {
                c.put(mid, ry, if r == 0 { "┬" } else if r == ROWS - 1 { "┴" } else { "│" }, line);
            }
        }
        let pct = format!("{v}%");
        c.text(x + (w - pct.chars().count() as i32) / 2, y + 2 + ROWS, &pct, base);
        c.text(x + 1, y + 3 + ROWS, if muted { "[✓] Mute" } else { "[ ] Mute" }, base);
    }

    /// Re-reads the volume now and then, in case something else changed it.
    pub(super) fn tick_vol(&mut self) -> bool {
        if self.vol_check.elapsed().as_millis() < 2000 || matches!(self.drag, Drag::Volume) {
            return false;
        }
        self.vol_check = Instant::now();
        let v = crate::volume::get();
        let changed = v != self.vol;
        self.vol = v;
        changed
    }
}
