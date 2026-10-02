// Paint: a little bitmap editor. Each pixel is two cells wide so it comes out
// square and the mouse can hit every one. Left button paints in the first
// colour, right in the second, and pictures save as .bmp.
use super::{notepad::expand, Action, App, Launch};
use crate::{
    draw::{st, Canvas},
    icons::Icon,
    menu::{Cmd, Item, Sys},
    theme::{home, Theme},
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind};
use image::{ImageFormat, RgbImage};
use ratatui::style::{Color, Style};
use std::path::PathBuf;

type Rgb = [u8; 3];

/// The Windows 95 Paint colours.
const PALETTE: [Rgb; 16] = [
    [0, 0, 0], [128, 128, 128], [128, 0, 0], [128, 128, 0], [0, 128, 0], [0, 128, 128], [0, 0, 128], [128, 0, 128],
    [255, 255, 255], [192, 192, 192], [255, 0, 0], [255, 255, 0], [0, 255, 0], [0, 255, 255], [0, 0, 255], [255, 0, 255],
];

#[derive(Clone, Copy, PartialEq, Debug)]
enum Tool {
    Pencil,
    Brush,
    Eraser,
    Fill,
    Line,
    Rect,
}

const TOOLS: [(Tool, &str, &str); 6] = [
    (Tool::Pencil, "✎", "Pencil"),
    (Tool::Brush, "●", "Brush"),
    (Tool::Eraser, "▭", "Eraser"),
    (Tool::Fill, "▼", "Fill"),
    (Tool::Line, "╱", "Line"),
    (Tool::Rect, "□", "Rectangle"),
];

/// Left of the picture: the tools, one per row.
const TOOLBOX: i32 = 4;
/// Below the picture: two rows of colours and the status line.
const FOOT: i32 = 3;

pub struct Paint {
    img: RgbImage,
    path: Option<PathBuf>,
    modified: bool,
    tool: Tool,
    /// left and right button colours
    fg: Rgb,
    bg: Rgb,
    /// top-left pixel in view
    scroll: (i32, i32),
    /// a stroke in progress: the button's colour and the last pixel, or a
    /// line or box being dragged out from its start
    stroke: Option<(Rgb, (i32, i32), (i32, i32))>,
    undo: Vec<RgbImage>,
    hover: Option<(i32, i32)>,
    /// typing a file name to save as
    prompt: Option<String>,
    status: Option<String>,
    size: (u16, u16),
}

impl Paint {
    pub fn new(path: Option<PathBuf>) -> Paint {
        let mut status = None;
        let img = match &path {
            Some(p) if p.exists() => match image::open(p) {
                Ok(i) => i.to_rgb8(),
                Err(e) => {
                    status = Some(format!("Can't open it: {e}"));
                    blank(40, 20)
                }
            },
            _ => blank(40, 20),
        };
        Paint {
            img,
            path,
            modified: false,
            tool: Tool::Pencil,
            fg: PALETTE[0],
            bg: PALETTE[8],
            scroll: (0, 0),
            stroke: None,
            undo: vec![],
            hover: None,
            prompt: None,
            status,
            size: (88, 26),
        }
    }

    fn view(&self) -> (i32, i32) {
        ((self.size.0 as i32 - TOOLBOX) / 2, self.size.1 as i32 - FOOT)
    }

    /// The pixel under a cell of the window, if it's on the picture.
    fn pixel_at(&self, x: i32, y: i32) -> Option<(i32, i32)> {
        let (vw, vh) = self.view();
        if x < TOOLBOX || y < 0 || y >= vh || (x - TOOLBOX) / 2 >= vw {
            return None;
        }
        let (px, py) = ((x - TOOLBOX) / 2 + self.scroll.0, y + self.scroll.1);
        (px < self.img.width() as i32 && py < self.img.height() as i32).then_some((px, py))
    }

    fn set(&mut self, x: i32, y: i32, c: Rgb, size: i32) {
        for dy in 0..size {
            for dx in 0..size {
                let (px, py) = (x + dx - size / 2, y + dy - size / 2);
                if px >= 0 && py >= 0 && (px as u32) < self.img.width() && (py as u32) < self.img.height() {
                    self.img.put_pixel(px as u32, py as u32, image::Rgb(c));
                }
            }
        }
    }

    fn line(&mut self, a: (i32, i32), b: (i32, i32), c: Rgb, size: i32) {
        for (x, y) in line_points(a, b) {
            self.set(x, y, c, size);
        }
    }

    fn fill(&mut self, x: i32, y: i32, c: Rgb) {
        let from = self.img.get_pixel(x as u32, y as u32).0;
        if from == c {
            return;
        }
        let mut todo = vec![(x, y)];
        let (w, h) = (self.img.width() as i32, self.img.height() as i32);
        while let Some((x, y)) = todo.pop() {
            if x < 0 || y < 0 || x >= w || y >= h || self.img.get_pixel(x as u32, y as u32).0 != from {
                continue;
            }
            self.img.put_pixel(x as u32, y as u32, image::Rgb(c));
            todo.extend([(x + 1, y), (x - 1, y), (x, y + 1), (x, y - 1)]);
        }
    }

    fn brush(&self) -> i32 {
        match self.tool {
            Tool::Brush => 2,
            Tool::Eraser => 3,
            _ => 1,
        }
    }

    fn press(&mut self, p: (i32, i32), right: bool) {
        self.undo.push(self.img.clone());
        if self.undo.len() > 30 {
            self.undo.remove(0);
        }
        let c = if right { self.bg } else { self.fg };
        let c = if self.tool == Tool::Eraser { self.bg } else { c };
        match self.tool {
            Tool::Fill => {
                self.fill(p.0, p.1, c);
                self.modified = true;
                return;
            }
            Tool::Line | Tool::Rect => {}
            _ => {
                self.set(p.0, p.1, c, self.brush());
                self.modified = true;
            }
        }
        self.stroke = Some((c, p, p));
    }

    fn drag(&mut self, p: (i32, i32)) {
        let Some((c, start, last)) = self.stroke else { return };
        match self.tool {
            Tool::Line | Tool::Rect => self.stroke = Some((c, start, p)),
            Tool::Fill => {}
            _ => {
                let b = self.brush();
                self.line(last, p, c, b);
                self.stroke = Some((c, start, p));
            }
        }
    }

    fn release(&mut self) {
        let Some((c, a, b)) = self.stroke.take() else { return };
        match self.tool {
            Tool::Line => self.line(a, b, c, 1),
            Tool::Rect => {
                for (p, q) in box_sides(a, b) {
                    self.line(p, q, c, 1);
                }
            }
            _ => return,
        }
        self.modified = true;
    }

    fn save(&mut self) -> Action {
        let Some(p) = self.path.clone() else {
            self.prompt = Some(tidy(&default_path()));
            return Action::None;
        };
        self.status = Some(match self.img.save_with_format(&p, ImageFormat::Bmp) {
            Ok(_) => {
                self.modified = false;
                format!("Saved {}", tidy(&p))
            }
            Err(e) => format!("Can't save: {e}"),
        });
        Action::None
    }

    fn keep_in_view(&mut self) {
        let (vw, vh) = self.view();
        let (mx, my) = ((self.img.width() as i32 - vw).max(0), (self.img.height() as i32 - vh).max(0));
        self.scroll = (self.scroll.0.clamp(0, mx), self.scroll.1.clamp(0, my));
    }
}

fn blank(w: u32, h: u32) -> RgbImage {
    RgbImage::from_pixel(w, h, image::Rgb([255, 255, 255]))
}

fn tidy(p: &std::path::Path) -> String {
    let s = p.to_string_lossy().to_string();
    let h = home().to_string_lossy().to_string();
    s.strip_prefix(&h).map(|r| format!("~{r}")).unwrap_or(s)
}

/// ~/Pictures/untitled.bmp, or (2), (3)... if that's taken.
fn default_path() -> PathBuf {
    let dir = Some(home().join("Pictures")).filter(|d| d.is_dir()).unwrap_or_else(home);
    let mut p = dir.join("untitled.bmp");
    let mut i = 1;
    while p.exists() {
        i += 1;
        p = dir.join(format!("untitled ({i}).bmp"));
    }
    p
}

fn line_points(a: (i32, i32), b: (i32, i32)) -> Vec<(i32, i32)> {
    let (dx, dy) = ((b.0 - a.0).abs(), -(b.1 - a.1).abs());
    let (sx, sy) = ((b.0 - a.0).signum(), (b.1 - a.1).signum());
    let (mut x, mut y, mut err) = (a.0, a.1, dx + dy);
    let mut v = vec![];
    loop {
        v.push((x, y));
        if (x, y) == b {
            return v;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
    }
}

fn box_sides(a: (i32, i32), b: (i32, i32)) -> [((i32, i32), (i32, i32)); 4] {
    [(a, (b.0, a.1)), ((b.0, a.1), b), (b, (a.0, b.1)), ((a.0, b.1), a)]
}

fn col(c: Rgb) -> Color {
    Color::Rgb(c[0], c[1], c[2])
}

impl App for Paint {
    fn title(&self) -> String {
        let name = self.path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or("untitled".into());
        format!("{}{} - Paint", name, if self.modified { "*" } else { "" })
    }
    fn icon(&self) -> Icon {
        Icon::Paint
    }
    fn size_hint(&self) -> (u16, u16) {
        let w = (self.img.width() as u16 * 2 + TOOLBOX as u16).clamp(40, 120);
        let h = (self.img.height() as u16 + FOOT as u16).clamp(12, 40);
        (w, h)
    }

    fn render(&mut self, c: &mut Canvas, th: &Theme, _focused: bool) {
        let (w, h) = (self.size.0 as i32, self.size.1 as i32);
        let face = st(th.text, th.face);
        c.fill(0, 0, w, h, face);
        // tools
        for (i, (t, g, _)) in TOOLS.iter().enumerate() {
            let s = if *t == self.tool { th.sel() } else { face };
            c.text(0, i as i32, &format!(" {g} "), s);
        }
        // the picture, and a line or box being dragged out
        let preview: Vec<(i32, i32)> = match (self.tool, self.stroke) {
            (Tool::Line, Some((_, a, b))) => line_points(a, b),
            (Tool::Rect, Some((_, a, b))) => box_sides(a, b).iter().flat_map(|&(p, q)| line_points(p, q)).collect(),
            _ => vec![],
        };
        let (vw, vh) = self.view();
        for y in 0..vh {
            for x in 0..vw {
                let (px, py) = (x + self.scroll.0, y + self.scroll.1);
                let cx = TOOLBOX + 2 * x;
                if px >= self.img.width() as i32 || py >= self.img.height() as i32 {
                    c.text(cx, y, "  ", st(th.dim, th.shadow));
                    continue;
                }
                let mut p = self.img.get_pixel(px as u32, py as u32).0;
                if preview.contains(&(px, py)) {
                    p = self.stroke.map_or(p, |s| s.0);
                }
                c.text(cx, y, "  ", Style::new().bg(col(p)));
            }
        }
        // the colours: the two in use, then the palette in two rows
        let py = h - FOOT;
        c.text(1, py, "██", Style::new().fg(col(self.fg)).bg(th.face));
        c.text(2, py + 1, "██", Style::new().fg(col(self.bg)).bg(th.face));
        for (i, p) in PALETTE.iter().enumerate() {
            let (x, y) = (TOOLBOX + 1 + 3 * (i as i32 % 8), py + i as i32 / 8);
            c.text(x, y, "██", Style::new().fg(col(*p)).bg(th.face));
        }
        // status: a file name being typed, a message, or the tool and pixel
        let line = if let Some(t) = &self.prompt {
            format!("Save as: {t}")
        } else if let Some(s) = &self.status {
            s.clone()
        } else {
            let name = TOOLS.iter().find(|t| t.0 == self.tool).map_or("", |t| t.2);
            let at = self.hover.map(|(x, y)| format!("  {x},{y}")).unwrap_or_default();
            format!("{name}{at}  {}×{}", self.img.width(), self.img.height())
        };
        let sx = TOOLBOX + 26;
        if self.prompt.is_some() {
            c.field(sx, py, w - sx - 1, &line, th);
        } else {
            c.text_max(sx, py, &line, st(th.dim, th.face), w - 1);
            c.text_max(sx, py + 1, "Left and right click: two colours", st(th.dim, th.face), w - 1);
        }
    }

    fn cursor(&self) -> Option<(u16, u16)> {
        let t = self.prompt.as_ref()?;
        let sx = TOOLBOX as u16 + 26;
        let room = (self.size.0 as usize).saturating_sub(sx as usize + 3);
        Some((sx + 1 + (t.chars().count() + 9).min(room) as u16, self.size.1 - FOOT as u16))
    }

    fn key(&mut self, k: KeyEvent) -> Action {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        if let Some(t) = &mut self.prompt {
            match k.code {
                KeyCode::Esc => self.prompt = None,
                KeyCode::Enter => {
                    let mut p = expand(t.trim());
                    if p.extension().is_none() {
                        p.set_extension("bmp");
                    }
                    self.prompt = None;
                    self.path = Some(p);
                    return self.save();
                }
                KeyCode::Backspace => {
                    t.pop();
                }
                KeyCode::Char('u') if ctrl => t.clear(),
                KeyCode::Char(ch) if !ctrl => t.push(ch),
                _ => {}
            }
            return Action::None;
        }
        self.status = None;
        match k.code {
            KeyCode::Char('s') if ctrl => return self.save(),
            KeyCode::Char('z') if ctrl => return self.command("undo"),
            KeyCode::Char('n') if ctrl => return self.command("new"),
            KeyCode::Up => self.scroll.1 -= 3,
            KeyCode::Down => self.scroll.1 += 3,
            KeyCode::Left => self.scroll.0 -= 3,
            KeyCode::Right => self.scroll.0 += 3,
            KeyCode::Char(ch) => {
                if let Some(t) = TOOLS.iter().find(|t| t.2.to_lowercase().starts_with(ch)) {
                    self.tool = t.0;
                }
            }
            _ => {}
        }
        self.keep_in_view();
        Action::None
    }

    fn paste(&mut self, s: &str) {
        if let Some(t) = &mut self.prompt {
            t.push_str(s.lines().next().unwrap_or(""));
        }
    }

    fn mouse(&mut self, kind: MouseEventKind, x: i32, y: i32, mods: KeyModifiers) -> Action {
        let (w, h) = (self.size.0 as i32, self.size.1 as i32);
        self.hover = self.pixel_at(x, y);
        match kind {
            MouseEventKind::Down(b) if b != MouseButton::Middle => {
                let right = b == MouseButton::Right;
                self.status = None;
                if let Some(p) = self.pixel_at(x, y) {
                    self.press(p, right);
                } else if x < TOOLBOX && (0..TOOLS.len() as i32).contains(&y) {
                    self.tool = TOOLS[y as usize].0;
                } else if y >= h - FOOT && y < h - 1 && x > TOOLBOX && (x - TOOLBOX - 1) % 3 < 2 {
                    let i = (y - (h - FOOT)) * 8 + (x - TOOLBOX - 1) / 3;
                    if let Some(&p) = PALETTE.get(i as usize).filter(|_| (x - TOOLBOX - 1) / 3 < 8) {
                        if right { self.bg = p } else { self.fg = p }
                    }
                }
            }
            MouseEventKind::Drag(_) => {
                // keep within the picture so a stroke off the edge still ends there
                let (vw, vh) = self.view();
                let cx = x.clamp(TOOLBOX, TOOLBOX + 2 * vw - 1);
                let cy = y.clamp(0, vh - 1);
                if let Some(p) = self.pixel_at(cx, cy).or(self.stroke.map(|s| s.2)) {
                    self.drag(p);
                }
            }
            MouseEventKind::Up(_) => self.release(),
            MouseEventKind::ScrollUp if mods.contains(KeyModifiers::SHIFT) => self.scroll.0 -= 3,
            MouseEventKind::ScrollDown if mods.contains(KeyModifiers::SHIFT) => self.scroll.0 += 3,
            MouseEventKind::ScrollUp => self.scroll.1 -= 3,
            MouseEventKind::ScrollDown => self.scroll.1 += 3,
            _ => {}
        }
        let _ = w;
        self.keep_in_view();
        Action::None
    }

    fn resize(&mut self, w: u16, h: u16) {
        self.size = (w, h);
        self.keep_in_view();
    }

    fn menubar(&self) -> Vec<(&'static str, Vec<Item>)> {
        vec![
            ("File", vec![
                Item::new("New", Cmd::App("new")).key("Ctrl+N"),
                Item::new("Open...", Cmd::App("open")),
                Item::new("Save", Cmd::App("save")).key("Ctrl+S"),
                Item::new("Save As...", Cmd::App("saveas")),
                Item::sep(),
                Item::new("Exit", Cmd::Sys(Sys::Close)),
            ]),
            ("Edit", vec![
                Item::new("Undo", Cmd::App("undo")).key("Ctrl+Z").enabled(!self.undo.is_empty()),
                Item::new("Clear Picture", Cmd::App("clear")),
            ]),
            ("Image", vec![
                Item::new("Bigger", Cmd::App("bigger")),
                Item::new("Smaller", Cmd::App("smaller")),
            ]),
        ]
    }

    fn command(&mut self, cmd: &str) -> Action {
        match cmd {
            "new" => return Action::Launch(Launch::Paint(None)),
            "open" => {
                let dir = self.path.as_ref().and_then(|p| p.parent().map(|d| d.to_path_buf())).unwrap_or_else(|| default_path().parent().unwrap().to_path_buf());
                return Action::Launch(Launch::Explorer(dir));
            }
            "save" => return self.save(),
            "saveas" => self.prompt = Some(tidy(&self.path.clone().unwrap_or_else(default_path))),
            "undo" => {
                if let Some(i) = self.undo.pop() {
                    self.img = i;
                    self.modified = true;
                }
            }
            "clear" => {
                self.undo.push(self.img.clone());
                self.img = RgbImage::from_pixel(self.img.width(), self.img.height(), image::Rgb(self.bg));
                self.modified = true;
            }
            "bigger" | "smaller" => {
                // grows or shrinks the canvas by a quarter, keeping the top-left
                let f = if cmd == "bigger" { 1.25 } else { 0.8 };
                let (nw, nh) = (((self.img.width() as f32 * f) as u32).clamp(4, 400), ((self.img.height() as f32 * f) as u32).clamp(4, 300));
                self.undo.push(self.img.clone());
                let mut n = RgbImage::from_pixel(nw, nh, image::Rgb(self.bg));
                image::imageops::replace(&mut n, &self.img, 0, 0);
                self.img = n;
                self.modified = true;
                self.keep_in_view();
            }
            _ => {}
        }
        Action::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draws_fills_saves_and_opens() {
        let mut p = Paint::new(None);
        p.size = (88, 26);
        let l = MouseButton::Left;
        // a pencil stroke across, with no gaps even when the mouse jumps
        p.mouse(MouseEventKind::Down(l), TOOLBOX, 2, KeyModifiers::NONE);
        p.mouse(MouseEventKind::Drag(l), TOOLBOX + 2 * 9, 2, KeyModifiers::NONE);
        p.mouse(MouseEventKind::Up(l), TOOLBOX + 2 * 9, 2, KeyModifiers::NONE);
        assert!((0..10).all(|x| p.img.get_pixel(x, 2).0 == [0, 0, 0]));
        // a box, then fill its inside red with the right button
        p.fg = PALETTE[10];
        p.tool = Tool::Rect;
        p.mouse(MouseEventKind::Down(l), TOOLBOX + 2 * 20, 5, KeyModifiers::NONE);
        p.mouse(MouseEventKind::Drag(l), TOOLBOX + 2 * 25, 9, KeyModifiers::NONE);
        p.mouse(MouseEventKind::Up(l), TOOLBOX + 2 * 25, 9, KeyModifiers::NONE);
        assert_eq!(p.img.get_pixel(20, 5).0, PALETTE[10]);
        assert_eq!(p.img.get_pixel(25, 9).0, PALETTE[10]);
        p.bg = PALETTE[12];
        p.tool = Tool::Fill;
        p.mouse(MouseEventKind::Down(MouseButton::Right), TOOLBOX + 2 * 22, 7, KeyModifiers::NONE);
        assert_eq!(p.img.get_pixel(22, 7).0, PALETTE[12]);
        assert_eq!(p.img.get_pixel(30, 7).0, [255, 255, 255], "the fill stays inside the box");
        // undo takes the fill back off
        p.command("undo");
        assert_eq!(p.img.get_pixel(22, 7).0, [255, 255, 255]);
        // save as a bitmap and open it again
        let f = std::env::temp_dir().join(format!("win95-paint-{}.bmp", std::process::id()));
        p.path = Some(f.clone());
        p.save();
        let q = Paint::new(Some(f.clone()));
        assert_eq!(q.img.get_pixel(20, 5).0, PALETTE[10]);
        assert_eq!(q.img.dimensions(), (40, 20));
        let _ = std::fs::remove_file(f);
    }
}
