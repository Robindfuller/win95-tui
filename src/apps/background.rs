// Desktop background: a picture that fills, fits or stretches, how far it
// fades out, the colour behind it, and a shade behind the icons. Changes show
// on the desktop as you make them; Cancel puts back what was there.
use super::{dialogs::Buttons, notepad::expand, Action, App};
use crate::{
    draw::{st, Canvas},
    icons::Icon,
    theme::{home, Rgb, Theme},
    wallpaper::{self, Back, FITS, MAX_FADE},
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::style::{Color, Style};

const X: i32 = 12;
const BAR: (i32, i32) = (X + 2, 20);
const SWATCHES: i32 = X + 9;
/// Windows 95 teal, then greys, then the theme's own colours
const FIXED: [Rgb; 6] = [(0, 128, 128), (0, 0, 0), (64, 64, 64), (128, 128, 128), (192, 192, 192), (255, 255, 255)];

pub struct Background {
    back: Back,
    was: Back,
    picture: String,
    hex: String,
    /// typing goes to the hex field rather than the picture one
    on_hex: bool,
    swatches: Vec<Rgb>,
    buttons: Buttons,
    size: (u16, u16),
}

fn rgb(c: Color) -> Option<Rgb> {
    match c {
        Color::Rgb(r, g, b) => Some((r, g, b)),
        _ => None,
    }
}

fn hex_of(c: Rgb) -> String {
    format!("#{:02x}{:02x}{:02x}", c.0, c.1, c.2)
}

/// A path as you'd type it, with ~ for home.
fn tidy(p: &std::path::Path) -> String {
    let s = p.to_string_lossy().to_string();
    let h = home().to_string_lossy().to_string();
    s.strip_prefix(&h).map(|r| format!("~{r}")).unwrap_or(s)
}

impl Background {
    pub fn new(th: &Theme) -> Background {
        let back = Back::load();
        let mut swatches = FIXED.to_vec();
        swatches.extend([th.red, th.orange, th.yellow, th.green, th.cyan, th.blue, th.magenta].into_iter().filter_map(rgb));
        Background {
            picture: back.picture.as_deref().map(tidy).unwrap_or_default(),
            hex: back.colour.map(hex_of).unwrap_or_default(),
            was: back.clone(),
            back,
            on_hex: false,
            swatches,
            buttons: Buttons::new(&["OK", "Cancel"]),
            size: (62, 17),
        }
    }

    fn apply(&self) -> Action {
        self.back.save();
        Action::Background
    }

    fn set_picture(&mut self, p: Option<std::path::PathBuf>) -> Action {
        self.picture = p.as_deref().map(tidy).unwrap_or_default();
        self.back.picture = p;
        self.apply()
    }

    /// The next or previous picture in the same folder.
    fn step(&mut self, by: i32) -> Action {
        let all = wallpaper::siblings(self.back.picture.as_deref());
        if all.is_empty() {
            return Action::None;
        }
        let now = self.back.picture.as_ref().and_then(|p| std::fs::canonicalize(p).ok());
        let i = now.and_then(|n| all.iter().position(|p| *p == n)).map_or(if by > 0 { 0 } else { all.len() - 1 }, |i| (i as i32 + by).rem_euclid(all.len() as i32) as usize);
        self.set_picture(Some(all[i].clone()))
    }

    fn typed(&mut self) -> Action {
        if self.on_hex {
            let c = crate::theme::hex(&self.hex);
            if self.hex.trim().is_empty() || c.is_some() {
                self.back.colour = c;
                return self.apply();
            }
            return Action::None;
        }
        let t = self.picture.trim();
        if t.is_empty() {
            self.back.picture = None;
            return self.apply();
        }
        let p = expand(t);
        if wallpaper::is_picture(&p) || p == wallpaper::omarchy() && p.exists() {
            self.back.picture = Some(p);
            return self.apply();
        }
        Action::None
    }

    fn cancel(&self) -> Action {
        self.was.save();
        Action::Many(vec![Action::Background, Action::Close])
    }

    fn status(&self) -> String {
        let t = self.picture.trim();
        if t.is_empty() {
            return "No picture, just the colour".into();
        }
        let p = expand(t);
        if self.back.picture.as_ref() != Some(&p) {
            return "Can't find a picture there yet".into();
        }
        let real = std::fs::canonicalize(&p).unwrap_or(p.clone());
        let all = wallpaper::siblings(Some(&p));
        let name = real.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        match all.iter().position(|q| *q == real) {
            Some(i) => format!("{name}  ({} of {})", i + 1, all.len()),
            None => name,
        }
    }
}

impl App for Background {
    fn title(&self) -> String {
        "Background".into()
    }
    fn icon(&self) -> Icon {
        Icon::Computer
    }
    fn size_hint(&self) -> (u16, u16) {
        (62, 17)
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

        c.text(2, 1, "Picture:", s);
        c.field(X, 1, w - X - 2, &self.picture, th);
        c.text_max(X, 2, &self.status(), dim, w - 1);
        c.button(X, 3, 5, "◂", th, false, false);
        c.button(X + 6, 3, 5, "▸", th, false, false);
        c.button(X + 12, 3, 11, "Omarchy", th, false, false);
        c.button(X + 24, 3, 8, "None", th, false, false);

        c.text(2, 5, "Position:", s);
        for (i, (f, name)) in FITS.iter().enumerate() {
            let mark = if *f == self.back.fit { "(•) " } else { "( ) " };
            c.text(X + 12 * i as i32, 5, &format!("{mark}{name}"), s);
        }

        c.text(2, 7, "Fade:", s);
        c.text(X, 7, "◂", s);
        let filled = self.back.fade as i32 * BAR.1 / MAX_FADE as i32;
        for i in 0..BAR.1 {
            let (ch, col) = if i < filled { ("█", th.accent) } else { ("░", th.dim) };
            c.put(BAR.0 + i, 7, ch, st(col, th.face));
        }
        c.text(BAR.0 + BAR.1 + 1, 7, "▸", s);
        c.text(BAR.0 + BAR.1 + 3, 7, &format!("{}%", self.back.fade), s);

        c.text(2, 9, "Colour:", s);
        let theme_on = self.back.colour.is_none();
        c.text(X, 9, "Theme", if theme_on { th.sel() } else { s });
        for (i, &col) in self.swatches.iter().enumerate() {
            let x = SWATCHES + 3 * i as i32;
            c.text(x, 9, "██", Style::new().fg(Color::Rgb(col.0, col.1, col.2)).bg(th.face));
            if self.back.colour == Some(col) {
                c.text(x, 10, "▔▔", st(th.text, th.face));
            }
        }
        c.text(2, 11, "Hex:", s);
        c.field(X, 11, 10, &self.hex, th);
        if let Some(col) = self.back.colour {
            c.text(X + 11, 11, "████", Style::new().fg(Color::Rgb(col.0, col.1, col.2)).bg(th.face));
        }

        c.text(X, 13, if self.back.shade { "[✓] Shade behind icons" } else { "[ ] Shade behind icons" }, s);
        c.text(X + 4, 14, "keeps icons readable on any picture", dim);
        self.buttons.render(c, th, w, h - 1);
    }
    fn cursor(&self) -> Option<(u16, u16)> {
        if self.on_hex {
            return Some((X as u16 + 1 + self.hex.chars().count().min(8) as u16, 11));
        }
        let room = self.size.0 as usize - X as usize - 4;
        Some((X as u16 + 1 + self.picture.chars().count().min(room) as u16, 1))
    }
    fn key(&mut self, k: KeyEvent) -> Action {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        match k.code {
            KeyCode::Esc => return self.cancel(),
            KeyCode::Enter => return if self.buttons.focus == 1 { self.cancel() } else { Action::Close },
            KeyCode::Tab | KeyCode::BackTab => self.on_hex = !self.on_hex,
            KeyCode::PageDown => return self.step(1),
            KeyCode::PageUp => return self.step(-1),
            KeyCode::Backspace => {
                if self.on_hex { self.hex.pop() } else { self.picture.pop() };
                return self.typed();
            }
            KeyCode::Char('u') if ctrl => {
                if self.on_hex { self.hex.clear() } else { self.picture.clear() };
                return self.typed();
            }
            KeyCode::Char(ch) if !ctrl => {
                if self.on_hex { self.hex.push(ch) } else { self.picture.push(ch) };
                return self.typed();
            }
            _ => {}
        }
        Action::None
    }
    fn paste(&mut self, s: &str) {
        let line = s.lines().next().unwrap_or("").trim_matches(|c| c == '\'' || c == '"');
        if self.on_hex { self.hex.push_str(line) } else { self.picture.push_str(line) };
    }
    fn mouse(&mut self, kind: MouseEventKind, x: i32, y: i32, _m: KeyModifiers) -> Action {
        let down = matches!(kind, MouseEventKind::Down(MouseButton::Left));
        // the fade bar can be dragged along
        if y == 7 && matches!(kind, MouseEventKind::Down(MouseButton::Left) | MouseEventKind::Drag(MouseButton::Left)) && x >= BAR.0 && x < BAR.0 + BAR.1 {
            self.back.fade = (((x - BAR.0 + 1) * MAX_FADE as i32) / BAR.1) as u8;
            return self.apply();
        }
        if !down {
            return Action::None;
        }
        match y {
            1 => self.on_hex = false,
            11 => self.on_hex = true,
            3 if (X..X + 5).contains(&x) => return self.step(-1),
            3 if (X + 6..X + 11).contains(&x) => return self.step(1),
            3 if (X + 12..X + 23).contains(&x) => return self.set_picture(Some(wallpaper::omarchy())),
            3 if (X + 24..X + 32).contains(&x) => return self.set_picture(None),
            5 if x >= X => {
                if let Some((f, _)) = FITS.get(((x - X) / 12) as usize) {
                    self.back.fit = *f;
                    return self.apply();
                }
            }
            7 if x == X || x == BAR.0 + BAR.1 + 1 => {
                let by: i32 = if x == X { -10 } else { 10 };
                self.back.fade = (self.back.fade as i32 + by).clamp(0, MAX_FADE as i32) as u8;
                return self.apply();
            }
            9 if (X..X + 5).contains(&x) => {
                self.back.colour = None;
                self.hex.clear();
                return self.apply();
            }
            9 if x >= SWATCHES && (x - SWATCHES) % 3 < 2 => {
                if let Some(&col) = self.swatches.get(((x - SWATCHES) / 3) as usize) {
                    self.back.colour = Some(col);
                    self.hex = hex_of(col);
                    return self.apply();
                }
            }
            13 if x >= X && x < X + 22 => {
                self.back.shade = !self.back.shade;
                return self.apply();
            }
            _ if y == self.size.1 as i32 - 1 => match self.buttons.hit(self.size.0 as i32, x) {
                Some(0) => return Action::Close,
                Some(_) => return self.cancel(),
                None => {}
            },
            _ => {}
        }
        Action::None
    }
    fn resize(&mut self, w: u16, h: u16) {
        self.size = (w, h);
    }
}
