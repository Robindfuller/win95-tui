// Media Player, laid out like the one in Windows 95: the picture, a track bar
// with its scale, and the row of transport buttons. A mockup for now: the
// "video" is drifting clouds drawn here, not a file.
use super::{Action, App, Launch};
use crate::{
    draw::{st, Canvas},
    icons::Icon,
    menu::{Cmd, Item, Sys},
    theme::Theme,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::style::Color;
use std::time::{Duration, Instant};

const LENGTH: f64 = 90.0;
const FPS: f64 = 15.0;
const STEP: f64 = 5.0;

#[derive(Clone, Copy, PartialEq)]
enum Scale {
    Time,
    Frames,
}

pub struct MediaPlayer {
    size: (u16, u16),
    pos: f64,
    playing: bool,
    repeat: bool,
    scale: Scale,
    /// the selection, from mark in to mark out
    sel: (Option<f64>, Option<f64>),
    last: Instant,
    drawn: Instant,
    /// the button held down, and dragging the track bar's thumb
    pressed: Option<&'static str>,
    dragging: bool,
    /// where the buttons and track bar were drawn, for clicks
    buttons: Vec<(i32, i32, &'static str)>,
    track: (i32, i32, i32),
}

/// Transport buttons: label, width, command. None is a gap.
const BUTTONS: [Option<(&str, i32, &str)>; 11] = [
    Some(("▶", 4, "play")),
    Some(("■", 4, "stop")),
    Some(("▲", 4, "eject")),
    None,
    Some(("|◀", 4, "prev")),
    Some(("◀◀", 4, "rew")),
    Some(("▶▶", 4, "ffwd")),
    Some(("▶|", 4, "next")),
    None,
    Some(("[▼", 4, "in")),
    Some(("▼]", 4, "out")),
];

impl MediaPlayer {
    pub fn new() -> MediaPlayer {
        let now = Instant::now();
        MediaPlayer {
            size: (66, 28),
            pos: 0.0,
            playing: true,
            repeat: false,
            scale: Scale::Time,
            sel: (None, None),
            last: now,
            drawn: now,
            pressed: None,
            dragging: false,
            buttons: vec![],
            track: (0, 0, 0),
        }
    }

    fn seek(&mut self, to: f64) {
        self.pos = to.clamp(0.0, LENGTH);
        self.last = Instant::now();
    }

    fn clock(t: f64) -> String {
        let t = t.max(0.0);
        format!("{:02}:{:04.1}", (t / 60.0) as u32, t % 60.0)
    }

    fn readout(&self) -> String {
        match self.scale {
            Scale::Time => format!("{} / {}", Self::clock(self.pos), Self::clock(LENGTH)),
            Scale::Frames => format!("{} / {}", (self.pos * FPS) as u32, (LENGTH * FPS) as u32),
        }
    }

    /// The picture's size in pixels inside w by h cells, 4:3.
    fn fit(w: i32, h: i32) -> (i32, i32) {
        let (mut pw, mut ph) = (w, w * 3 / 4);
        if ph > h * 2 {
            ph = h * 2;
            pw = ph * 4 / 3;
        }
        (pw, ph)
    }

    fn picture(&self, c: &mut Canvas, x: i32, y: i32, w: i32, h: i32) {
        let (pw, ph) = Self::fit(w, h);
        if pw <= 0 || ph <= 0 {
            return;
        }
        let (ox, oy) = (x + (w - pw) / 2, y + (h - (ph + 1) / 2) / 2);
        let t = self.pos;
        let px = |px: i32, py: i32| -> Color {
            let (u, v) = (px as f64 / pw as f64, py as f64 / ph as f64);
            // sky from deep blue at the top to pale at the horizon
            let sky = (
                lerp(30.0, 130.0, v),
                lerp(70.0, 175.0, v),
                lerp(190.0, 245.0, v),
            );
            let n = fbm(u * 3.2 + t * 0.06, v * 2.4 + t * 0.01);
            let cloud = smooth(0.48, 0.72, n);
            // the undersides of clouds a little grey
            let shade = 1.0 - 0.25 * smooth(0.0, 0.08, fbm(u * 3.2 + t * 0.06, v * 2.4 + t * 0.01 - 0.06) - n).max(0.0);
            let white = 250.0 * shade;
            Color::Rgb(
                lerp(sky.0, white, cloud) as u8,
                lerp(sky.1, white, cloud) as u8,
                lerp(sky.2, white + 5.0 * shade, cloud).min(255.0) as u8,
            )
        };
        for row in (0..ph).step_by(2) {
            for col in 0..pw {
                let top = px(col, row);
                let bot = if row + 1 < ph { px(col, row + 1) } else { Color::Rgb(0, 0, 0) };
                c.put(ox + col, oy + row / 2, "▀", st(top, bot));
            }
        }
    }

    fn run(&mut self, cmd: &str) -> Action {
        match cmd {
            "play" => {
                if !self.playing && self.pos >= LENGTH {
                    self.seek(0.0);
                }
                self.playing = !self.playing;
                self.last = Instant::now();
            }
            "stop" => {
                self.playing = false;
                self.seek(0.0);
            }
            "rew" => self.seek(self.pos - STEP),
            "ffwd" => self.seek(self.pos + STEP),
            "prev" => self.seek(match self.sel.0 {
                Some(a) if self.pos > a + 0.5 => a,
                _ => 0.0,
            }),
            "next" => self.seek(match self.sel.1 {
                Some(b) if self.pos < b - 0.5 => b,
                _ => LENGTH,
            }),
            "in" => self.sel.0 = Some(self.pos),
            "out" => self.sel.1 = Some(self.pos),
            "clear" => self.sel = (None, None),
            "repeat" => self.repeat = !self.repeat,
            "time" => self.scale = Scale::Time,
            "frames" => self.scale = Scale::Frames,
            "eject" => {}
            "open" | "props" | "volume" | "device" => {
                return Action::Launch(Launch::Msg {
                    title: "Media Player".into(),
                    text: "This is a mockup, so it only plays the clouds for now.".into(),
                })
            }
            "about" => {
                return Action::Launch(Launch::Msg {
                    title: "About Media Player".into(),
                    text: "Media Player\n\nPlays video in half blocks.\nSpace plays and pauses, arrows step, Home and End jump.".into(),
                })
            }
            _ => {}
        }
        Action::None
    }
}

fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t.clamp(0.0, 1.0)
}

fn smooth(a: f64, b: f64, x: f64) -> f64 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn hash(x: i64, y: i64) -> f64 {
    let mut h = (x.wrapping_mul(374761393) ^ y.wrapping_mul(668265263)) as u64;
    h = (h ^ (h >> 13)).wrapping_mul(1274126177);
    ((h ^ (h >> 16)) & 0xffff) as f64 / 65535.0
}

fn noise(x: f64, y: f64) -> f64 {
    let (xi, yi) = (x.floor(), y.floor());
    let (fx, fy) = (x - xi, y - yi);
    let (sx, sy) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
    let (xi, yi) = (xi as i64, yi as i64);
    let a = lerp(hash(xi, yi), hash(xi + 1, yi), sx);
    let b = lerp(hash(xi, yi + 1), hash(xi + 1, yi + 1), sx);
    lerp(a, b, sy)
}

fn fbm(x: f64, y: f64) -> f64 {
    noise(x, y) * 0.55 + noise(x * 2.1, y * 2.1) * 0.3 + noise(x * 4.3, y * 4.3) * 0.15
}

impl App for MediaPlayer {
    fn title(&self) -> String {
        "clouds.mpg - Media Player".into()
    }

    fn tab_title(&self) -> String {
        "Media Player".into()
    }

    fn icon(&self) -> Icon {
        Icon::MediaPlayer
    }

    fn size_hint(&self) -> (u16, u16) {
        (66, 28)
    }

    fn render(&mut self, c: &mut Canvas, th: &Theme, _focused: bool) {
        let (cw, ch) = (self.size.0 as i32, self.size.1 as i32);
        c.fill(0, 0, cw, ch, st(th.text, th.face));

        // the picture, in a sunken frame
        let vh = (ch - 5).max(3);
        c.bevel(0, 0, cw, vh, th, false);
        let black = Color::Rgb(0, 0, 0);
        c.fill(1, 1, cw - 2, vh - 2, st(th.text, black));
        self.picture(c, 1, 1, cw - 2, vh - 2);

        // the track bar: a groove, the selection, and the thumb
        let ty = vh;
        let (x0, x1) = (2, cw - 3);
        let span = (x1 - x0).max(1);
        let at = |t: f64| x0 + ((t / LENGTH) * span as f64).round() as i32;
        for x in x0..=x1 {
            c.put(x, ty, "━", st(th.shadow, th.face));
        }
        if let (Some(a), Some(b)) = self.sel {
            let (a, b) = if a <= b { (a, b) } else { (b, a) };
            for x in at(a)..=at(b) {
                c.put(x, ty, "━", st(th.accent, th.face));
            }
            c.put(at(a), ty + 1, "▼", st(th.text, th.face));
            c.put(at(b), ty + 1, "▼", st(th.text, th.face));
        }
        let tx = at(self.pos);
        c.button(tx - 1, ty, 3, "", th, false, self.dragging);
        self.track = (x0, x1, ty);

        // the scale: ticks and their numbers
        let ticks = 9;
        for i in 0..=ticks {
            let t = LENGTH * i as f64 / ticks as f64;
            let x = at(t);
            if self.sel.0.is_none_or(|a| at(a) != x) && self.sel.1.is_none_or(|b| at(b) != x) {
                c.put(x, ty + 1, "╵", st(th.text, th.face));
            }
            if i % 3 == 0 {
                let label = match self.scale {
                    Scale::Time => format!("{}:{:02}", t as u32 / 60, t as u32 % 60),
                    Scale::Frames => format!("{}", (t * FPS) as u32),
                };
                let lx = (x - label.chars().count() as i32 / 2).clamp(0, cw - label.chars().count() as i32);
                c.text(lx, ty + 2, &label, st(th.text, th.face));
            }
        }

        // the transport buttons
        let by = ch - 1;
        let mut x = 1;
        self.buttons.clear();
        for b in BUTTONS {
            let Some((label, w, cmd)) = b else {
                x += 1;
                continue;
            };
            let label = if cmd == "play" && self.playing { "❚❚" } else { label };
            c.button(x, by, w, label, th, false, self.pressed == Some(cmd));
            if cmd == "eject" {
                // no disc to eject from a file
                c.text(x + 1, by, " ▲", st(th.dim, th.button));
            }
            self.buttons.push((x, w, cmd));
            x += w;
        }

        // where it is, in a sunken box at the right
        let read = self.readout();
        let rw = read.chars().count() as i32 + 2;
        if x + 1 + rw <= cw {
            c.field(cw - rw - 1, by, rw, &read, th);
        }
    }

    fn key(&mut self, k: KeyEvent) -> Action {
        match k.code {
            KeyCode::Char(' ') | KeyCode::Enter => return self.run("play"),
            KeyCode::Char('s') => return self.run("stop"),
            KeyCode::Left => return self.run("rew"),
            KeyCode::Right => return self.run("ffwd"),
            KeyCode::Home => self.seek(0.0),
            KeyCode::End => self.seek(LENGTH),
            KeyCode::Char('i') => return self.run("in"),
            KeyCode::Char('o') => return self.run("out"),
            _ => {}
        }
        Action::None
    }

    fn mouse(&mut self, kind: MouseEventKind, x: i32, y: i32, _mods: KeyModifiers) -> Action {
        let (x0, x1, ty) = self.track;
        let to_time = |x: i32| (x - x0) as f64 / (x1 - x0).max(1) as f64 * LENGTH;
        match kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if y == ty && x >= x0 - 1 && x <= x1 + 1 {
                    self.dragging = true;
                    self.seek(to_time(x));
                } else if y == self.size.1 as i32 - 1 {
                    if let Some(&(_, _, cmd)) = self.buttons.iter().find(|(bx, bw, _)| x >= *bx && x < bx + bw) {
                        self.pressed = Some(cmd);
                        return self.run(cmd);
                    }
                }
            }
            MouseEventKind::Drag(MouseButton::Left) if self.dragging => self.seek(to_time(x)),
            MouseEventKind::Up(_) => {
                self.dragging = false;
                self.pressed = None;
            }
            MouseEventKind::ScrollUp => self.seek(self.pos + 1.0),
            MouseEventKind::ScrollDown => self.seek(self.pos - 1.0),
            _ => {}
        }
        Action::None
    }

    fn resize(&mut self, w: u16, h: u16) {
        self.size = (w, h);
    }

    fn poll(&mut self) -> (bool, Action) {
        let now = Instant::now();
        if self.playing && !self.dragging {
            self.pos += now.duration_since(self.last).as_secs_f64();
            if self.pos >= LENGTH {
                if self.repeat {
                    self.pos = 0.0;
                } else {
                    self.pos = LENGTH;
                    self.playing = false;
                }
            }
        }
        self.last = now;
        if self.playing && now.duration_since(self.drawn) >= Duration::from_secs_f64(1.0 / FPS) {
            self.drawn = now;
            return (true, Action::None);
        }
        (false, Action::None)
    }

    fn menubar(&self) -> Vec<(&'static str, Vec<Item>)> {
        vec![
            ("File", vec![
                Item::new("Open...", Cmd::App("open")).key("Ctrl+O"),
                Item::sep(),
                Item::new("Exit", Cmd::Sys(Sys::Close)),
            ]),
            ("Edit", vec![
                Item::new("Mark in", Cmd::App("in")).key("I"),
                Item::new("Mark out", Cmd::App("out")).key("O"),
                Item::new("Clear selection", Cmd::App("clear")).enabled(self.sel != (None, None)),
                Item::sep(),
                Item::new("Auto repeat", Cmd::App("repeat")).checked(self.repeat),
            ]),
            ("Device", vec![
                Item::new("Properties...", Cmd::App("props")),
                Item::new("Volume Control...", Cmd::App("volume")),
                Item::sep(),
                Item::new("1 ActiveMovie...", Cmd::App("device")),
                Item::new("2 Video for Windows...", Cmd::App("device")),
                Item::new("3 Sound...", Cmd::App("device")),
                Item::new("4 MIDI Sequencer...", Cmd::App("device")),
                Item::new("5 CD Audio", Cmd::App("device")),
            ]),
            ("Scale", vec![
                Item::new("Time", Cmd::App("time")).checked(self.scale == Scale::Time),
                Item::new("Frames", Cmd::App("frames")).checked(self.scale == Scale::Frames),
            ]),
            ("Help", vec![Item::new("About Media Player", Cmd::App("about")).icon(Icon::Help)]),
        ]
    }

    fn command(&mut self, cmd: &str) -> Action {
        self.run(cmd)
    }
}
