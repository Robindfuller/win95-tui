// A hidden Chromium that opens pages for us, so sites that turn away simple
// fetchers or build themselves with JavaScript still come through. One stays
// running, spoken to over a pipe (the DevTools protocol), and we only ever
// take the finished page's HTML from it (and, for AI-assisted mode, a picture
// of it and a map of where everything sits).
use serde_json::{json, Value};
use std::{
    fs::File,
    io::{BufRead, BufReader, Write},
    os::{fd::FromRawFd, unix::process::CommandExt},
    process::{Child, Command, Stdio},
    sync::{
        mpsc::{channel, Receiver, RecvTimeoutError},
        Mutex,
    },
    time::{Duration, Instant},
};
use url::Url;

struct Chrome {
    child: Child,
    to: File,
    from: Receiver<Value>,
    id: u64,
}

/// What a page looks like: a JPEG of its top part, base64, and look.js's map.
#[derive(Clone, Debug)]
pub struct Look {
    pub shot: String,
    pub map: String,
}

/// how wide the page is laid out when we look at it, as on a laptop
const LOOK_W: u32 = 1280;

static CHROME: Mutex<Option<Chrome>> = Mutex::new(None);

fn binary() -> Option<&'static str> {
    ["chromium", "google-chrome-stable", "google-chrome", "chromium-browser"].into_iter().find(|b| {
        std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(b).is_file()))
    })
}

fn version(bin: &str) -> String {
    let out = Command::new(bin).arg("--version").output().ok();
    let s = out.map(|o| String::from_utf8_lossy(&o.stdout).to_string()).unwrap_or_default();
    s.split_whitespace().find_map(|w| w.split('.').next()?.parse::<u32>().ok()).unwrap_or(140).to_string()
}

fn pipe() -> Option<(i32, i32)> {
    let mut fds = [0; 2];
    (unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } == 0).then_some((fds[0], fds[1]))
}

impl Chrome {
    fn start() -> Option<Chrome> {
        let bin = binary()?;
        let profile = crate::theme::home().join(".cache/win95-tui/browser");
        let _ = std::fs::create_dir_all(&profile);
        let ua = format!("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{}.0.0.0 Safari/537.36", version(bin));
        // it reads our commands on fd 3 and answers on fd 4
        let (cmd_r, cmd_w) = pipe()?;
        let (ans_r, ans_w) = pipe()?;
        let mut c = Command::new(bin);
        c.args([
            "--headless=new",
            "--remote-debugging-pipe",
            "--disable-gpu",
            "--no-first-run",
            "--no-default-browser-check",
            "--mute-audio",
            "--disable-blink-features=AutomationControlled",
            "--blink-settings=imagesEnabled=false",
        ])
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg(format!("--user-agent={ua}"))
        .arg("about:blank")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
        unsafe {
            c.pre_exec(move || {
                for (from, to) in [(cmd_r, 3), (ans_w, 4)] {
                    if from == to {
                        libc::fcntl(to, libc::F_SETFD, 0);
                    } else if libc::dup2(from, to) < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                }
                // it goes when the desktop does
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
                Ok(())
            });
        }
        let child = c.spawn().ok();
        unsafe {
            libc::close(cmd_r);
            libc::close(ans_w);
        }
        let child = child?;
        let to = unsafe { File::from_raw_fd(cmd_w) };
        let from_file = unsafe { File::from_raw_fd(ans_r) };
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let mut r = BufReader::new(from_file);
            let mut buf = vec![];
            while r.read_until(0, &mut buf).is_ok_and(|n| n > 0) {
                buf.pop_if(|b| *b == 0);
                if let Ok(v) = serde_json::from_slice::<Value>(&buf) {
                    if tx.send(v).is_err() {
                        break;
                    }
                }
                buf.clear();
            }
        });
        Some(Chrome { child, to, from: rx, id: 0 })
    }

    fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    fn send(&mut self, method: &str, params: Value, session: Option<&str>) -> Option<u64> {
        self.id += 1;
        let mut msg = json!({ "id": self.id, "method": method, "params": params });
        if let Some(s) = session {
            msg["sessionId"] = json!(s);
        }
        let mut bytes = serde_json::to_vec(&msg).ok()?;
        bytes.push(0);
        self.to.write_all(&bytes).ok()?;
        Some(self.id)
    }

    /// Sends a command and waits for its answer, noting events on the way.
    fn call(&mut self, method: &str, params: Value, session: Option<&str>, until: Instant, events: &mut Vec<String>) -> Option<Value> {
        let id = self.send(method, params, session)?;
        loop {
            let v = self.next(until)?;
            if v["id"].as_u64() == Some(id) {
                return v.get("result").cloned();
            }
            if let Some(m) = v["method"].as_str() {
                events.push(m.to_string());
            }
        }
    }

    fn next(&mut self, until: Instant) -> Option<Value> {
        let left = until.checked_duration_since(Instant::now())?;
        match self.from.recv_timeout(left) {
            Ok(v) => Some(v),
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => None,
        }
    }

    /// Opens a page in a fresh tab and returns where it ended up and its HTML.
    fn open(&mut self, url: &Url, look: bool) -> Option<(Url, String, Option<Look>)> {
        let until = Instant::now() + Duration::from_secs(25);
        let mut ev = vec![];
        let target = self.call("Target.createTarget", json!({ "url": "about:blank" }), None, until, &mut ev)?["targetId"].as_str()?.to_string();
        let got = self.load(&target, url, until, look);
        let _ = self.send("Target.closeTarget", json!({ "targetId": target }), None);
        got
    }

    fn load(&mut self, target: &str, url: &Url, until: Instant, look: bool) -> Option<(Url, String, Option<Look>)> {
        let mut ev = vec![];
        let session = self.call("Target.attachToTarget", json!({ "targetId": target, "flatten": true }), None, until, &mut ev)?["sessionId"].as_str()?.to_string();
        let s = Some(session.as_str());
        self.call("Page.enable", json!({}), s, until, &mut ev)?;
        if look {
            self.call("Emulation.setDeviceMetricsOverride", json!({ "width": LOOK_W, "height": 900, "deviceScaleFactor": 1, "mobile": false }), s, until, &mut ev)?;
        }
        ev.clear();
        let nav = self.call("Page.navigate", json!({ "url": url.as_str() }), s, until, &mut ev)?;
        if nav.get("errorText").and_then(Value::as_str).is_some_and(|e| !e.is_empty()) {
            return None;
        }
        // wait for the page to load, but not for every last advert: once its
        // HTML is in, give scripts a moment and take what's there
        let mut ready: Option<Instant> = None;
        loop {
            if ev.iter().any(|e| e == "Page.loadEventFired") {
                break;
            }
            if ready.is_none() && ev.iter().any(|e| e == "Page.domContentEventFired") {
                ready = Some(Instant::now());
            }
            let stop = ready.map_or(until, |r| (r + Duration::from_millis(2500)).min(until));
            match self.next(stop) {
                Some(v) => {
                    if v["sessionId"].as_str() == s {
                        if let Some(m) = v["method"].as_str() {
                            ev.push(m.to_string());
                        }
                    }
                }
                None if ready.is_some() => break,
                None => return None,
            }
        }
        std::thread::sleep(Duration::from_millis(300));
        // a "checking your browser" page clears itself after a few seconds,
        // and some pages only fill in once their scripts have run
        let (start, wait) = (Instant::now(), Instant::now() + Duration::from_secs(15));
        let mut waited = false;
        while Instant::now() < wait {
            let expr = "document.readyState + '|' + (document.body ? document.body.innerText.length : 0) + '|' + document.title";
            let t = self.call("Runtime.evaluate", json!({ "expression": expr, "returnByValue": true }), s, wait, &mut ev);
            let t = t.as_ref().and_then(|t| t["result"]["value"].as_str()).unwrap_or("").to_lowercase();
            let mut parts = t.splitn(3, '|');
            let (state, len, title) = (parts.next().unwrap_or(""), parts.next().and_then(|n| n.parse::<usize>().ok()).unwrap_or(0), parts.next().unwrap_or(""));
            let check = ["just a moment", "attention required", "checking your browser", "please wait", "one moment"].iter().any(|c| title.contains(c));
            let thin = len < 200 && start.elapsed() < Duration::from_secs(6);
            if !check && !thin && state != "loading" {
                if waited {
                    // the real page has just arrived; let it settle
                    std::thread::sleep(Duration::from_millis(800));
                }
                break;
            }
            waited = true;
            std::thread::sleep(Duration::from_millis(400));
        }
        let until = until.max(Instant::now() + Duration::from_secs(5));
        let r = self.call(
            "Runtime.evaluate",
            json!({ "expression": "JSON.stringify([location.href, document.documentElement.outerHTML])", "returnByValue": true }),
            s,
            until,
            &mut ev,
        )?;
        let pair: Vec<String> = serde_json::from_str(r["result"]["value"].as_str()?).ok()?;
        let fin = Url::parse(pair.first()?).unwrap_or_else(|_| url.clone());
        let look = if look { self.look(s, until) } else { None };
        Some((fin, pair.get(1)?.clone(), look))
    }

    fn look(&mut self, s: Option<&str>, until: Instant) -> Option<Look> {
        let mut ev = vec![];
        let until = until.max(Instant::now() + Duration::from_secs(8));
        let r = self.call("Runtime.evaluate", json!({ "expression": include_str!("look.js"), "returnByValue": true }), s, until, &mut ev)?;
        let map = r["result"]["value"].as_str()?.to_string();
        let h = serde_json::from_str::<Value>(&map).ok().and_then(|v| v["h"].as_f64()).unwrap_or(900.0).clamp(300.0, 2400.0);
        let clip = json!({ "x": 0, "y": 0, "width": LOOK_W, "height": h, "scale": 0.6 });
        let shot = self.call("Page.captureScreenshot", json!({ "format": "jpeg", "quality": 60, "clip": clip, "captureBeyondViewport": true }), s, until, &mut ev)?;
        Some(Look { shot: shot["data"].as_str()?.to_string(), map })
    }
}

/// Opens a web page with Chromium, when it's installed. One page at a time.
/// With `look`, it also says what the page looks like.
pub fn get(url: &Url, look: bool) -> Option<(Url, String, Option<Look>)> {
    let mut slot = CHROME.lock().unwrap_or_else(|e| e.into_inner());
    if !slot.as_mut().is_some_and(|c| c.alive()) {
        *slot = Chrome::start();
    }
    let got = slot.as_mut()?.open(url, look);
    if got.is_none() {
        // it may have wedged: start afresh next time
        if let Some(mut c) = slot.take() {
            let _ = c.child.kill();
            let _ = c.child.wait();
        }
    }
    got
}

#[cfg(test)]
mod tests {
    #[test]
    #[ignore]
    fn raw() {
        let u = url::Url::parse(&std::env::var("BROWSE").unwrap()).unwrap();
        match super::get(&u, false) {
            Some((fin, html, _)) => {
                let title = html.split("<title").nth(1).and_then(|t| t.split('>').nth(1)).and_then(|t| t.split('<').next()).unwrap_or("");
                println!("final={fin} len={} title={title:?}", html.len());
            }
            None => println!("none"),
        }
    }
}
