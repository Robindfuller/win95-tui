// The speaker volume, read and set with PipeWire's wpctl.
use std::process::{Command, Stdio};

const SINK: &str = "@DEFAULT_AUDIO_SINK@";

/// The volume as a percentage, and whether it's muted. None without wpctl.
pub fn get() -> Option<(u8, bool)> {
    let out = Command::new("wpctl").args(["get-volume", SINK]).stderr(Stdio::null()).output().ok()?;
    parse(&String::from_utf8_lossy(&out.stdout))
}

/// Reads "Volume: 0.35" or "Volume: 0.35 [MUTED]".
fn parse(s: &str) -> Option<(u8, bool)> {
    let v: f32 = s.split_whitespace().nth(1)?.parse().ok()?;
    Some(((v * 100.0).round().clamp(0.0, 100.0) as u8, s.contains("[MUTED]")))
}

pub fn set(pct: u8) {
    run(&["set-volume", "-l", "1.0", SINK, &format!("{}%", pct.min(100))]);
}

pub fn set_mute(on: bool) {
    run(&["set-mute", SINK, if on { "1" } else { "0" }]);
}

fn run(args: &[&str]) {
    // tests never touch the real speaker
    if cfg!(test) {
        return;
    }
    let _ = Command::new("wpctl").args(args).stdout(Stdio::null()).stderr(Stdio::null()).status();
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_wpctl() {
        assert_eq!(super::parse("Volume: 0.35\n"), Some((35, false)));
        assert_eq!(super::parse("Volume: 1.00 [MUTED]\n"), Some((100, true)));
        assert_eq!(super::parse(""), None);
    }
}
