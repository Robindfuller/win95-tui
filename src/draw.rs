// A clipped drawing surface over a ratatui Buffer, using signed coordinates so
// windows can hang off the edge of the screen.
use crate::theme::Theme;
use ratatui::{
    buffer::{Buffer, Cell},
    layout::Rect,
    style::{Color, Modifier, Style},
};
use unicode_width::UnicodeWidthChar;

pub struct Canvas<'a> {
    pub buf: &'a mut Buffer,
    pub clip: Rect,
}

pub fn st(fg: Color, bg: Color) -> Style {
    Style::new().fg(fg).bg(bg)
}

impl<'a> Canvas<'a> {
    pub fn new(buf: &'a mut Buffer) -> Self {
        let clip = buf.area;
        Canvas { buf, clip }
    }

    pub fn inside(&self, x: i32, y: i32) -> bool {
        let r = self.clip;
        x >= r.x as i32 && y >= r.y as i32 && x < (r.x + r.width) as i32 && y < (r.y + r.height) as i32
    }

    pub fn cell(&mut self, x: i32, y: i32) -> Option<&mut Cell> {
        if self.inside(x, y) { self.buf.cell_mut((x as u16, y as u16)) } else { None }
    }

    pub fn bg_at(&self, x: i32, y: i32) -> Color {
        if self.inside(x, y) {
            self.buf.cell((x as u16, y as u16)).map(|c| c.bg).unwrap_or(Color::Reset)
        } else {
            Color::Reset
        }
    }

    pub fn put(&mut self, x: i32, y: i32, sym: &str, style: Style) {
        if let Some(cell) = self.cell(x, y) {
            cell.reset();
            cell.set_symbol(sym);
            cell.set_style(style);
        }
    }

    pub fn put_c(&mut self, x: i32, y: i32, ch: char, style: Style) {
        let mut b = [0u8; 4];
        self.put(x, y, ch.encode_utf8(&mut b), style);
    }

    /// Writes text, stopping before `max_x`. Returns the x after the last char.
    pub fn text_max(&mut self, x: i32, y: i32, s: &str, style: Style, max_x: i32) -> i32 {
        let mut cx = x;
        for ch in s.chars() {
            let w = ch.width().unwrap_or(0) as i32;
            if w == 0 {
                continue;
            }
            if cx + w > max_x {
                break;
            }
            self.put_c(cx, y, ch, style);
            if w == 2 {
                self.put(cx + 1, y, " ", style);
            }
            cx += w;
        }
        cx
    }

    pub fn text(&mut self, x: i32, y: i32, s: &str, style: Style) -> i32 {
        self.text_max(x, y, s, style, i32::MAX)
    }

    pub fn fill(&mut self, x: i32, y: i32, w: i32, h: i32, style: Style) {
        for yy in y..y + h {
            for xx in x..x + w {
                self.put(xx, yy, " ", style);
            }
        }
    }

    /// Copies `src` (origin 0,0) onto this canvas at (x,y), limited to `area`.
    pub fn blit(&mut self, src: &Buffer, x: i32, y: i32) {
        let a = src.area;
        for sy in 0..a.height as i32 {
            for sx in 0..a.width as i32 {
                let (dx, dy) = (x + sx, y + sy);
                if !self.inside(dx, dy) {
                    continue;
                }
                let cell = src.cell((sx as u16, sy as u16)).cloned().unwrap_or_default();
                let wide = cell.symbol().chars().next().and_then(|c| c.width()).unwrap_or(1) > 1;
                if wide && (sx + 1 >= a.width as i32 || !self.inside(dx + 1, dy)) {
                    self.put(dx, dy, " ", Style::new().bg(cell.bg));
                    continue;
                }
                if let Some(d) = self.cell(dx, dy) {
                    *d = cell;
                }
            }
        }
    }

    /// A raised (or sunken) bevel frame drawn with eighth-block edges.
    pub fn bevel(&mut self, x: i32, y: i32, w: i32, h: i32, th: &Theme, raised: bool) {
        let (lt, rb) = if raised { (th.hilite, th.shadow) } else { (th.shadow, th.hilite) };
        for xx in x..x + w {
            self.put(xx, y, "▔", st(lt, th.face));
            self.put(xx, y + h - 1, "▁", st(rb, th.face));
        }
        for yy in y + 1..y + h - 1 {
            self.put(x, yy, "▏", st(lt, th.face));
            self.put(x + w - 1, yy, "▕", st(rb, th.face));
        }
    }

    /// Draws a push button. `w` includes padding.
    pub fn button(&mut self, x: i32, y: i32, w: i32, label: &str, th: &Theme, default: bool, pressed: bool) {
        let bg = if pressed { th.shadow } else { th.button };
        let mut s = st(th.text, bg);
        if default {
            s = s.add_modifier(Modifier::BOLD);
        }
        self.fill(x, y, w, 1, s);
        let lw = label.chars().count() as i32;
        let lx = x + (w - lw) / 2;
        self.text(lx, y, label, s);
        self.put(x, y, "▏", st(if pressed { th.shadow } else { th.hilite }, bg));
        self.put(x + w - 1, y, "▕", st(if pressed { th.hilite } else { th.shadow }, bg));
    }

    /// A sunken single-line text field.
    pub fn field(&mut self, x: i32, y: i32, w: i32, text: &str, th: &Theme) {
        let s = st(th.text, th.client);
        self.fill(x, y, w, 1, s);
        let chars: Vec<char> = text.chars().collect();
        let room = (w - 2).max(0) as usize;
        let start = chars.len().saturating_sub(room);
        let shown: String = chars[start..].iter().collect();
        self.text_max(x + 1, y, &shown, s, x + w - 1);
    }

    /// Draws half-block pixel art. Each string is one pixel row; two rows make one text line.
    pub fn pixels(&mut self, x: i32, y: i32, rows: &[&str], pal: &dyn Fn(char) -> Option<Color>) {
        for (line, pair) in rows.chunks(2).enumerate() {
            let top: Vec<char> = pair[0].chars().collect();
            let bot: Vec<char> = pair.get(1).map(|r| r.chars().collect()).unwrap_or_default();
            for i in 0..top.len().max(bot.len()) {
                let t = top.get(i).and_then(|&c| pal(c));
                let b = bot.get(i).and_then(|&c| pal(c));
                let (cx, cy) = (x + i as i32, y + line as i32);
                let under = self.bg_at(cx, cy);
                match (t, b) {
                    (None, None) => {}
                    (Some(t), None) => self.put(cx, cy, "▀", st(t, under)),
                    (None, Some(b)) => self.put(cx, cy, "▄", st(b, under)),
                    (Some(t), Some(b)) if t == b => self.put(cx, cy, " ", Style::new().bg(t)),
                    (Some(t), Some(b)) => self.put(cx, cy, "▀", st(t, b)),
                }
            }
        }
    }
}
