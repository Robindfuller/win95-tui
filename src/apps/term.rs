// A real terminal: a pty running a shell or program, parsed by vt100.
use super::{Action, App};
use crate::{draw::Canvas, icons::Icon, menu::{Cmd, Item}, theme::{home, Theme}};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use ratatui::style::{Color, Modifier, Style};
use std::{
    io::{Read, Write},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
};

#[derive(Default)]
pub struct Cb {
    replies: Vec<u8>,
    title: Option<String>,
    fg: String,
    bg: String,
}

impl vt100::Callbacks for Cb {
    fn set_window_title(&mut self, _: &mut vt100::Screen, t: &[u8]) {
        let t = String::from_utf8_lossy(t).trim().to_string();
        self.title = (!t.is_empty()).then_some(t);
    }
    fn unhandled_csi(&mut self, screen: &mut vt100::Screen, i1: Option<u8>, _i2: Option<u8>, params: &[&[u16]], c: char) {
        let p0 = params.first().and_then(|p| p.first()).copied().unwrap_or(0);
        match (i1, c) {
            (None, 'n') if p0 == 6 => {
                let (r, col) = screen.cursor_position();
                self.replies.extend(format!("\x1b[{};{}R", r + 1, col + 1).bytes());
            }
            (None, 'n') if p0 == 5 => self.replies.extend(b"\x1b[0n"),
            (None, 'c') => self.replies.extend(b"\x1b[?62;22c"),
            (Some(b'>'), 'c') => self.replies.extend(b"\x1b[>1;10;0c"),
            _ => {}
        }
    }
    fn unhandled_osc(&mut self, _: &mut vt100::Screen, params: &[&[u8]]) {
        if params.len() >= 2 && params[1] == b"?" {
            match params[0] {
                b"10" => self.replies.extend(format!("\x1b]10;rgb:{}\x1b\\", self.fg).bytes()),
                b"11" => self.replies.extend(format!("\x1b]11;rgb:{}\x1b\\", self.bg).bytes()),
                _ => {}
            }
        }
    }
}

/// Rewrites the few control sequences vt100 doesn't know (HVP, HPA, HPR, VPR,
/// REP and the ANSI save/restore cursor) into ones it does. Holds partial
/// sequences across reads.
#[derive(Default)]
struct Filter {
    state: u8,
    csi: Vec<u8>,
    last: Vec<u8>,
}

const GROUND: u8 = 0;
const ESC: u8 = 1;
const CSI: u8 = 2;
const STR: u8 = 3;
const STR_ESC: u8 = 4;

impl Filter {
    fn feed(&mut self, input: &[u8], out: &mut Vec<u8>) {
        for &b in input {
            match self.state {
                ESC => match b {
                    b'[' => {
                        self.csi.clear();
                        self.state = CSI;
                    }
                    b']' | b'P' | b'_' | b'^' | b'X' => {
                        out.extend([0x1b, b]);
                        self.state = STR;
                    }
                    _ => {
                        out.extend([0x1b, b]);
                        self.state = GROUND;
                    }
                },
                CSI => {
                    self.csi.push(b);
                    if (0x40..=0x7e).contains(&b) {
                        self.csi_done(out);
                        self.state = GROUND;
                    } else if self.csi.len() > 128 {
                        out.extend(b"\x1b[");
                        out.extend(&self.csi);
                        self.state = GROUND;
                    }
                }
                STR => {
                    out.push(b);
                    if b == 0x07 {
                        self.state = GROUND;
                    } else if b == 0x1b {
                        self.state = STR_ESC;
                    }
                }
                STR_ESC => {
                    out.push(b);
                    self.state = if b == b'\\' { GROUND } else { STR };
                }
                _ => {
                    if b == 0x1b {
                        self.state = ESC;
                        continue;
                    }
                    out.push(b);
                    if b >= 0xc0 || (0x21..0x7f).contains(&b) || b == b' ' {
                        self.last.clear();
                        self.last.push(b);
                    } else if (0x80..0xc0).contains(&b) && self.last.len() < 4 {
                        self.last.push(b);
                    }
                }
            }
        }
    }

    fn csi_done(&mut self, out: &mut Vec<u8>) {
        let (fin, body) = self.csi.split_last().map(|(f, b)| (*f, b.to_vec())).unwrap_or((0, vec![]));
        let plain = !body.first().is_some_and(|c| b"<=>?".contains(c)) && !body.iter().any(|c| (0x20..0x30).contains(c));
        let csi = |out: &mut Vec<u8>, f: u8| {
            out.extend(b"\x1b[");
            out.extend(&body);
            out.push(f);
        };
        match fin {
            b'f' if plain => csi(out, b'H'),
            b'`' if plain => csi(out, b'G'),
            b'a' if plain => csi(out, b'C'),
            b'e' if plain => csi(out, b'B'),
            b's' if plain && body.is_empty() => out.extend(b"\x1b7"),
            b'u' if plain && body.is_empty() => out.extend(b"\x1b8"),
            b'b' if plain => {
                let n: usize = std::str::from_utf8(&body).ok().and_then(|s| s.parse().ok()).unwrap_or(1).clamp(1, 2000);
                for _ in 0..n {
                    out.extend(&self.last);
                }
            }
            _ => {
                out.extend(b"\x1b[");
                out.extend(&self.csi);
            }
        }
    }
}

pub struct TermApp {
    parser: Arc<Mutex<vt100::Parser<Cb>>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    dirty: Arc<AtomicBool>,
    dead: Arc<AtomicBool>,
    base: String,
    icon: Icon,
    keep_open: bool,
    finished: bool,
    size: (u16, u16),
    scroll: usize,
    fg: Color,
    bg: Color,
    cwd: PathBuf,
}

impl TermApp {
    pub fn new(cmd: Option<&str>, cwd: Option<PathBuf>, title: &str, icon: Icon, keep_open: bool, th: &Theme) -> anyhow::Result<TermApp> {
        let pty = native_pty_system().openpty(PtySize { rows: 24, cols: 80, pixel_width: 0, pixel_height: 0 })?;
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".into());
        let mut b = match cmd {
            None => CommandBuilder::new_default_prog(),
            Some(c) => {
                let mut b = CommandBuilder::new(&shell);
                b.args(["-lc", c]);
                b
            }
        };
        b.env("TERM", "xterm-256color");
        b.env("COLORTERM", "truecolor");
        b.env("WIN95_TUI", "1");
        b.env_remove("TERM_PROGRAM");
        let cwd = cwd.unwrap_or_else(home);
        b.cwd(&cwd);
        let child = pty.slave.spawn_command(b)?;
        drop(pty.slave);
        let mut reader = pty.master.try_clone_reader()?;
        let writer: Arc<Mutex<Box<dyn Write + Send>>> = Arc::new(Mutex::new(pty.master.take_writer()?));
        let cb = Cb { fg: th.fg_hex.clone(), bg: th.bg_hex.clone(), ..Default::default() };
        let parser = Arc::new(Mutex::new(vt100::Parser::new_with_callbacks(24, 80, 5000, cb)));
        let dirty = Arc::new(AtomicBool::new(true));
        let dead = Arc::new(AtomicBool::new(false));
        {
            let (parser, writer, dirty, dead) = (parser.clone(), writer.clone(), dirty.clone(), dead.clone());
            thread::spawn(move || {
                let mut buf = [0u8; 16384];
                let mut filter = Filter::default();
                let mut clean = Vec::with_capacity(32768);
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            clean.clear();
                            filter.feed(&buf[..n], &mut clean);
                            let replies = {
                                let mut p = parser.lock().unwrap();
                                p.process(&clean);
                                std::mem::take(&mut p.callbacks_mut().replies)
                            };
                            if !replies.is_empty() {
                                let mut w = writer.lock().unwrap();
                                let _ = w.write_all(&replies);
                                let _ = w.flush();
                            }
                            dirty.store(true, Ordering::Relaxed);
                        }
                    }
                }
                dead.store(true, Ordering::Relaxed);
                dirty.store(true, Ordering::Relaxed);
            });
        }
        Ok(TermApp {
            parser,
            writer,
            master: pty.master,
            child,
            dirty,
            dead,
            base: title.into(),
            icon,
            keep_open,
            finished: false,
            size: (80, 24),
            scroll: 0,
            fg: th.text,
            bg: th.client,
            cwd,
        })
    }

    fn send(&self, bytes: &[u8]) {
        if bytes.is_empty() || self.finished {
            return;
        }
        let mut w = self.writer.lock().unwrap();
        let _ = w.write_all(bytes);
        let _ = w.flush();
    }

    fn set_scroll(&mut self, n: usize) {
        let mut p = self.parser.lock().unwrap();
        p.screen_mut().set_scrollback(n);
        self.scroll = p.screen().scrollback();
        self.dirty.store(true, Ordering::Relaxed);
    }

    fn color(&self, c: vt100::Color, fg: bool) -> Color {
        match c {
            vt100::Color::Default => if fg { self.fg } else { self.bg },
            vt100::Color::Idx(i) => Color::Indexed(i),
            vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
        }
    }
}

impl Drop for TermApp {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

pub fn key_bytes(k: &KeyEvent, app_cursor: bool) -> Vec<u8> {
    let m = k.modifiers;
    let (alt, ctrl, shift) = (m.contains(KeyModifiers::ALT), m.contains(KeyModifiers::CONTROL), m.contains(KeyModifiers::SHIFT));
    let modn = 1 + shift as u8 + 2 * alt as u8 + 4 * ctrl as u8;
    let arrow = |f: char| -> Vec<u8> {
        if modn > 1 {
            format!("\x1b[1;{}{}", modn, f).into_bytes()
        } else if app_cursor {
            format!("\x1bO{}", f).into_bytes()
        } else {
            format!("\x1b[{}", f).into_bytes()
        }
    };
    let tilde = |n: u8| -> Vec<u8> {
        if modn > 1 { format!("\x1b[{};{}~", n, modn).into_bytes() } else { format!("\x1b[{}~", n).into_bytes() }
    };
    let mut out = Vec::new();
    match k.code {
        KeyCode::Char(c) => {
            if alt {
                out.push(0x1b);
            }
            if ctrl {
                let b = match c.to_ascii_lowercase() {
                    l @ 'a'..='z' => (l as u8) & 0x1f,
                    ' ' | '@' | '2' => 0,
                    '[' | '3' => 27,
                    '\\' | '4' => 28,
                    ']' | '5' => 29,
                    '^' | '6' => 30,
                    '_' | '7' | '-' => 31,
                    '8' | '?' => 127,
                    _ => {
                        out.extend(c.to_string().bytes());
                        return out;
                    }
                };
                out.push(b);
            } else {
                out.extend(c.to_string().bytes());
            }
        }
        KeyCode::Enter => {
            if alt {
                out.push(0x1b);
            }
            out.push(b'\r');
        }
        KeyCode::Tab => out.push(b'\t'),
        KeyCode::BackTab => out.extend(b"\x1b[Z"),
        KeyCode::Backspace => {
            if alt {
                out.push(0x1b);
            }
            out.push(if ctrl { 0x08 } else { 0x7f });
        }
        KeyCode::Esc => out.push(0x1b),
        KeyCode::Up => out = arrow('A'),
        KeyCode::Down => out = arrow('B'),
        KeyCode::Right => out = arrow('C'),
        KeyCode::Left => out = arrow('D'),
        KeyCode::Home => out = arrow('H'),
        KeyCode::End => out = arrow('F'),
        KeyCode::Insert => out = tilde(2),
        KeyCode::Delete => out = tilde(3),
        KeyCode::PageUp => out = tilde(5),
        KeyCode::PageDown => out = tilde(6),
        KeyCode::F(n @ 1..=4) => {
            let f = (b'P' + n - 1) as char;
            out = if modn > 1 { format!("\x1b[1;{}{}", modn, f).into_bytes() } else { format!("\x1bO{}", f).into_bytes() };
        }
        KeyCode::F(n @ 5..=12) => out = tilde([15, 17, 18, 19, 20, 21, 23, 24][(n - 5) as usize]),
        _ => {}
    }
    out
}

impl App for TermApp {
    fn title(&self) -> String {
        if self.finished {
            return format!("Finished - {}", self.base);
        }
        match &self.parser.lock().unwrap().callbacks().title {
            Some(t) => format!("{} - {}", self.base, t),
            None => self.base.clone(),
        }
    }

    fn icon(&self) -> Icon {
        self.icon
    }

    fn size_hint(&self) -> (u16, u16) {
        (80, 24)
    }

    fn render(&mut self, c: &mut Canvas, _th: &Theme, _focused: bool) {
        let p = self.parser.lock().unwrap();
        let s = p.screen();
        let (rows, cols) = s.size();
        for y in 0..rows {
            for x in 0..cols {
                let Some(cell) = s.cell(y, x) else { continue };
                if cell.is_wide_continuation() {
                    continue;
                }
                let (mut fg, mut bg) = (self.color(cell.fgcolor(), true), self.color(cell.bgcolor(), false));
                if cell.inverse() {
                    std::mem::swap(&mut fg, &mut bg);
                }
                let mut style = Style::new().fg(fg).bg(bg);
                let mut m = Modifier::empty();
                if cell.bold() {
                    m |= Modifier::BOLD;
                }
                if cell.dim() {
                    m |= Modifier::DIM;
                }
                if cell.italic() {
                    m |= Modifier::ITALIC;
                }
                if cell.underline() {
                    m |= Modifier::UNDERLINED;
                }
                style = style.add_modifier(m);
                let sym = if cell.has_contents() { cell.contents() } else { " " };
                c.put(x as i32, y as i32, sym, style);
                if cell.is_wide() {
                    c.put(x as i32 + 1, y as i32, " ", Style::new().bg(bg));
                }
            }
        }
        if self.scroll > 0 {
            let tag = format!(" ↑{} ", self.scroll);
            let x = cols as i32 - tag.chars().count() as i32;
            c.text(x, 0, &tag, Style::new().fg(self.bg).bg(self.fg));
        }
    }

    fn cursor(&self) -> Option<(u16, u16)> {
        let p = self.parser.lock().unwrap();
        let s = p.screen();
        if s.hide_cursor() || self.scroll > 0 || self.finished {
            return None;
        }
        let (r, c) = s.cursor_position();
        Some((c, r))
    }

    fn key(&mut self, k: KeyEvent) -> Action {
        if self.finished {
            return match k.code {
                KeyCode::Enter | KeyCode::Esc => Action::Close,
                _ => Action::None,
            };
        }
        if self.scroll > 0 {
            if k.modifiers.contains(KeyModifiers::SHIFT) && matches!(k.code, KeyCode::PageUp | KeyCode::PageDown) {
            } else {
                self.set_scroll(0);
            }
        }
        if k.modifiers.contains(KeyModifiers::SHIFT) {
            let page = self.size.1 as usize;
            match k.code {
                KeyCode::PageUp => {
                    self.set_scroll(self.scroll + page);
                    return Action::None;
                }
                KeyCode::PageDown => {
                    self.set_scroll(self.scroll.saturating_sub(page));
                    return Action::None;
                }
                _ => {}
            }
        }
        let app_cursor = self.parser.lock().unwrap().screen().application_cursor();
        self.send(&key_bytes(&k, app_cursor));
        Action::None
    }

    fn mouse(&mut self, kind: MouseEventKind, x: i32, y: i32, mods: KeyModifiers) -> Action {
        let (mode, enc, alt) = {
            let p = self.parser.lock().unwrap();
            let s = p.screen();
            (s.mouse_protocol_mode(), s.mouse_protocol_encoding(), s.alternate_screen())
        };
        use vt100::MouseProtocolMode as M;
        if mode == M::None || self.finished {
            match kind {
                MouseEventKind::ScrollUp if alt && !self.finished => self.send(b"\x1b[A\x1b[A\x1b[A"),
                MouseEventKind::ScrollDown if alt && !self.finished => self.send(b"\x1b[B\x1b[B\x1b[B"),
                MouseEventKind::ScrollUp => self.set_scroll(self.scroll + 3),
                MouseEventKind::ScrollDown => self.set_scroll(self.scroll.saturating_sub(3)),
                _ => {}
            }
            return Action::None;
        }
        let x = x.clamp(0, self.size.0 as i32 - 1) as u32;
        let y = y.clamp(0, self.size.1 as i32 - 1) as u32;
        let btn = |b: MouseButton| match b {
            MouseButton::Left => 0,
            MouseButton::Middle => 1,
            MouseButton::Right => 2,
        };
        let (code, release) = match kind {
            MouseEventKind::Down(b) => (btn(b), false),
            MouseEventKind::Up(b) if mode != M::Press => (btn(b), true),
            MouseEventKind::Drag(b) if matches!(mode, M::ButtonMotion | M::AnyMotion) => (btn(b) + 32, false),
            MouseEventKind::Moved if mode == M::AnyMotion => (35, false),
            MouseEventKind::ScrollUp => (64, false),
            MouseEventKind::ScrollDown => (65, false),
            _ => return Action::None,
        };
        let mut code = code;
        if mods.contains(KeyModifiers::SHIFT) {
            code += 4;
        }
        if mods.contains(KeyModifiers::ALT) {
            code += 8;
        }
        if mods.contains(KeyModifiers::CONTROL) {
            code += 16;
        }
        let bytes = match enc {
            vt100::MouseProtocolEncoding::Sgr => format!("\x1b[<{};{};{}{}", code, x + 1, y + 1, if release { 'm' } else { 'M' }).into_bytes(),
            _ => {
                let c = if release { 3 } else { code };
                vec![0x1b, b'[', b'M', (32 + c) as u8, (33 + x).min(255) as u8, (33 + y).min(255) as u8]
            }
        };
        self.send(&bytes);
        Action::None
    }

    fn paste(&mut self, s: &str) {
        let bracketed = self.parser.lock().unwrap().screen().bracketed_paste();
        let s = s.replace("\r\n", "\r").replace('\n', "\r");
        if bracketed {
            self.send(format!("\x1b[200~{}\x1b[201~", s).as_bytes());
        } else {
            self.send(s.as_bytes());
        }
    }

    fn resize(&mut self, w: u16, h: u16) {
        if (w, h) == self.size || w == 0 || h == 0 {
            return;
        }
        self.size = (w, h);
        let _ = self.master.resize(PtySize { rows: h, cols: w, pixel_width: 0, pixel_height: 0 });
        self.parser.lock().unwrap().screen_mut().set_size(h, w);
        self.dirty.store(true, Ordering::Relaxed);
    }

    fn poll(&mut self) -> (bool, Action) {
        let dirty = self.dirty.swap(false, Ordering::Relaxed);
        if !self.finished && (self.dead.load(Ordering::Relaxed) || matches!(self.child.try_wait(), Ok(Some(_)))) {
            if self.keep_open {
                self.finished = true;
                return (true, Action::None);
            }
            return (true, Action::Close);
        }
        (dirty, Action::None)
    }

    fn menubar(&self) -> Vec<(&'static str, Vec<Item>)> {
        vec![
            ("File", vec![
                Item::new("New Window", Cmd::App("new")).icon(Icon::Terminal),
                Item::new("Open Folder Here", Cmd::App("explore")).icon(Icon::Folder),
                Item::sep(),
                Item::new("Close", Cmd::Sys(crate::menu::Sys::Close)).key("Alt+F4"),
            ]),
            ("Edit", vec![
                Item::new("Paste", Cmd::App("paste")).key("Ctrl+Shift+V"),
                Item::new("Scroll Up", Cmd::App("pgup")).key("Shift+PgUp"),
                Item::new("Scroll Down", Cmd::App("pgdn")).key("Shift+PgDn"),
            ]),
        ]
    }

    fn command(&mut self, cmd: &str) -> Action {
        match cmd {
            "new" => Action::Launch(super::Launch::Shell { cmd: None, cwd: Some(self.cwd.clone()), title: "Terminal".into(), icon: Icon::Terminal, keep_open: false }),
            "explore" => Action::Launch(super::Launch::Explorer(self.cwd.clone())),
            "pgup" => {
                self.set_scroll(self.scroll + self.size.1 as usize);
                Action::None
            }
            "pgdn" => {
                self.set_scroll(self.scroll.saturating_sub(self.size.1 as usize));
                Action::None
            }
            "paste" => {
                if let Ok(out) = std::process::Command::new("wl-paste").arg("-n").output() {
                    self.paste(&String::from_utf8_lossy(&out.stdout));
                }
                Action::None
            }
            _ => Action::None,
        }
    }

    fn theme_changed(&mut self, th: &Theme) {
        self.fg = th.text;
        self.bg = th.client;
        let mut p = self.parser.lock().unwrap();
        p.callbacks_mut().fg = th.fg_hex.clone();
        p.callbacks_mut().bg = th.bg_hex.clone();
    }
}
