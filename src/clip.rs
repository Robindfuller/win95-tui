//! The desktop clipboard, through wl-clipboard. Over SSH there's no Wayland,
//! so copied text goes to your own terminal instead (OSC 52), and pasting
//! is left to the terminal's own paste key.

use std::{
    io::Write,
    process::{Command, Stdio},
};

/// Put `body` on the clipboard as `mime` (plain text if None).
pub fn wl_copy(mime: Option<&str>, body: &str) -> Result<(), String> {
    let mut cmd = Command::new("wl-copy");
    if let Some(t) = mime {
        cmd.args(["-t", t]);
    }
    // wl-copy stays behind to serve the clipboard, holding on to any
    // pipe it was given, so only stdin is one
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("Can't run wl-copy: {e}"))?;
    child.stdin.take().unwrap().write_all(body.as_bytes()).map_err(|e| e.to_string())?;
    match child.wait() {
        Ok(s) if s.success() => Ok(()),
        _ => Err("wl-copy failed. Is this running over SSH?".into()),
    }
}

/// What's on the clipboard as `mime` (plain text if None).
pub fn wl_paste(mime: Option<&str>) -> Option<String> {
    let mut cmd = Command::new("wl-paste");
    cmd.arg("-n");
    if let Some(t) = mime {
        cmd.args(["-t", t]);
    }
    let out = cmd.stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

pub fn copy_text(s: &str) {
    if std::env::var_os("WAYLAND_DISPLAY").is_some() && wl_copy(None, s).is_ok() {
        return;
    }
    let osc = format!("\x1b]52;c;{}\x07", base64(s.as_bytes()));
    // in a session it goes to whichever terminal is attached
    if crate::session::SERVER.load(std::sync::atomic::Ordering::Relaxed) {
        crate::session::OUTBOX.lock().unwrap().extend_from_slice(osc.as_bytes());
        return;
    }
    let mut out = std::io::stdout();
    let _ = write!(out, "{osc}");
    let _ = out.flush();
}

pub fn paste_text() -> Option<String> {
    wl_paste(None)
}

fn base64(b: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(b.len().div_ceil(3) * 4);
    for c in b.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            s.push(if i <= c.len() { A[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' });
        }
    }
    s
}

#[cfg(test)]
mod tests {
    #[test]
    fn base64() {
        assert_eq!(super::base64(b""), "");
        assert_eq!(super::base64(b"f"), "Zg==");
        assert_eq!(super::base64(b"fo"), "Zm8=");
        assert_eq!(super::base64(b"foo"), "Zm9v");
        assert_eq!(super::base64("héllo\n".as_bytes()), "aMOpbGxvCg==");
    }
}
