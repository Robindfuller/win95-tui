// Sessions: the desktop runs in a background server that keeps its windows
// and their shells alive, and the terminal you open is a client attached to
// it. Closing the terminal only detaches; running win95 again picks up where
// you left off, and `win95 stop` ends it.
//
// The server draws into a ratatui Terminal whose backend, instead of writing
// to a screen, sends the changed cells down a socket. The client paints them
// with an ordinary crossterm backend and sends back keys, mouse and resizes.
use crate::wm::Desktop;
use crossterm::{
    event::{self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, Event, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags},
    execute,
};
use ratatui::{
    backend::{Backend, ClearType, CrosstermBackend, WindowSize},
    buffer::Cell,
    layout::{Position, Size},
    Terminal,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{
    io::{self, BufWriter, Read, Write},
    os::unix::{fs::PermissionsExt, net::{UnixListener, UnixStream}, process::CommandExt},
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Mutex,
    },
    time::{Duration, Instant, UNIX_EPOCH},
};

/// Set in the server, so Quit can offer to detach instead.
pub static SERVER: AtomicBool = AtomicBool::new(false);
/// Escape codes meant for the client's own terminal (the clipboard over SSH).
pub static OUTBOX: Mutex<Vec<u8>> = Mutex::new(Vec::new());

#[derive(Serialize, Deserialize, Debug)]
enum ToServer {
    /// a client attaching: its size, the desktop it asked for, and when its
    /// win95 was built, to spot a newer one than the server's
    Hello { w: u16, h: u16, tiling: Option<bool>, build: u64 },
    Event(Event),
    Stop,
}

#[derive(Serialize, Deserialize, Debug)]
enum ToClient {
    Draw(Vec<(u16, u16, Cell)>),
    HideCursor,
    ShowCursor,
    Cursor(u16, u16),
    Clear,
    Flush,
    Raw(Vec<u8>),
    /// the desktop quit
    Bye,
    /// detached, or another terminal took over
    Detached,
}

fn socket() -> PathBuf {
    match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(d) => PathBuf::from(d).join("win95-tui.sock"),
        None => std::env::temp_dir().join(format!("win95-tui-{}.sock", unsafe { libc::getuid() })),
    }
}

/// When this win95 binary was last changed, in seconds.
fn build() -> u64 {
    std::env::current_exe()
        .and_then(std::fs::metadata)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs())
}

fn send<T: Serialize>(w: &mut impl Write, m: &T) -> io::Result<()> {
    let body = serde_json::to_vec(m).map_err(io::Error::other)?;
    w.write_all(&(body.len() as u32).to_le_bytes())?;
    w.write_all(&body)
}

fn recv<T: DeserializeOwned>(r: &mut impl Read) -> io::Result<T> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let mut body = vec![0u8; u32::from_le_bytes(len) as usize];
    r.read_exact(&mut body)?;
    serde_json::from_slice(&body).map_err(io::Error::other)
}

// ---------------------------------------------------------------- server

/// A ratatui backend that sends what it would draw to the attached client.
struct Remote {
    out: Option<BufWriter<UnixStream>>,
    size: Size,
    cursor: Position,
}

impl Remote {
    fn put(&mut self, m: ToClient) {
        if let Some(out) = &mut self.out {
            if send(out, &m).is_err() {
                self.out = None;
            }
        }
    }
}

impl Backend for Remote {
    type Error = io::Error;
    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        let cells = content.map(|(x, y, c)| (x, y, c.clone())).collect();
        self.put(ToClient::Draw(cells));
        Ok(())
    }
    fn hide_cursor(&mut self) -> io::Result<()> {
        self.put(ToClient::HideCursor);
        Ok(())
    }
    fn show_cursor(&mut self) -> io::Result<()> {
        self.put(ToClient::ShowCursor);
        Ok(())
    }
    fn get_cursor_position(&mut self) -> io::Result<Position> {
        Ok(self.cursor)
    }
    fn set_cursor_position<P: Into<Position>>(&mut self, p: P) -> io::Result<()> {
        self.cursor = p.into();
        self.put(ToClient::Cursor(self.cursor.x, self.cursor.y));
        Ok(())
    }
    fn clear(&mut self) -> io::Result<()> {
        self.put(ToClient::Clear);
        Ok(())
    }
    fn clear_region(&mut self, _t: ClearType) -> io::Result<()> {
        self.clear()
    }
    fn size(&self) -> io::Result<Size> {
        Ok(self.size)
    }
    fn window_size(&mut self) -> io::Result<WindowSize> {
        Ok(WindowSize { columns_rows: self.size, pixels: Size::new(0, 0) })
    }
    fn flush(&mut self) -> io::Result<()> {
        self.put(ToClient::Flush);
        if let Some(out) = &mut self.out {
            if out.flush().is_err() {
                self.out = None;
            }
        }
        Ok(())
    }
}

enum In {
    Attach(UnixStream),
    Msg(u64, ToServer),
    Gone(u64),
}

/// Runs the desktop with no terminal of its own, for clients to attach to.
pub fn serve() -> anyhow::Result<()> {
    SERVER.store(true, Ordering::Relaxed);
    let path = socket();
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)?;
    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    let (tx, rx) = mpsc::channel();
    {
        let tx = tx.clone();
        std::thread::spawn(move || {
            for s in listener.incoming().flatten() {
                if tx.send(In::Attach(s)).is_err() {
                    break;
                }
            }
        });
    }
    let built = build();
    let mut term = Terminal::new(Remote { out: None, size: Size::new(80, 24), cursor: Position::new(0, 0) })?;
    let mut desk: Option<Desktop> = None;
    let (mut next, mut cur) = (0u64, None::<u64>);
    // connections that haven't said hello yet
    let mut waiting = std::collections::HashMap::new();
    let frame = Duration::from_millis(16);
    let mut last = Instant::now() - frame;
    let mut dirty = true;
    loop {
        let wait = if dirty { frame.saturating_sub(last.elapsed()) } else { Duration::from_millis(25) };
        let mut msgs = vec![];
        match rx.recv_timeout(wait) {
            Ok(m) => msgs.push(m),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        msgs.extend(rx.try_iter());
        for m in msgs {
            match m {
                In::Attach(s) => {
                    next += 1;
                    let (g, tx) = (next, tx.clone());
                    let Ok(mut r) = s.try_clone() else { continue };
                    std::thread::spawn(move || {
                        while let Ok(m) = recv::<ToServer>(&mut r) {
                            if tx.send(In::Msg(g, m)).is_err() {
                                return;
                            }
                        }
                        let _ = tx.send(In::Gone(g));
                    });
                    waiting.insert(g, s);
                }
                In::Msg(_, ToServer::Stop) => {
                    if let Some(d) = &mut desk {
                        d.quit = true;
                    } else {
                        return finish(&mut term);
                    }
                }
                In::Msg(g, ToServer::Hello { w, h, tiling, build }) => {
                    let Some(s) = waiting.remove(&g) else { continue };
                    // a newer terminal takes over from the one before
                    term.backend_mut().put(ToClient::Detached);
                    let _ = term.backend_mut().flush();
                    term.backend_mut().out = Some(BufWriter::new(s));
                    cur = Some(g);
                    {
                        term.backend_mut().size = Size::new(w, h);
                        let d = desk.get_or_insert_with(|| {
                            let mut d = Desktop::new(w, h);
                            d.set_tiling(crate::theme::saved_tiling(), false);
                            d
                        });
                        d.event(Event::Resize(w, h));
                        if let Some(t) = tiling {
                            d.set_tiling(t, false);
                        }
                        if build > built {
                            d.launch(crate::apps::Launch::Msg {
                                title: "Newer win95".into(),
                                text: "A newer win95 is installed than the one\nrunning here. Quit (not Detach) and start\nwin95 again to use it.".into(),
                            });
                        }
                        let _ = term.clear();
                        let _ = term.hide_cursor();
                        dirty = true;
                    }
                }
                In::Msg(g, ToServer::Event(e)) if Some(g) == cur => {
                    if let Event::Resize(w, h) = e {
                        // the next draw sees the new size and repaints everything
                        term.backend_mut().size = Size::new(w, h);
                    }
                    if let Some(d) = &mut desk {
                        d.event(e);
                        dirty = true;
                    }
                }
                In::Gone(g) => {
                    waiting.remove(&g);
                    if Some(g) == cur {
                        term.backend_mut().out = None;
                        cur = None;
                    }
                }
                _ => {}
            }
        }
        let Some(d) = &mut desk else { continue };
        dirty |= d.tick();
        if d.quit {
            return finish(&mut term);
        }
        if d.detach {
            d.detach = false;
            term.backend_mut().put(ToClient::Detached);
            let _ = term.backend_mut().flush();
            term.backend_mut().out = None;
            cur = None;
        }
        let raw = std::mem::take(&mut *OUTBOX.lock().unwrap());
        if !raw.is_empty() {
            term.backend_mut().put(ToClient::Raw(raw));
        }
        if dirty && last.elapsed() >= frame && term.backend().out.is_some() {
            term.draw(|f| d.render(f))?;
            last = Instant::now();
            dirty = false;
        }
    }
    finish(&mut term)
}

fn finish(term: &mut Terminal<Remote>) -> anyhow::Result<()> {
    term.backend_mut().put(ToClient::Bye);
    let _ = term.backend_mut().flush();
    let _ = std::fs::remove_file(socket());
    Ok(())
}

// ---------------------------------------------------------------- client

/// Starts the server in the background, on its own away from this terminal.
fn start_server() -> io::Result<()> {
    let exe = std::env::current_exe()?;
    let mut cmd = Command::new(exe);
    cmd.arg("--server").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    cmd.spawn()?;
    Ok(())
}

fn connect(start: bool) -> io::Result<UnixStream> {
    if let Ok(s) = UnixStream::connect(socket()) {
        return Ok(s);
    }
    if !start {
        return Err(io::Error::new(io::ErrorKind::NotFound, "not running"));
    }
    start_server()?;
    let t = Instant::now();
    loop {
        std::thread::sleep(Duration::from_millis(20));
        match UnixStream::connect(socket()) {
            Ok(s) => return Ok(s),
            Err(e) if t.elapsed() > Duration::from_secs(5) => return Err(e),
            Err(_) => {}
        }
    }
}

/// Opens the desktop in this terminal, starting it if it isn't running.
pub fn attach(tiling: Option<bool>) -> anyhow::Result<()> {
    let stream = connect(true)?;
    let mut w = BufWriter::new(stream.try_clone()?);
    let mut r = stream;

    let terminal = ratatui::init();
    execute!(io::stdout(), EnableMouseCapture, EnableBracketedPaste)?;
    let kb = crossterm::terminal::supports_keyboard_enhancement().unwrap_or(false);
    if kb {
        execute!(io::stdout(), PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES))?;
    }
    let size = terminal.size()?;
    send(&mut w, &ToServer::Hello { w: size.width, h: size.height, tiling, build: build() })?;
    w.flush()?;

    // paint what the server sends until it says goodbye
    let (done_tx, done) = mpsc::channel();
    std::thread::spawn(move || {
        let mut screen = CrosstermBackend::new(io::stdout());
        let why = loop {
            let Ok(m) = recv::<ToClient>(&mut r) else { break ToClient::Bye };
            let _ = match m {
                ToClient::Draw(cells) => screen.draw(cells.iter().map(|(x, y, c)| (*x, *y, c))),
                ToClient::HideCursor => screen.hide_cursor(),
                ToClient::ShowCursor => screen.show_cursor(),
                ToClient::Cursor(x, y) => screen.set_cursor_position(Position::new(x, y)),
                ToClient::Clear => screen.clear(),
                ToClient::Flush => Backend::flush(&mut screen),
                ToClient::Raw(b) => {
                    let mut o = io::stdout();
                    let _ = o.write_all(&b);
                    o.flush()
                }
                m @ (ToClient::Bye | ToClient::Detached) => break m,
            };
        };
        let _ = done_tx.send(why);
    });

    let why = loop {
        if let Ok(why) = done.try_recv() {
            break why;
        }
        if event::poll(Duration::from_millis(50))? {
            let e = event::read()?;
            if send(&mut w, &ToServer::Event(e)).and_then(|_| w.flush()).is_err() {
                break ToClient::Bye;
            }
        }
    };

    if kb {
        let _ = execute!(io::stdout(), PopKeyboardEnhancementFlags);
    }
    let _ = execute!(io::stdout(), DisableMouseCapture, DisableBracketedPaste);
    ratatui::restore();
    if matches!(why, ToClient::Detached) {
        println!("win95 is still running. Type win95 to come back to it.");
    }
    Ok(())
}

/// `win95 stop`: quits the desktop and everything in it.
pub fn stop() -> anyhow::Result<()> {
    match connect(false) {
        Ok(s) => {
            let mut w = BufWriter::new(s);
            send(&mut w, &ToServer::Stop)?;
            w.flush()?;
            println!("Stopped win95.");
        }
        Err(_) => println!("win95 isn't running."),
    }
    Ok(())
}
