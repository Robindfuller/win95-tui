// Doom, played by tui-doom: a separate program (GPL, like Doom itself) run in
// its --pipe mode. It sends 320x200 RGB frames on stdout, which this draws in
// half blocks, and takes keys on stdin as "d <key>" and "u <key>" lines.
use super::{Action, App, Launch};
use crate::{draw::{st, Canvas}, icons::Icon, theme::{home, Theme}};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers, ModifierKeyCode, MouseButton, MouseEventKind};
use ratatui::style::Color;
use std::{
    io::{BufRead, BufReader, Read, Write},
    path::PathBuf,
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

const FRAME_W: usize = 320;
const FRAME_H: usize = 200;
/// Until the terminal shows it sends key releases, a key counts as held
/// until this long after it was pressed (the wait before key repeat)...
const FIRST_HOLD: Duration = Duration::from_millis(650);
/// ...and then until this long after its last repeat.
const NEXT_HOLD: Duration = Duration::from_millis(150);
/// Some terminals (foot) send a held key's repeats as a release and a press,
/// so a release only counts once this long passes without a press after it.
const RELEASE_GRACE: Duration = Duration::from_millis(30);

#[derive(Default)]
struct Shared {
    /// frame count and the latest frame
    frame: Mutex<(u64, Vec<u8>)>,
    ended: AtomicBool,
    /// what tui-doom said on stderr
    said: Mutex<String>,
}

pub struct Doom {
    size: (u16, u16),
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    shared: Arc<Shared>,
    seen: u64,
    frame: Vec<u8>,
    /// why it couldn't start
    error: Option<String>,
    /// key releases have been seen, so held keys can be trusted
    releases: bool,
    down: Vec<String>,
    held: Vec<(String, Instant)>,
    /// keys released, and when that counts if no press follows
    releasing: Vec<(String, Instant)>,
}

fn program() -> Option<PathBuf> {
    let path = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect::<Vec<_>>()).unwrap_or_default();
    path.into_iter().chain([home().join(".cargo/bin"), home().join(".local/bin")]).map(|d| d.join("tui-doom")).find(|p| p.is_file())
}

impl Doom {
    pub fn new() -> Doom {
        let shared = Arc::new(Shared::default());
        let mut d = Doom {
            size: (80, 30),
            child: None,
            stdin: None,
            shared: shared.clone(),
            seen: 0,
            frame: vec![],
            error: None,
            releases: false,
            down: vec![],
            held: vec![],
            releasing: vec![],
        };
        let Some(prog) = program() else {
            d.error = Some("Doom needs tui-doom, which isn't installed.\n\nIt's at ~/Projects/tui-doom:\ncargo install --path ~/Projects/tui-doom".into());
            return d;
        };
        let spawned = Command::new(prog).arg("--pipe").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn();
        let mut child = match spawned {
            Ok(c) => c,
            Err(e) => {
                d.error = Some(format!("Couldn't start tui-doom:\n{e}"));
                return d;
            }
        };
        d.stdin = child.stdin.take();
        if let Some(mut out) = child.stdout.take() {
            let sh = shared.clone();
            thread::spawn(move || {
                let mut buf = vec![0u8; FRAME_W * FRAME_H * 3];
                while out.read_exact(&mut buf).is_ok() {
                    let mut f = sh.frame.lock().unwrap();
                    f.0 += 1;
                    std::mem::swap(&mut f.1, &mut buf);
                    buf.resize(FRAME_W * FRAME_H * 3, 0);
                }
                sh.ended.store(true, Ordering::SeqCst);
            });
        }
        if let Some(err) = child.stderr.take() {
            let sh = shared;
            thread::spawn(move || {
                for line in BufReader::new(err).lines().map_while(Result::ok) {
                    let mut s = sh.said.lock().unwrap();
                    s.push_str(&line);
                    s.push('\n');
                }
            });
        }
        d.child = Some(child);
        d
    }

    fn send(&mut self, down: bool, key: &str) {
        if let Some(i) = &mut self.stdin {
            let _ = writeln!(i, "{} {key}", if down { 'd' } else { 'u' });
        }
    }

    fn press(&mut self, key: &str) {
        if !self.down.iter().any(|k| k == key) {
            self.down.push(key.into());
            self.send(true, key);
        }
    }

    fn release(&mut self, key: &str) {
        let before = self.down.len() + self.held.len();
        self.down.retain(|k| k != key);
        self.held.retain(|h| h.0 != key);
        if self.down.len() + self.held.len() != before {
            self.send(false, key);
        }
    }

    fn release_all(&mut self) {
        self.releasing.clear();
        let keys: Vec<String> = self.down.drain(..).chain(self.held.drain(..).map(|h| h.0)).collect();
        for k in keys {
            self.send(false, &k);
        }
    }

    /// The picture's size in pixels inside the window, 4:3 like Doom on a monitor.
    fn fit(&self) -> (usize, usize) {
        let (cw, ch) = (self.size.0 as usize, self.size.1 as usize);
        let mut w = cw;
        let mut h = w * 3 / 4;
        if h > ch * 2 {
            h = ch * 2;
            w = h * 4 / 3;
        }
        (w, h)
    }

    /// Doom's frame averaged down (or repeated up) to w by h pixels.
    fn scaled(&self, w: usize, h: usize) -> Vec<(u8, u8, u8)> {
        let f = &self.frame;
        let mut out = Vec::with_capacity(w * h);
        for y in 0..h {
            let y0 = y * FRAME_H / h;
            let y1 = ((y + 1) * FRAME_H / h).max(y0 + 1);
            for x in 0..w {
                let x0 = x * FRAME_W / w;
                let x1 = ((x + 1) * FRAME_W / w).max(x0 + 1);
                let (mut r, mut g, mut b) = (0u32, 0u32, 0u32);
                for sy in y0..y1 {
                    for p in f[(sy * FRAME_W + x0) * 3..(sy * FRAME_W + x1) * 3].chunks_exact(3) {
                        r += p[0] as u32;
                        g += p[1] as u32;
                        b += p[2] as u32;
                    }
                }
                let n = ((y1 - y0) * (x1 - x0)) as u32;
                out.push(((r / n) as u8, (g / n) as u8, (b / n) as u8));
            }
        }
        out
    }
}

impl Drop for Doom {
    fn drop(&mut self) {
        self.stdin = None;
        if let Some(c) = &mut self.child {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

fn key_name(code: KeyCode) -> Option<String> {
    use KeyCode::*;
    use ModifierKeyCode::*;
    Some(
        match code {
            Char(' ') => "space",
            Char(c) => return Some(c.to_lowercase().to_string()),
            Up => "up",
            Down => "down",
            Left => "left",
            Right => "right",
            Enter => "enter",
            Esc => "esc",
            Tab | BackTab => "tab",
            Backspace => "backspace",
            Home => "home",
            End => "end",
            PageUp => "pgup",
            PageDown => "pgdn",
            Insert => "ins",
            Delete => "del",
            Pause => "pause",
            F(n) => return Some(format!("f{n}")),
            Modifier(LeftControl | RightControl) => "ctrl",
            Modifier(LeftShift | RightShift) => "shift",
            Modifier(LeftAlt | RightAlt) => "alt",
            _ => return None,
        }
        .into(),
    )
}

impl App for Doom {
    fn title(&self) -> String {
        "Doom".into()
    }

    fn icon(&self) -> Icon {
        Icon::Doom
    }

    fn size_hint(&self) -> (u16, u16) {
        (80, 30)
    }

    fn render(&mut self, c: &mut Canvas, th: &Theme, _focused: bool) {
        let (cw, ch) = (self.size.0 as i32, self.size.1 as i32);
        let black = Color::Rgb(0, 0, 0);
        c.fill(0, 0, cw, ch, st(th.text, black));
        if self.frame.is_empty() {
            let said = self.shared.said.lock().unwrap().clone();
            let text = self.error.clone().unwrap_or_else(|| said.lines().last().unwrap_or("Starting Doom...").to_string());
            for (i, line) in text.lines().enumerate() {
                c.text(2, 1 + i as i32, line, st(th.text, black));
            }
            return;
        }
        let (w, h) = self.fit();
        if w == 0 || h == 0 {
            return;
        }
        let pix = self.scaled(w, h);
        let (ox, oy) = ((cw - w as i32) / 2, (ch - h.div_ceil(2) as i32) / 2);
        let rgb = |p: (u8, u8, u8)| Color::Rgb(p.0, p.1, p.2);
        for y in (0..h).step_by(2) {
            for x in 0..w {
                let top = pix[y * w + x];
                let bottom = if y + 1 < h { pix[(y + 1) * w + x] } else { (0, 0, 0) };
                c.put(ox + x as i32, oy + (y / 2) as i32, "▀", st(rgb(top), rgb(bottom)));
            }
        }
    }

    fn key(&mut self, k: KeyEvent) -> Action {
        let Some(name) = key_name(k.code) else { return Action::None };
        match k.kind {
            KeyEventKind::Release => {
                self.releases = true;
                if !self.releasing.iter().any(|r| r.0 == name) {
                    self.releasing.push((name, Instant::now() + RELEASE_GRACE));
                }
            }
            _ if self.releases => {
                // a press straight after its release is a repeat
                self.releasing.retain(|r| r.0 != name);
                self.press(&name);
                // a repeat steps through the menus
                self.send(true, &name);
            }
            _ => {
                // A repeat can't be told from a second tap, so each is a press.
                self.send(true, &name);
                let now = Instant::now();
                match self.held.iter_mut().find(|h| h.0 == name) {
                    Some(h) => h.1 = h.1.max(now + NEXT_HOLD),
                    None => self.held.push((name, now + FIRST_HOLD)),
                }
            }
        }
        Action::None
    }

    fn mouse(&mut self, kind: MouseEventKind, _x: i32, _y: i32, _mods: KeyModifiers) -> Action {
        match kind {
            MouseEventKind::Down(MouseButton::Left) => self.press("ctrl"),
            MouseEventKind::Up(MouseButton::Left) => self.release("ctrl"),
            MouseEventKind::Down(MouseButton::Right) => self.press("space"),
            MouseEventKind::Up(MouseButton::Right) => self.release("space"),
            _ => {}
        }
        Action::None
    }

    fn resize(&mut self, w: u16, h: u16) {
        self.size = (w, h);
    }

    fn poll(&mut self) -> (bool, Action) {
        let now = Instant::now();
        let released: Vec<String> = self.releasing.iter().filter(|r| r.1 <= now).map(|r| r.0.clone()).collect();
        self.releasing.retain(|r| r.1 > now);
        for k in released {
            self.release(&k);
        }
        let gone: Vec<String> = self.held.iter().filter(|h| h.1 <= now).map(|h| h.0.clone()).collect();
        self.held.retain(|h| h.1 > now);
        for k in gone {
            self.send(false, &k);
        }
        if self.shared.ended.load(Ordering::SeqCst) && self.child.as_mut().is_some_and(|c| c.try_wait().is_ok_and(|s| s.is_some())) {
            self.child = None;
            let said = self.shared.said.lock().unwrap().clone();
            if let Some(e) = said.lines().rev().find_map(|l| l.strip_prefix("tui-doom: ")) {
                return (true, Action::Many(vec![Action::Close, Action::Launch(Launch::Msg { title: "Doom".into(), text: e.into() })]));
            }
            return (true, Action::Close);
        }
        let f = self.shared.frame.lock().unwrap();
        if f.0 == self.seen {
            return (false, Action::None);
        }
        self.seen = f.0;
        self.frame.clone_from(&f.1);
        (true, Action::None)
    }

    fn key_releases(&self) -> bool {
        true
    }

    fn focus_lost(&mut self) {
        self.release_all();
    }
}
