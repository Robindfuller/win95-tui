// Media Player, laid out like the one in Windows 95: the picture, a track bar
// with its scale, and the row of transport buttons. ffmpeg decodes the
// picture into small RGB frames, drawn here in half blocks like Doom, and mpv
// plays the sound. Pausing or seeking stops both and starts them again from
// the new place, which keeps them together without any talking between them.
use super::{Action, App, Launch};
use crate::{
    draw::{st, Canvas},
    icons::Icon,
    menu::{Cmd, Item, Sys},
    theme::{home, Theme},
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::style::Color;
use std::{
    io::Read,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

/// Frames are decoded no bigger than this; a terminal window shows fewer pixels.
const MAX_W: f64 = 320.0;
const MAX_H: f64 = 240.0;
const STEP: f64 = 5.0;

const VIDEO: [&str; 16] = ["mpg", "mpeg", "mp4", "m4v", "mkv", "avi", "mov", "webm", "wmv", "flv", "ogv", "ts", "m2ts", "mts", "3gp", "vob"];

/// Whether a path is a video Media Player plays.
pub fn plays(p: &Path) -> bool {
    p.extension().is_some_and(|e| VIDEO.contains(&e.to_string_lossy().to_lowercase().as_str()))
}

#[derive(Clone, Copy, PartialEq)]
enum Scale {
    Time,
    Frames,
}

/// What ffprobe says about a file, and the size its frames are decoded at.
struct Media {
    path: PathBuf,
    w: usize,
    h: usize,
    /// width over height, as it's meant to be shown
    aspect: f64,
    fps: f64,
    /// in seconds; 0 when it isn't known
    length: f64,
    sound: bool,
    info: String,
}

fn probe(path: &Path) -> Result<Media, String> {
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-show_entries", "stream=codec_type,codec_name,width,height,avg_frame_rate,r_frame_rate,sample_aspect_ratio:format=duration", "-of", "json"])
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .map_err(|_| "Media Player needs ffmpeg (and ffprobe with it) to play video.".to_string())?;
    let j: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_default();
    let ratio = |s: &str, sep: char| -> f64 {
        match s.split_once(sep) {
            Some((a, b)) => a.parse::<f64>().unwrap_or(0.0) / b.parse::<f64>().unwrap_or(1.0).max(1e-9),
            None => s.parse().unwrap_or(0.0),
        }
    };
    let streams = j["streams"].as_array().cloned().unwrap_or_default();
    let sound = streams.iter().any(|s| s["codec_type"] == "audio");
    let Some(v) = streams.iter().find(|s| s["codec_type"] == "video" && s["width"].as_f64().unwrap_or(0.0) > 0.0) else {
        let err = String::from_utf8_lossy(&out.stderr);
        let why = err.lines().last().unwrap_or("there's no picture in it.").to_string();
        return Err(format!("Couldn't play {}:\n{why}", label(path)));
    };
    let (w, h) = (v["width"].as_f64().unwrap_or(0.0), v["height"].as_f64().unwrap_or(1.0).max(1.0));
    // the average rate is the real one; MPEG files often double the other
    let rate = [&v["avg_frame_rate"], &v["r_frame_rate"]].iter().map(|r| ratio(r.as_str().unwrap_or("0"), '/')).find(|r| r.is_finite() && *r > 0.0).unwrap_or(0.0);
    let sar = ratio(v["sample_aspect_ratio"].as_str().unwrap_or("1:1"), ':');
    let sar = if sar > 0.0 { sar } else { 1.0 };
    let length = j["format"]["duration"].as_str().and_then(|d| d.parse().ok()).unwrap_or(0.0);
    let aspect = w * sar / h;
    let (dw, dh) = if aspect >= MAX_W / MAX_H { (MAX_W, MAX_W / aspect) } else { (MAX_H * aspect, MAX_H) };
    // never decode bigger than the file is
    let shrink = (w / dw).min(h / dh).min(1.0);
    let (dw, dh) = (((dw * shrink) as usize & !1).max(2), ((dh * shrink) as usize & !1).max(2));
    let fps = if rate > 0.0 { rate.min(30.0) } else { 25.0 };
    let info = format!(
        "{}\n\nPicture: {} by {}, {}, {:.2} frames a second\nSound: {}\nLength: {}",
        label(path),
        w as u32,
        h as u32,
        v["codec_name"].as_str().unwrap_or("?"),
        rate,
        if sound { "yes" } else { "none" },
        if length > 0.0 { short(length) } else { "unknown".into() },
    );
    Ok(Media { path: path.to_path_buf(), w: dw, h: dh, aspect, fps, length, sound, info })
}

fn label(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| p.display().to_string())
}

fn clock(t: f64) -> String {
    let t = t.max(0.0);
    let s = t as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{:02}:{:04.1}", s / 60, t % 60.0)
    }
}

fn short(t: f64) -> String {
    let s = t.max(0.0) as u64;
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

#[derive(Default)]
struct Shared {
    /// frame count and the latest frame
    frame: Mutex<(u64, Vec<u8>)>,
    ended: AtomicBool,
}

/// An ffmpeg turning the file into frames from one place, either in time with
/// the clock or just the one frame there.
struct Decoder {
    child: Child,
    shared: Arc<Shared>,
}

impl Decoder {
    fn start(m: &Media, from: f64, still: bool) -> Option<Decoder> {
        let mut cmd = Command::new("ffmpeg");
        cmd.args(["-hide_banner", "-loglevel", "error", "-nostdin", "-ss", &format!("{from:.3}"), "-i"])
            .arg(&m.path)
            .args(["-an", "-sn", "-vf", &format!("fps={},scale={}:{}:flags=area,setsar=1", m.fps, m.w, m.h), "-pix_fmt", "rgb24", "-f", "rawvideo"]);
        if still {
            cmd.args(["-frames:v", "1"]);
        }
        let mut child = cmd.arg("-").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
        let shared = Arc::new(Shared::default());
        let mut out = child.stdout.take()?;
        let (sh, size, fps) = (shared.clone(), m.w * m.h * 3, m.fps);
        thread::spawn(move || {
            let mut buf = vec![0u8; size];
            let t0 = Instant::now();
            let mut i = 0u32;
            while out.read_exact(&mut buf).is_ok() {
                if !still {
                    let due = t0 + Duration::from_secs_f64(i as f64 / fps);
                    let now = Instant::now();
                    if due > now {
                        thread::sleep(due - now);
                    }
                }
                i += 1;
                let mut f = sh.frame.lock().unwrap();
                f.0 += 1;
                std::mem::swap(&mut f.1, &mut buf);
                buf.resize(size, 0);
            }
            sh.ended.store(true, Ordering::SeqCst);
        });
        Some(Decoder { child, shared })
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// mpv playing just the sound, from one place.
struct Sound(Child);

impl Sound {
    fn start(m: &Media, from: f64) -> Option<Sound> {
        if !m.sound {
            return None;
        }
        Command::new("mpv")
            .args(["--no-video", "--no-terminal", "--really-quiet", "--no-config", &format!("--start={from:.3}")])
            .arg(&m.path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()
            .map(Sound)
    }
}

impl Drop for Sound {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

pub struct MediaPlayer {
    size: (u16, u16),
    media: Option<Media>,
    error: Option<String>,
    /// the last file it was asked to play, even one it couldn't
    tried: Option<PathBuf>,
    pos: f64,
    playing: bool,
    /// where playing started from, and when
    from: (f64, Instant),
    video: Option<Decoder>,
    sound: Option<Sound>,
    /// the frame on screen, and which of the decoder's it was
    frame: Vec<u8>,
    seen: u64,
    /// a still frame is wanted for the new place, once the last one is done
    want_still: bool,
    ticked: Instant,
    repeat: bool,
    scale: Scale,
    /// the selection, from mark in to mark out
    sel: (Option<f64>, Option<f64>),
    /// the button held down, and dragging the track bar's thumb
    pressed: Option<&'static str>,
    dragging: bool,
    /// it was playing when the thumb was picked up
    resume: bool,
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
    pub fn new(file: Option<PathBuf>) -> MediaPlayer {
        let now = Instant::now();
        let mut mp = MediaPlayer {
            size: (66, 28),
            media: None,
            error: None,
            tried: None,
            pos: 0.0,
            playing: false,
            from: (0.0, now),
            video: None,
            sound: None,
            frame: vec![],
            seen: 0,
            want_still: false,
            ticked: now,
            repeat: false,
            scale: Scale::Time,
            sel: (None, None),
            pressed: None,
            dragging: false,
            resume: false,
            buttons: vec![],
            track: (0, 0, 0),
        };
        if let Some(f) = file {
            mp.load(&f);
        }
        mp
    }

    fn load(&mut self, path: &Path) {
        self.halt();
        self.tried = Some(path.to_path_buf());
        (self.pos, self.sel, self.frame) = (0.0, (None, None), vec![]);
        match probe(path) {
            Ok(m) => {
                self.media = Some(m);
                self.error = None;
                self.play();
            }
            Err(e) => {
                self.media = None;
                self.error = Some(e);
            }
        }
    }

    fn length(&self) -> f64 {
        self.media.as_ref().map(|m| m.length).unwrap_or(0.0)
    }

    fn play(&mut self) {
        let Some(m) = &self.media else { return };
        if self.length() > 0.0 && self.pos >= self.length() - 0.05 {
            self.pos = 0.0;
        }
        self.video = Decoder::start(m, self.pos, false);
        self.sound = Sound::start(m, self.pos);
        self.seen = 0;
        self.want_still = false;
        self.from = (self.pos, Instant::now());
        self.playing = true;
    }

    fn halt(&mut self) {
        self.video = None;
        self.sound = None;
        self.playing = false;
    }

    /// Moves to a new place, carrying on playing if it was.
    fn seek(&mut self, to: f64) {
        let end = if self.length() > 0.0 { self.length() } else { f64::MAX };
        self.pos = to.clamp(0.0, end);
        if self.playing {
            self.play();
        } else {
            self.want_still = true;
        }
    }

    fn readout(&self) -> String {
        let fps = self.media.as_ref().map(|m| m.fps).unwrap_or(25.0);
        match self.scale {
            Scale::Time => format!("{} / {}", clock(self.pos), clock(self.length())),
            Scale::Frames => format!("{} / {}", (self.pos * fps) as u64, (self.length() * fps) as u64),
        }
    }

    /// The picture's size in pixels inside w by h cells.
    fn fit(&self, w: i32, h: i32) -> (i32, i32) {
        let aspect = self.media.as_ref().map(|m| m.aspect).unwrap_or(4.0 / 3.0);
        let (mut pw, mut ph) = (w, (w as f64 / aspect).round() as i32);
        if ph > h * 2 {
            ph = h * 2;
            pw = (ph as f64 * aspect).round() as i32;
        }
        (pw, ph)
    }

    fn picture(&self, c: &mut Canvas, x: i32, y: i32, w: i32, h: i32) {
        let Some(m) = &self.media else { return };
        if self.frame.len() != m.w * m.h * 3 {
            return;
        }
        let (pw, ph) = self.fit(w, h);
        if pw <= 0 || ph <= 0 {
            return;
        }
        let (ox, oy) = (x + (w - pw) / 2, y + (h - (ph + 1) / 2) / 2);
        let (fw, fh, f) = (m.w, m.h, &self.frame);
        let (pw, ph) = (pw as usize, ph as usize);
        // each pixel is the average of the frame's pixels under it
        let px = |px: usize, py: usize| -> Color {
            let (y0, x0) = (py * fh / ph, px * fw / pw);
            let y1 = ((py + 1) * fh / ph).max(y0 + 1).min(fh);
            let x1 = ((px + 1) * fw / pw).max(x0 + 1).min(fw);
            let (mut r, mut g, mut b) = (0u32, 0u32, 0u32);
            for sy in y0..y1 {
                for p in f[(sy * fw + x0) * 3..(sy * fw + x1) * 3].chunks_exact(3) {
                    r += p[0] as u32;
                    g += p[1] as u32;
                    b += p[2] as u32;
                }
            }
            let n = ((y1 - y0) * (x1 - x0)).max(1) as u32;
            Color::Rgb((r / n) as u8, (g / n) as u8, (b / n) as u8)
        };
        for row in (0..ph).step_by(2) {
            for col in 0..pw {
                let top = px(col, row);
                let bot = if row + 1 < ph { px(col, row + 1) } else { Color::Rgb(0, 0, 0) };
                c.put(ox + col as i32, oy + (row / 2) as i32, "▀", st(top, bot));
            }
        }
    }

    fn folder(&self) -> PathBuf {
        if let Some(d) = self.tried.as_ref().and_then(|p| p.parent()) {
            return d.to_path_buf();
        }
        let videos = home().join("Videos");
        if videos.is_dir() { videos } else { home() }
    }

    fn run(&mut self, cmd: &str) -> Action {
        match cmd {
            "play" => {
                if self.playing {
                    self.pos = self.from.0 + self.from.1.elapsed().as_secs_f64();
                    self.halt();
                } else {
                    self.play();
                }
            }
            "stop" => {
                self.halt();
                self.pos = 0.0;
                self.want_still = true;
            }
            "rew" => self.seek(self.pos - STEP),
            "ffwd" => self.seek(self.pos + STEP),
            "prev" => self.seek(match self.sel.0 {
                Some(a) if self.pos > a + 0.5 => a,
                _ => 0.0,
            }),
            "next" => {
                let to = match self.sel.1 {
                    Some(b) if self.pos < b - 0.5 => b,
                    _ => self.length(),
                };
                self.seek(to)
            }
            "in" if self.media.is_some() => self.sel.0 = Some(self.pos),
            "out" if self.media.is_some() => self.sel.1 = Some(self.pos),
            "clear" => self.sel = (None, None),
            "repeat" => self.repeat = !self.repeat,
            "time" => self.scale = Scale::Time,
            "frames" => self.scale = Scale::Frames,
            "eject" | "close" => {
                self.halt();
                (self.media, self.error, self.pos, self.sel, self.frame) = (None, None, 0.0, (None, None), vec![]);
            }
            "open" => return Action::Pick { dir: self.folder(), folder: false },
            "props" => {
                let text = self.media.as_ref().map(|m| m.info.clone()).unwrap_or_else(|| "No video open.".into());
                return Action::Launch(Launch::Msg { title: "Properties".into(), text });
            }
            "about" => {
                return Action::Launch(Launch::Msg {
                    title: "About Media Player".into(),
                    text: "Media Player\n\nPlays video in half blocks: ffmpeg for the picture, mpv for the sound.\n\nSpace plays and pauses, S stops, arrows step 5 seconds,\nHome and End jump, I and O mark a selection.".into(),
                })
            }
            _ => {}
        }
        Action::None
    }
}

impl App for MediaPlayer {
    fn title(&self) -> String {
        match &self.media {
            Some(m) => format!("{} - Media Player", label(&m.path)),
            None => "Media Player".into(),
        }
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
        if let Some(e) = &self.error {
            for (i, line) in e.lines().enumerate() {
                c.text_max(2, 1 + i as i32, line, st(th.text, black), cw - 1);
            }
        } else if self.media.is_none() {
            let hint = "File > Open... to choose a video";
            c.text(((cw - hint.chars().count() as i32) / 2).max(1), vh / 2, hint, st(th.dim, black));
        }
        self.picture(c, 1, 1, cw - 2, vh - 2);

        // the track bar: a groove, the selection, and the thumb
        let ty = vh;
        let (x0, x1) = (2, cw - 3);
        let span = (x1 - x0).max(1);
        let len = self.length();
        let at = |t: f64| x0 + if len > 0.0 { ((t / len).clamp(0.0, 1.0) * span as f64).round() as i32 } else { 0 };
        for x in x0..=x1 {
            c.put(x, ty, "━", st(th.shadow, th.face));
        }
        if let (Some(a), Some(b)) = self.sel {
            let (a, b) = if a <= b { (a, b) } else { (b, a) };
            for x in at(a)..=at(b) {
                c.put(x, ty, "━", st(th.accent, th.face));
            }
        }
        for m in [self.sel.0, self.sel.1].into_iter().flatten() {
            c.put(at(m), ty + 1, "▼", st(th.text, th.face));
        }
        if self.media.is_some() {
            c.button(at(self.pos) - 1, ty, 3, "", th, false, self.dragging);
        }
        self.track = (x0, x1, ty);

        // the scale: ticks and their numbers
        let ticks = 9;
        let fps = self.media.as_ref().map(|m| m.fps).unwrap_or(25.0);
        for i in 0..=ticks {
            let t = len * i as f64 / ticks as f64;
            let x = x0 + span * i / ticks;
            if c.bg_at(x, ty + 1) == th.face && ![self.sel.0, self.sel.1].into_iter().flatten().any(|m| at(m) == x) {
                c.put(x, ty + 1, "╵", st(th.text, th.face));
            }
            if i % 3 == 0 && len > 0.0 {
                let label = match self.scale {
                    Scale::Time => short(t),
                    Scale::Frames => format!("{}", (t * fps) as u64),
                };
                let n = label.chars().count() as i32;
                c.text((x - n / 2).clamp(0, (cw - n).max(0)), ty + 2, &label, st(th.text, th.face));
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
            if self.media.is_none() {
                c.text(x + 1, by, &format!("{label:^2}"), st(th.dim, th.button));
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
        if k.modifiers.contains(KeyModifiers::CONTROL) && k.code == KeyCode::Char('o') {
            return self.run("open");
        }
        match k.code {
            KeyCode::Char(' ') | KeyCode::Enter => return self.run("play"),
            KeyCode::Char('s') => return self.run("stop"),
            KeyCode::Left => return self.run("rew"),
            KeyCode::Right => return self.run("ffwd"),
            KeyCode::Home => self.seek(0.0),
            KeyCode::End => self.seek(self.length()),
            KeyCode::Char('i') => return self.run("in"),
            KeyCode::Char('o') => return self.run("out"),
            _ => {}
        }
        Action::None
    }

    fn mouse(&mut self, kind: MouseEventKind, x: i32, y: i32, _mods: KeyModifiers) -> Action {
        let (x0, x1, ty) = self.track;
        let len = self.length();
        let to_time = |x: i32| (x - x0) as f64 / (x1 - x0).max(1) as f64 * len;
        match kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if y == ty && x >= x0 - 1 && x <= x1 + 1 && self.media.is_some() && len > 0.0 {
                    // the picture follows the thumb while it's held, then
                    // playing carries on from where it's let go
                    self.dragging = true;
                    self.resume = self.playing;
                    self.halt();
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
                if self.dragging && self.resume {
                    self.play();
                }
                self.dragging = false;
                self.pressed = None;
            }
            MouseEventKind::ScrollUp => self.seek(self.pos + STEP),
            MouseEventKind::ScrollDown => self.seek(self.pos - STEP),
            _ => {}
        }
        Action::None
    }

    fn resize(&mut self, w: u16, h: u16) {
        self.size = (w, h);
    }

    fn poll(&mut self) -> (bool, Action) {
        let mut redraw = false;
        if self.playing {
            self.pos = self.from.0 + self.from.1.elapsed().as_secs_f64();
            let len = self.length();
            let done = self.video.as_ref().is_none_or(|v| v.shared.ended.load(Ordering::SeqCst));
            if (len > 0.0 && self.pos >= len) || (done && (len <= 0.0 || self.pos >= len - 1.0)) {
                self.halt();
                self.pos = if len > 0.0 { len } else { self.pos };
                if self.repeat {
                    self.pos = 0.0;
                    self.play();
                }
                redraw = true;
            }
            // the clock and thumb move a few times a second
            if self.ticked.elapsed() >= Duration::from_millis(200) {
                self.ticked = Instant::now();
                redraw = true;
            }
        } else if self.want_still && self.video.as_ref().is_none_or(|v| v.shared.ended.load(Ordering::SeqCst)) {
            // one frame at a time while the thumb is dragged
            self.want_still = false;
            self.seen = 0;
            self.video = self.media.as_ref().and_then(|m| Decoder::start(m, self.pos.min((m.length - 0.1).max(0.0)), true));
        }
        if let Some(v) = &self.video {
            let f = v.shared.frame.lock().unwrap();
            if f.0 != self.seen {
                self.seen = f.0;
                self.frame.clone_from(&f.1);
                redraw = true;
            }
        }
        (redraw, Action::None)
    }

    fn menubar(&self) -> Vec<(&'static str, Vec<Item>)> {
        let open = self.media.is_some();
        vec![
            ("File", vec![
                Item::new("Open...", Cmd::App("open")).key("Ctrl+O"),
                Item::new("Close", Cmd::App("close")).enabled(open),
                Item::sep(),
                Item::new("Exit", Cmd::Sys(Sys::Close)),
            ]),
            ("Edit", vec![
                Item::new("Mark in", Cmd::App("in")).key("I").enabled(open),
                Item::new("Mark out", Cmd::App("out")).key("O").enabled(open),
                Item::new("Clear selection", Cmd::App("clear")).enabled(self.sel != (None, None)),
                Item::sep(),
                Item::new("Auto repeat", Cmd::App("repeat")).checked(self.repeat),
            ]),
            ("Device", vec![Item::new("Properties...", Cmd::App("props")).enabled(open)]),
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

    /// A video picked with File > Open plays here; anything else opens in what it belongs to.
    fn open_more(&mut self, files: &[PathBuf]) -> Action {
        let Some(f) = files.first() else { return Action::None };
        if !plays(f) {
            return Action::Launch(crate::assoc::launch(f));
        }
        self.load(f);
        Action::None
    }
}
