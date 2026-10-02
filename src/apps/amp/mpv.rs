// mpv does the playing. It runs in the background with no window and we talk
// to it over its JSON socket: commands go one way, property changes and
// end-of-file events come back on a channel.
use anyhow::{bail, Result};
use serde_json::{json, Value};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::mpsc::{channel, Receiver},
    thread,
    time::Duration,
};

pub enum Ev {
    Prop(String, Value),
    /// why the last file stopped: eof, error, stop, quit...
    EndFile(String),
    FileLoaded,
    Gone,
}

pub struct Mpv {
    child: Child,
    sock: UnixStream,
    path: PathBuf,
    pub rx: Receiver<Ev>,
}

const WATCH: [&str; 8] = ["time-pos", "duration", "pause", "media-title", "audio-bitrate", "audio-params/samplerate", "audio-params/channel-count", "metadata/by-key/icy-title"];

impl Mpv {
    pub fn start(volume: u8) -> Result<Mpv> {
        let dir = std::env::var("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(|_| std::env::temp_dir());
        let path = dir.join(format!("win95-amp-{}.sock", std::process::id()));
        let _ = fs::remove_file(&path);
        let mut child = Command::new("mpv")
            .args(["--idle=yes", "--no-video", "--no-terminal", "--audio-display=no", "--force-window=no", "--audio-client-name=win95-amp"])
            .arg(format!("--volume={volume}"))
            .arg(format!("--input-ipc-server={}", path.display()))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| anyhow::anyhow!("can't start mpv: {e}"))?;
        let mut sock = None;
        for _ in 0..100 {
            if let Ok(s) = UnixStream::connect(&path) {
                sock = Some(s);
                break;
            }
            if let Ok(Some(_)) = child.try_wait() {
                bail!("mpv stopped as it started");
            }
            thread::sleep(Duration::from_millis(30));
        }
        let Some(sock) = sock else {
            let _ = child.kill();
            bail!("mpv didn't open its socket");
        };
        let (tx, rx) = channel();
        let read = sock.try_clone()?;
        thread::spawn(move || {
            for line in BufReader::new(read).lines() {
                let Ok(line) = line else { break };
                let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
                let ev = match v["event"].as_str() {
                    Some("property-change") => Ev::Prop(v["name"].as_str().unwrap_or("").into(), v["data"].clone()),
                    Some("end-file") => Ev::EndFile(v["reason"].as_str().unwrap_or("").into()),
                    Some("file-loaded") => Ev::FileLoaded,
                    _ => continue,
                };
                if tx.send(ev).is_err() {
                    return;
                }
            }
            let _ = tx.send(Ev::Gone);
        });
        let mut m = Mpv { child, sock, path, rx };
        for (i, p) in WATCH.iter().enumerate() {
            m.cmd(json!(["observe_property", i + 1, p]));
        }
        Ok(m)
    }

    pub fn cmd(&mut self, c: Value) {
        let mut line = json!({ "command": c }).to_string();
        line.push('\n');
        let _ = self.sock.write_all(line.as_bytes());
    }

    pub fn load(&mut self, loc: &str) {
        self.cmd(json!(["loadfile", loc, "replace"]));
        self.set("pause", json!(false));
    }

    pub fn set(&mut self, prop: &str, v: Value) {
        self.cmd(json!(["set_property", prop, v]));
    }

    pub fn seek(&mut self, secs: f64, absolute: bool) {
        self.cmd(json!(["seek", secs, if absolute { "absolute" } else { "relative" }]));
    }

    pub fn stop(&mut self) {
        self.cmd(json!(["stop"]));
    }
}

impl Drop for Mpv {
    fn drop(&mut self) {
        self.cmd(json!(["quit"]));
        thread::sleep(Duration::from_millis(50));
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_file(&self.path);
    }
}
