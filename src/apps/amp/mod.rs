// Amp: a Winamp-style music and internet radio player. mpv does the playing;
// the player, equaliser and playlist or radio tabs stack in one window.
mod library;
mod mpv;
mod radio;
mod vis;

use super::{Action, App, Launch};
use crate::{
    draw::{st, Canvas},
    icons::Icon,
    menu::{Cmd, Item, Sys},
    theme::{home, Theme},
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind};
use library::{Entry, Prober, Settings};
use mpv::{Ev, Mpv};
use radio::{Station, GENRES};
use ratatui::style::{Color, Modifier};
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    sync::mpsc::{channel, Receiver},
    thread,
    time::{Duration, Instant, SystemTime},
};
use unicode_width::UnicodeWidthChar;
use vis::Vis;


const FREQS: [&str; 10] = ["60", "170", "310", "600", "1k", "3k", "6k", "12k", "14k", "16k"];
const HZ: [u32; 10] = [60, 170, 310, 600, 1000, 3000, 6000, 12000, 14000, 16000];

const PRESETS: [(&str, [f32; 10]); 9] = [
    ("Flat", [0.0; 10]),
    ("Rock", [5.0, 3.0, -2.0, -4.0, -1.0, 2.0, 5.0, 6.0, 6.0, 6.0]),
    ("Pop", [-1.0, 3.0, 5.0, 5.0, 3.0, 0.0, -1.0, -1.0, -1.0, -1.0]),
    ("Jazz", [3.0, 2.0, 0.0, 2.0, -2.0, -2.0, 0.0, 2.0, 3.0, 4.0]),
    ("Classical", [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, -4.0, -4.0, -4.0, -6.0]),
    ("Dance", [6.0, 5.0, 1.0, 0.0, 0.0, -3.0, -4.0, -4.0, 0.0, 0.0]),
    ("Bass boost", [7.0, 6.0, 4.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
    ("Treble boost", [0.0, 0.0, 0.0, 0.0, 0.0, 2.0, 5.0, 7.0, 8.0, 8.0]),
    ("Vocal", [-2.0, -3.0, -2.0, 1.0, 4.0, 4.0, 3.0, 1.0, 0.0, -2.0]),
];
// menu commands are static strings, so each preset and genre has its own
const PRESET_CMDS: [&str; 9] = ["preset0", "preset1", "preset2", "preset3", "preset4", "preset5", "preset6", "preset7", "preset8"];
const GENRE_CMDS: [&str; 14] = ["genre0", "genre1", "genre2", "genre3", "genre4", "genre5", "genre6", "genre7", "genre8", "genre9", "genre10", "genre11", "genre12", "genre13"];

/// Rows the player takes, the equaliser adds, and the playlist adds at least.
const PLAYER_H: u16 = 10;
const EQ_H: u16 = 8;
const LIST_H: u16 = 14;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Hit {
    Btn(&'static str),
    Seek,
    Vol,
    Bal,
    Eq(usize),
    Preamp,
    Row(usize),
    Station(usize),
    File(usize),
    Search,
    List,
    Vis,
    Time,
    Display,
    Dialog,
}

#[derive(Clone, Copy, PartialEq)]
enum Drag {
    Seek,
    Vol,
    Bal,
    Eq(usize),
    Preamp,
}

#[derive(PartialEq)]
enum Kind {
    Up,
    Dir,
    File,
}

struct Browser {
    dir: PathBuf,
    items: Vec<(PathBuf, Kind)>,
    sel: usize,
    scroll: usize,
}

impl Browser {
    fn open(dir: PathBuf) -> Browser {
        let mut items = vec![];
        if let Some(up) = dir.parent() {
            items.push((up.to_path_buf(), Kind::Up));
        }
        let mut found: Vec<PathBuf> = std::fs::read_dir(&dir).map(|rd| rd.flatten().map(|e| e.path()).filter(|p| !library::hidden(p)).collect()).unwrap_or_default();
        found.sort_by_key(|p| (!p.is_dir(), p.file_name().map(|n| n.to_string_lossy().to_lowercase())));
        for p in found {
            if p.is_dir() {
                items.push((p, Kind::Dir));
            } else if library::is_audio(&p) {
                items.push((p, Kind::File));
            }
        }
        Browser { dir, items, sel: 0, scroll: 0 }
    }
}

enum Overlay {
    None,
    Browse(Browser),
    Url(String),
}

pub struct Amp {
    s: Settings,
    mpv: Option<Mpv>,
    err: Option<String>,
    vis: Vis,
    list: Vec<Entry>,
    /// the playlist row playing, if what's playing came from the playlist
    cur: Option<usize>,
    sel: usize,
    scroll: usize,
    now: Option<Entry>,
    playing: bool,
    paused: bool,
    pos: f64,
    dur: Option<f64>,
    icy: Option<String>,
    kbps: Option<u32>,
    khz: Option<u32>,
    chans: Option<u32>,
    query: String,
    searching: bool,
    genre: usize,
    stations: Vec<Station>,
    st_sel: usize,
    st_scroll: usize,
    show_favs: bool,
    st_note: String,
    radio_rx: Option<Receiver<anyhow::Result<Vec<Station>>>>,
    favs: Vec<Station>,
    prober: Prober,
    overlay: Overlay,
    drag: Option<Drag>,
    hits: Vec<(i32, i32, i32, i32, Hit)>,
    /// rows the list on show has room for, set when it's drawn
    rows: usize,
    /// the selections last drawn, to scroll a moved one into view
    seen: [usize; 2],
    clock: Instant,
    frame: Instant,
    click: Option<(Instant, Hit)>,
    eq_due: Option<Instant>,
    fails: usize,
    status: Option<(String, Instant)>,
    /// the EQ slider last touched, to show its value
    eq_touch: Option<usize>,
    size: (u16, u16),
    rng: u64,
}

fn fmt_time(s: f64) -> String {
    let s = s.max(0.0) as u64;
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

impl Amp {
    pub fn new(files: Vec<PathBuf>) -> Amp {
        let s = library::load_settings();
        let (mpv, err) = match Mpv::start(s.volume) {
            Ok(m) => (Some(m), None),
            Err(e) => (None, Some(format!("{e}. Amp needs mpv installed."))),
        };
        let favs = library::load_favs();
        let seed = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(7) | 1;
        let mut a = Amp {
            mpv,
            err,
            vis: Vis::new(),
            list: library::load_playlist(),
            cur: None,
            sel: 0,
            scroll: 0,
            now: None,
            playing: false,
            paused: false,
            pos: 0.0,
            dur: None,
            icy: None,
            kbps: None,
            khz: None,
            chans: None,
            query: String::new(),
            searching: false,
            genre: 0,
            stations: vec![],
            st_sel: 0,
            st_scroll: 0,
            show_favs: !favs.is_empty(),
            st_note: String::new(),
            radio_rx: None,
            favs,
            prober: Prober::new(),
            overlay: Overlay::None,
            drag: None,
            hits: vec![],
            rows: 10,
            seen: [usize::MAX; 2],
            clock: Instant::now(),
            frame: Instant::now(),
            click: None,
            eq_due: None,
            fails: 0,
            status: None,
            eq_touch: None,
            size: (0, 0),
            rng: seed,
            s,
        };
        a.s.compact = false;
        for e in a.list.iter().filter(|e| !e.stream && e.dur.is_none()) {
            a.prober.ask(&e.loc);
        }
        a.apply_af();
        a.search();
        a.open_files(&files);
        a
    }

    /// Adds files or folders to the playlist and plays the first of them.
    fn open_files(&mut self, files: &[PathBuf]) {
        let mut es = vec![];
        for f in files {
            if f.is_dir() {
                es.extend(library::scan(f).iter().map(|p| Entry::file(p)));
            } else {
                es.push(Entry::file(f));
            }
        }
        if !es.is_empty() {
            let i = self.add(es);
            self.play(i);
        }
    }

    fn save(&self) {
        library::save_settings(&self.s);
        library::save_playlist(&self.list);
    }

    fn say(&mut self, msg: impl Into<String>) {
        self.status = Some((msg.into(), Instant::now()));
    }

    fn rand(&mut self) -> u64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        self.rng
    }

    // ------------------------------------------------------------ playing

    fn apply_af(&mut self) {
        let mut f = vec![];
        if self.s.eq_on {
            if self.s.preamp.abs() > 0.05 {
                f.push(format!("volume={:.1}dB", self.s.preamp));
            }
            for (i, g) in self.s.eq.iter().enumerate() {
                if g.abs() > 0.05 {
                    f.push(format!("equalizer=f={}:t=o:w=1:g={:.1}", HZ[i], g));
                }
            }
        }
        if self.s.balance.abs() > 0.01 {
            f.push(format!("stereotools=balance_out={:.2}", self.s.balance));
        }
        let af = if f.is_empty() { String::new() } else { format!("lavfi=[{}]", f.join(",")) };
        if let Some(m) = &mut self.mpv {
            m.set("af", json!(af));
        }
    }

    fn eq_changed(&mut self) {
        self.eq_due = Some(Instant::now());
    }

    fn set_volume(&mut self, v: i32) {
        self.s.volume = v.clamp(0, 100) as u8;
        let v = self.s.volume;
        if let Some(m) = &mut self.mpv {
            m.set("volume", json!(v));
        }
    }

    fn play_entry(&mut self, e: Entry, idx: Option<usize>) {
        let Some(m) = &mut self.mpv else {
            let e = self.err.clone().unwrap_or_else(|| "mpv isn't running".into());
            return self.say(e);
        };
        m.load(&e.loc);
        self.cur = idx;
        self.now = Some(e);
        (self.pos, self.dur, self.icy, self.kbps, self.khz, self.chans) = (0.0, None, None, None, None, None);
        self.playing = true;
        self.paused = false;
        self.clock = Instant::now();
        if self.s.vis < 2 {
            self.vis.start();
        }
    }

    fn play(&mut self, i: usize) {
        if let Some(e) = self.list.get(i).cloned() {
            self.sel = i;
            self.play_entry(e, Some(i));
        }
    }

    fn stop(&mut self) {
        if let Some(m) = &mut self.mpv {
            m.stop();
        }
        self.playing = false;
        self.paused = false;
        self.pos = 0.0;
        self.vis.stop();
    }

    fn pause(&mut self) {
        if !self.playing {
            return self.play_selected();
        }
        self.paused = !self.paused;
        let p = self.paused;
        if let Some(m) = &mut self.mpv {
            m.set("pause", json!(p));
        }
        if p {
            self.vis.stop()
        } else if self.s.vis < 2 {
            self.vis.start()
        }
    }

    fn radio_shown(&self) -> bool {
        self.s.radio_tab && self.s.show_pl && !self.s.compact
    }

    fn play_selected(&mut self) {
        if self.radio_shown() {
            if let Some(st) = self.shown_stations().get(self.st_sel).cloned() {
                return self.play_entry(Entry::station(&st), None);
            }
        }
        if !self.list.is_empty() {
            self.play(self.sel.min(self.list.len() - 1));
        }
    }

    fn next(&mut self) {
        if self.list.is_empty() {
            return;
        }
        let n = self.list.len();
        let i = match self.cur {
            _ if self.s.shuffle && n > 1 => {
                let mut i = (self.rand() % n as u64) as usize;
                if Some(i) == self.cur {
                    i = (i + 1) % n;
                }
                i
            }
            Some(c) if c + 1 < n => c + 1,
            Some(_) if self.s.repeat => 0,
            Some(_) => return self.stop(),
            None => self.sel.min(n - 1),
        };
        self.play(i);
    }

    fn prev(&mut self) {
        let stream = self.now.as_ref().is_some_and(|e| e.stream);
        if self.playing && self.pos > 3.0 && !stream {
            if let Some(m) = &mut self.mpv {
                m.seek(0.0, true);
            }
            return;
        }
        let n = self.list.len();
        match self.cur {
            Some(0) if self.s.repeat && n > 0 => self.play(n - 1),
            Some(c) if c > 0 => self.play(c - 1),
            Some(c) => self.play(c),
            None if n > 0 => self.play(self.sel.min(n - 1)),
            None => {}
        }
    }

    fn seek_to(&mut self, frac: f64) {
        if let (Some(d), true, Some(m)) = (self.dur, self.playing, &mut self.mpv) {
            let t = (frac.clamp(0.0, 1.0) * d).min(d - 0.5).max(0.0);
            self.pos = t;
            m.seek(t, true);
        }
    }

    fn seek_by(&mut self, secs: f64) {
        if let (true, Some(_), Some(m)) = (self.playing, self.dur, &mut self.mpv) {
            m.seek(secs, false);
        }
    }

    // ------------------------------------------------------------ playlist

    /// Adds entries to the end of the playlist and returns where they start.
    fn add(&mut self, es: Vec<Entry>) -> usize {
        let start = self.list.len();
        for e in &es {
            if !e.stream && e.dur.is_none() {
                self.prober.ask(&e.loc);
            }
        }
        let n = es.len();
        self.list.extend(es);
        if n > 0 {
            self.say(format!("Added {n} {}", if n == 1 { "track" } else { "tracks" }));
            library::save_playlist(&self.list);
        }
        start
    }

    fn remove(&mut self) {
        if self.sel >= self.list.len() {
            return;
        }
        self.list.remove(self.sel);
        self.cur = match self.cur {
            Some(c) if c == self.sel => None,
            Some(c) if c > self.sel => Some(c - 1),
            c => c,
        };
        self.sel = self.sel.min(self.list.len().saturating_sub(1));
        library::save_playlist(&self.list);
    }

    fn sort(&mut self) {
        let playing = self.cur.map(|c| self.list[c].loc.clone());
        self.list.sort_by_key(|e| e.title.to_lowercase());
        self.cur = playing.and_then(|l| self.list.iter().position(|e| e.loc == l));
        library::save_playlist(&self.list);
    }

    // ------------------------------------------------------------ radio

    fn shown_stations(&self) -> &Vec<Station> {
        if self.show_favs { &self.favs } else { &self.stations }
    }

    fn search(&mut self) {
        let (q, g) = (self.query.clone(), GENRES[self.genre].to_string());
        let (tx, rx) = channel();
        thread::spawn(move || {
            let _ = tx.send(radio::search(&q, &g));
        });
        self.radio_rx = Some(rx);
        self.st_note = "Looking for stations...".into();
    }

    fn run_search(&mut self) {
        self.searching = false;
        self.show_favs = false;
        (self.st_sel, self.st_scroll) = (0, 0);
        self.stations.clear();
        self.search();
    }

    fn fav(&mut self) {
        let st = if self.radio_shown() {
            self.shown_stations().get(self.st_sel).cloned()
        } else {
            self.now.as_ref().filter(|e| e.stream).map(|e| Station { name: e.title.clone(), url: e.loc.clone(), bitrate: self.kbps.unwrap_or(0), codec: String::new(), country: String::new() })
        };
        let Some(st) = st else { return };
        if let Some(i) = self.favs.iter().position(|f| f.url == st.url) {
            self.favs.remove(i);
            self.say(format!("Took {} out of favourites", st.name));
            if self.show_favs {
                self.st_sel = self.st_sel.min(self.favs.len().saturating_sub(1));
            }
        } else {
            self.say(format!("{} is a favourite", st.name));
            self.favs.push(st);
        }
        library::save_favs(&self.favs);
    }

    fn is_fav(&self, url: &str) -> bool {
        self.favs.iter().any(|f| f.url == url)
    }

    // ------------------------------------------------------------ background work

    /// Takes in what mpv, ffprobe and the radio search have sent.
    fn update(&mut self) -> bool {
        let mut dirty = false;
        let mut evs = vec![];
        if let Some(m) = &self.mpv {
            while let Ok(e) = m.rx.try_recv() {
                evs.push(e);
            }
        }
        for e in evs {
            dirty = true;
            match e {
                Ev::Prop(name, v) => self.prop(&name, v),
                Ev::FileLoaded => self.fails = 0,
                Ev::EndFile(r) => self.ended(&r),
                Ev::Gone => {
                    self.mpv = None;
                    self.err = Some("mpv stopped. Close Amp and open it again.".into());
                    self.playing = false;
                    self.vis.stop();
                }
            }
        }
        while let Ok((loc, title, dur)) = self.prober.rx.try_recv() {
            dirty = true;
            for e in self.list.iter_mut().chain(self.now.iter_mut()).filter(|e| e.loc == loc) {
                e.dur = dur.or(e.dur);
                if let Some(t) = &title {
                    e.title = t.clone();
                }
            }
        }
        if let Some(rx) = &self.radio_rx {
            if let Ok(r) = rx.try_recv() {
                dirty = true;
                self.radio_rx = None;
                match r {
                    Ok(v) => {
                        self.st_note = if v.is_empty() { "No stations found".into() } else { String::new() };
                        self.stations = v;
                    }
                    Err(e) => self.st_note = format!("Search failed: {e}"),
                }
            }
        }
        if self.eq_due.is_some_and(|t| t.elapsed() > Duration::from_millis(120)) {
            self.eq_due = None;
            self.apply_af();
        }
        if self.status.as_ref().is_some_and(|(_, t)| t.elapsed() > Duration::from_secs(3)) {
            self.status = None;
            dirty = true;
        }
        dirty
    }

    fn prop(&mut self, name: &str, v: Value) {
        let stream = self.now.as_ref().is_some_and(|e| e.stream);
        match name {
            "time-pos" => self.pos = v.as_f64().unwrap_or(0.0),
            "duration" => {
                self.dur = v.as_f64().filter(|d| *d > 0.0 && !stream);
                if let (Some(d), Some(c)) = (self.dur, self.cur) {
                    if let Some(e) = self.list.get_mut(c) {
                        e.dur.get_or_insert(d);
                    }
                }
            }
            "pause" => self.paused = v.as_bool().unwrap_or(false) && self.playing,
            "audio-bitrate" => self.kbps = v.as_f64().map(|b| (b / 1000.0).round() as u32).filter(|b| *b > 0),
            "audio-params/samplerate" => self.khz = v.as_f64().map(|r| (r / 1000.0).round() as u32),
            "audio-params/channel-count" => self.chans = v.as_u64().map(|c| c as u32),
            "metadata/by-key/icy-title" => self.icy = v.as_str().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()),
            "media-title" if stream => {
                // some streams only say what's on through the title
                let name = self.now.as_ref().map(|e| e.title.clone());
                let loc = self.now.as_ref().map(|e| e.loc.clone()).unwrap_or_default();
                // but not when the "title" is just the stream's file name
                let file = |t: &str| !t.contains(' ') && t.rsplit_once('.').is_some_and(|(_, x)| x.len() <= 5);
                if let Some(t) = v.as_str().filter(|t| !t.contains("://") && !loc.contains(*t) && !file(t) && Some(t.to_string()) != name) {
                    self.icy.get_or_insert(t.to_string());
                }
            }
            _ => {}
        }
    }

    fn ended(&mut self, reason: &str) {
        let stream = self.now.as_ref().is_some_and(|e| e.stream);
        match reason {
            "eof" if stream => {
                self.stop();
                self.say("The station stopped sending");
            }
            "eof" => self.next(),
            "error" => {
                let t = self.now.as_ref().map(|e| e.title.clone()).unwrap_or_default();
                self.say(format!("Can't play {t}"));
                self.fails += 1;
                if !stream && self.cur.is_some() && self.fails < self.list.len() {
                    self.next();
                } else {
                    self.stop();
                }
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------ commands

    /// The window's client height with the panels that are on.
    fn natural_h(&self) -> u16 {
        if self.s.compact {
            return 1;
        }
        PLAYER_H + if self.s.show_eq { EQ_H } else { 0 } + if self.s.show_pl { LIST_H } else { 0 }
    }

    fn refit(&self) -> Action {
        Action::Resize(self.size.0.max(60), self.natural_h())
    }

    fn run(&mut self, c: &str) -> Action {
        match c {
            "play" => {
                if self.playing && self.paused {
                    self.pause()
                } else if self.playing && self.cur.is_some() {
                    if let Some(m) = &mut self.mpv {
                        m.seek(0.0, true);
                    }
                } else if !self.playing {
                    self.play_selected()
                }
            }
            "play-sel" => self.play_selected(),
            "pause" => self.pause(),
            "stop" => self.stop(),
            "prev" => self.prev(),
            "next" => self.next(),
            "shuffle" => self.s.shuffle = !self.s.shuffle,
            "repeat" => self.s.repeat = !self.s.repeat,
            "add" => {
                self.overlay = Overlay::Browse(Browser::open(self.s.browse.clone()));
                if !self.s.show_pl || self.s.compact {
                    self.s.show_pl = true;
                    self.s.compact = false;
                    return self.refit();
                }
            }
            "url" => self.overlay = Overlay::Url(String::new()),
            "remove" => self.remove(),
            "clear" => {
                self.list.clear();
                (self.cur, self.sel, self.scroll) = (None, 0, 0);
                library::save_playlist(&self.list);
            }
            "sort" => self.sort(),
            "eq" => {
                self.s.show_eq = !self.s.show_eq;
                self.s.compact = false;
                return self.refit();
            }
            "pl" => {
                self.s.show_pl = !self.s.show_pl;
                self.s.compact = false;
                return self.refit();
            }
            "compact" => {
                self.s.compact = !self.s.compact;
                return self.refit();
            }
            "vis0" | "vis1" | "vis2" => {
                self.s.vis = c[3..].parse().unwrap_or(0);
                if self.s.vis == 2 {
                    self.vis.stop();
                } else if self.playing && !self.paused {
                    self.vis.start();
                }
            }
            "remaining" => self.s.remaining = !self.s.remaining,
            "help" => return Action::Launch(Launch::Msg { title: "Amp".into(), text: HELP.into() }),
            "eq-on" => {
                self.s.eq_on = !self.s.eq_on;
                self.apply_af();
            }
            "presets" => {
                let items = PRESETS.iter().enumerate().map(|(i, (n, g))| Item::new(*n, Cmd::App(PRESET_CMDS[i])).checked(*g == self.s.eq)).collect();
                return self.menu_under(items, Hit::Btn("presets"));
            }
            "genres" => {
                let items = GENRES.iter().enumerate().map(|(i, g)| Item::new(*g, Cmd::App(GENRE_CMDS[i])).checked(i == self.genre)).collect();
                return self.menu_under(items, Hit::Btn("genres"));
            }
            "favs" => {
                self.show_favs = !self.show_favs;
                (self.st_sel, self.st_scroll) = (0, 0);
            }
            "fav" => self.fav(),
            "station-add" => {
                if let Some(st) = self.shown_stations().get(self.st_sel).cloned() {
                    self.add(vec![Entry::station(&st)]);
                }
            }
            "tab-pl" | "tab-radio" => {
                self.s.radio_tab = c == "tab-radio";
                self.searching = false;
            }
            "b-add" => self.browse_enter(true),
            "b-folder" => {
                if let Overlay::Browse(b) = &self.overlay {
                    let files = library::scan(&b.dir);
                    self.add(files.iter().map(|f| Entry::file(f)).collect());
                }
            }
            "b-up" => {
                if let Overlay::Browse(b) = &self.overlay {
                    if let Some(up) = b.dir.parent() {
                        self.overlay = Overlay::Browse(Browser::open(up.to_path_buf()));
                    }
                }
            }
            "close" => self.close_overlay(),
            "url-ok" => {
                if let Overlay::Url(t) = &self.overlay {
                    let es = library::entries_from(t);
                    self.overlay = Overlay::None;
                    if es.is_empty() {
                        self.say("That isn't a link or a file");
                    } else {
                        let i = self.add(es);
                        self.play(i);
                    }
                }
            }
            _ => {
                if let Some(i) = PRESET_CMDS.iter().position(|p| *p == c) {
                    self.s.eq = PRESETS[i].1;
                    self.s.eq_on = true;
                    self.apply_af();
                } else if let Some(i) = GENRE_CMDS.iter().position(|g| *g == c) {
                    self.genre = i;
                    self.run_search();
                }
            }
        }
        Action::None
    }

    fn menu_under(&self, items: Vec<Item>, h: Hit) -> Action {
        let (x, y) = self.hits.iter().rev().find(|t| t.4 == h).map(|t| (t.0, t.1 + 1)).unwrap_or((0, 0));
        Action::Menu(items, x, y)
    }

    fn close_overlay(&mut self) {
        if let Overlay::Browse(b) = &self.overlay {
            self.s.browse = b.dir.clone();
        }
        self.overlay = Overlay::None;
    }

    /// Opens the folder or adds the file under the browser's cursor.
    fn browse_enter(&mut self, step: bool) {
        let Overlay::Browse(b) = &mut self.overlay else { return };
        let Some((p, k)) = b.items.get(b.sel) else { return };
        match k {
            Kind::Up | Kind::Dir => {
                let p = p.clone();
                self.overlay = Overlay::Browse(Browser::open(p));
            }
            Kind::File => {
                let e = Entry::file(p);
                if step {
                    b.sel = (b.sel + 1).min(b.items.len() - 1);
                }
                self.add(vec![e]);
            }
        }
    }

    // ------------------------------------------------------------ input

    /// The Winamp keys, which work everywhere.
    fn transport_key(&mut self, k: KeyEvent) -> Action {
        let big = k.modifiers.contains(KeyModifiers::SHIFT);
        match k.code {
            KeyCode::Char('z') => self.run("prev"),
            KeyCode::Char('x') => self.run("play"),
            KeyCode::Char('c') | KeyCode::Char(' ') => self.run("pause"),
            KeyCode::Char('v') => self.run("stop"),
            KeyCode::Char('b') => self.run("next"),
            KeyCode::Left => {
                self.seek_by(if big { -30.0 } else { -5.0 });
                Action::None
            }
            KeyCode::Right => {
                self.seek_by(if big { 30.0 } else { 5.0 });
                Action::None
            }
            KeyCode::Char('+') | KeyCode::Char('=') => {
                self.set_volume(self.s.volume as i32 + 5);
                Action::None
            }
            KeyCode::Char('-') => {
                self.set_volume(self.s.volume as i32 - 5);
                Action::None
            }
            _ => Action::None,
        }
    }

    fn hit_at(&self, x: i32, y: i32) -> Option<Hit> {
        self.hits.iter().rev().find(|(hx, hy, w, h, _)| x >= *hx && x < hx + w && y >= *hy && y < hy + h).map(|t| t.4)
    }

    fn scroll_list(&mut self, d: i32) {
        let rows = self.rows;
        let mv = |off: &mut usize, len: usize| *off = (*off as i32 + d).clamp(0, len.saturating_sub(rows) as i32) as usize;
        match &mut self.overlay {
            Overlay::Browse(b) => mv(&mut b.scroll, b.items.len()),
            _ if self.s.radio_tab => {
                let n = self.shown_stations().len();
                mv(&mut self.st_scroll, n)
            }
            _ => mv(&mut self.scroll, self.list.len()),
        }
    }

    fn press(&mut self, x: i32, y: i32) -> Action {
        let Some(h) = self.hit_at(x, y) else { return Action::None };
        let double = self.click.is_some_and(|(t, last)| last == h && t.elapsed() < Duration::from_millis(400));
        self.click = Some((Instant::now(), h));
        if h != Hit::Search {
            self.searching = false;
        }
        let drag = match h {
            Hit::Seek => Some(Drag::Seek),
            Hit::Vol => Some(Drag::Vol),
            Hit::Bal => Some(Drag::Bal),
            Hit::Eq(i) => Some(Drag::Eq(i)),
            Hit::Preamp => Some(Drag::Preamp),
            _ => None,
        };
        if let Some(d) = drag {
            self.drag = Some(d);
            self.drag_to(d, x, y);
            return Action::None;
        }
        match h {
            Hit::Btn(c) => return self.run(c),
            Hit::Row(i) => {
                self.sel = i;
                if double {
                    self.play(i);
                }
            }
            Hit::Station(i) => {
                self.st_sel = i;
                if double {
                    self.play_selected();
                }
            }
            Hit::File(i) => {
                if let Overlay::Browse(b) = &mut self.overlay {
                    b.sel = i;
                    if double {
                        self.browse_enter(false);
                    }
                }
            }
            Hit::Search => self.searching = true,
            Hit::Vis => {
                let v = ["vis1", "vis2", "vis0"][self.s.vis.min(2) as usize];
                return self.run(v);
            }
            Hit::Time => self.s.remaining = !self.s.remaining,
            Hit::Display if double => return self.run("compact"),
            _ => {}
        }
        Action::None
    }

    fn context(&mut self, x: i32, y: i32) -> Action {
        let items = match self.hit_at(x, y) {
            Some(Hit::Row(i)) => {
                self.sel = i;
                vec![Item::new("Play", Cmd::App("play-sel")), Item::new("Remove", Cmd::App("remove")).key("Del"), Item::sep(), Item::new("Add files...", Cmd::App("add"))]
            }
            Some(Hit::Station(i)) => {
                self.st_sel = i;
                let fav = self.shown_stations().get(i).is_some_and(|s| self.is_fav(&s.url));
                vec![
                    Item::new("Play", Cmd::App("play-sel")),
                    Item::new(if fav { "Unfavourite" } else { "Favourite" }, Cmd::App("fav")).key("F"),
                    Item::new("Add to playlist", Cmd::App("station-add")),
                ]
            }
            _ => return Action::None,
        };
        Action::Menu(items, x, y)
    }

    fn drag_to(&mut self, d: Drag, x: i32, y: i32) {
        let h = match d {
            Drag::Seek => Hit::Seek,
            Drag::Vol => Hit::Vol,
            Drag::Bal => Hit::Bal,
            Drag::Eq(i) => Hit::Eq(i),
            Drag::Preamp => Hit::Preamp,
        };
        let Some(&(rx, ry, rw, rh, _)) = self.hits.iter().rev().find(|t| t.4 == h) else { return };
        let fx = ((x - rx) as f64 / (rw.max(2) - 1) as f64).clamp(0.0, 1.0);
        let fy = 1.0 - ((y - ry) as f32 / (rh.max(2) - 1) as f32).clamp(0.0, 1.0);
        let gain = ((fy * 24.0 - 12.0) * 2.0).round() / 2.0;
        match d {
            Drag::Seek => self.seek_to(fx),
            Drag::Vol => self.set_volume((fx * 100.0).round() as i32),
            Drag::Bal => {
                let b = (fx * 2.0 - 1.0) as f32;
                self.s.balance = if b.abs() < 0.08 { 0.0 } else { b };
                self.eq_changed();
            }
            Drag::Eq(i) => {
                self.s.eq[i] = gain;
                self.eq_touch = Some(i);
                self.eq_changed();
            }
            Drag::Preamp => {
                self.s.preamp = gain;
                self.eq_touch = None;
                self.eq_changed();
            }
        }
    }

    // ------------------------------------------------------------ drawing

    fn hit(&mut self, x: i32, y: i32, w: i32, h: i32, what: Hit) {
        self.hits.push((x, y, w, h, what));
    }

    /// What the player's screen scrolls across.
    fn marquee_text(&self) -> String {
        if let Some((s, _)) = &self.status {
            return s.clone();
        }
        if let Some(e) = &self.err {
            return e.clone();
        }
        match &self.now {
            Some(e) if e.stream => match &self.icy {
                Some(t) => format!("{}  ─  {t}", e.title),
                None => e.title.clone(),
            },
            Some(e) => {
                let n = self.cur.map(|c| format!("{}. ", c + 1)).unwrap_or_default();
                let d = e.dur.or(self.dur).map(|d| format!(" ({})", fmt_time(d))).unwrap_or_default();
                format!("{n}{}{d}", e.title)
            }
            None => "Amp  ·  A adds music  ·  / finds radio  ·  F1 for keys".into(),
        }
    }

    fn marquee(&self, w: usize) -> String {
        let t = self.marquee_text();
        if width(&t) as usize <= w || self.status.is_some() {
            return cut(&t, w);
        }
        let s: Vec<char> = format!("{t}   ***   ").chars().collect();
        let off = (self.clock.elapsed().as_millis() / 220) as usize % s.len();
        s.iter().cycle().skip(off).take(w).collect()
    }

    fn clock_text(&self) -> String {
        if !self.playing {
            return "00:00".into();
        }
        let left = self.s.remaining && self.dur.is_some();
        let t = if left { self.dur.unwrap_or(0.0) - self.pos } else { self.pos };
        let t = t.max(0.0) as u64;
        format!("{}{:02}:{:02}", if left { "-" } else { "" }, (t / 60).min(99), t % 60)
    }

    fn draw_compact(&mut self, c: &mut Canvas, th: &Theme, lcd: Lcd) {
        let w = self.size.0 as i32;
        let bx = (w - 20).max(10);
        let s = st(lcd.text, lcd.bg);
        c.fill(0, 0, bx - 1, 1, s);
        let glyph = if !self.playing { "■" } else if self.paused { "‖" } else { "▶" };
        c.text(1, 0, glyph, s);
        let tx = c.text_max(3, 0, &self.clock_text(), s.add_modifier(Modifier::BOLD), bx);
        let m = self.marquee((bx - 2 - (tx + 2)).max(0) as usize);
        c.text_max(tx + 2, 0, &m, s, bx - 1);
        self.hit(0, 0, bx - 1, 1, Hit::Display);
        for (i, (l, cmd)) in [("|◀", "prev"), ("▶", "play"), ("‖", "pause"), ("■", "stop"), ("▶|", "next")].into_iter().enumerate() {
            let x = bx + i as i32 * 4;
            c.button(x, 0, 4, l, th, false, false);
            self.hit(x, 0, 4, 1, Hit::Btn(cmd));
        }
    }

    fn draw_player(&mut self, c: &mut Canvas, th: &Theme, lcd: Lcd, y0: i32) -> i32 {
        let w = self.size.0 as i32;
        // the screen: a dark panel with half-block edges
        let (sx, sw) = (1, w - 2);
        let on = st(lcd.text, lcd.bg);
        let dim = st(lcd.dim, lcd.bg);
        for x in sx..sx + sw {
            c.put(x, y0, "▄", st(lcd.bg, th.face));
            c.put(x, y0 + 6, "▀", st(lcd.bg, th.face));
        }
        c.fill(sx, y0 + 1, sw, 5, on);
        self.hit(sx, y0, sw, 7, Hit::Display);
        let cx = sx + 2;
        let right = sx + sw - 2;
        let glyph = if !self.playing { "■" } else if self.paused { "‖" } else { "▶" };
        c.text(cx, y0 + 1, glyph, if self.playing { on } else { dim });
        let clock = self.clock_text();
        let tw = big(c, cx + 2, y0 + 1, &clock, if self.playing { lcd.text } else { lcd.dim }, lcd.bg);
        self.hit(cx + 2, y0 + 1, tw, 3, Hit::Time);
        let ix = cx + 2 + tw.max(19) + 2;
        let m = self.marquee((right - ix).max(0) as usize);
        c.text_max(ix, y0 + 1, &m, on.add_modifier(Modifier::BOLD), right);
        let num = |v: Option<u32>| v.map(|v| format!("{v:>3}")).unwrap_or("   ".into());
        let mut x = c.text_max(ix, y0 + 2, &num(self.kbps), on, right);
        x = c.text_max(x + 1, y0 + 2, "kbps", dim, right);
        x = c.text_max(x + 2, y0 + 2, &num(self.khz), on, right);
        c.text_max(x + 1, y0 + 2, "kHz", dim, right);
        let stereo = self.playing && self.chans.is_some_and(|c| c >= 2);
        let mono = self.playing && self.chans == Some(1);
        let mut x = c.text_max(ix, y0 + 3, "mono", if mono { on } else { dim }, right);
        x = c.text_max(x + 2, y0 + 3, "stereo", if stereo { on } else { dim }, right);
        if let Some(e) = self.now.as_ref().filter(|e| e.stream) {
            let fav = self.is_fav(&e.loc);
            x = c.text_max(x + 3, y0 + 3, "live", if self.playing { on } else { dim }, right);
            c.text_max(x + 2, y0 + 3, "★", if fav { st(th.yellow, lcd.bg) } else { dim }, right);
        }
        self.draw_vis(c, th, lcd, cx, y0 + 4, right - cx);
        // seek bar
        let y = y0 + 7;
        let (bx, bw) = (2, w - 4);
        match (self.dur, self.playing) {
            (Some(d), true) => {
                let k = ((self.pos / d).clamp(0.0, 1.0) * (bw - 1) as f64).round() as i32;
                for i in 0..bw {
                    let (s, col) = if i < k { ("━", th.accent) } else if i == k { ("●", th.text) } else { ("─", th.dim) };
                    c.put(bx + i, y, s, st(col, th.face));
                }
            }
            _ => {
                let live = self.playing && self.now.as_ref().is_some_and(|e| e.stream);
                for i in 0..bw {
                    c.put(bx + i, y, if live { "┄" } else { "─" }, st(th.dim, th.face));
                }
            }
        }
        self.hit(bx, y, bw, 1, Hit::Seek);
        // transport, volume and balance
        let y = y + 1;
        let mut x = 2;
        for (l, cmd) in [("|◀", "prev"), ("▶", "play"), ("‖", "pause"), ("■", "stop"), ("▶|", "next"), ("▲", "add")] {
            let down = match cmd {
                "play" => self.playing && !self.paused,
                "pause" => self.playing && self.paused,
                _ => false,
            };
            c.button(x, y, 5, l, th, false, down);
            self.hit(x, y, 5, 1, Hit::Btn(cmd));
            x += 5;
        }
        x += 2;
        let room = w - x - 2;
        let show_bal = room >= 36;
        let vw = if show_bal { 12 } else { (room - 9).min(12) };
        if vw >= 4 {
            x = c.text(x, y, "Vol ", st(th.text, th.face));
            for i in 0..vw {
                let lit = i < (self.s.volume as i32 * vw + 50) / 100;
                let r = RAMP[1 + (i * 8 / vw).min(7) as usize];
                c.put(x + i, y, r, st(if lit { th.accent } else { th.shadow }, th.face));
            }
            self.hit(x, y, vw, 1, Hit::Vol);
            x = c.text(x + vw + 1, y, &format!("{:>3}%", self.s.volume), st(th.text, th.face));
        }
        if show_bal {
            x = c.text(x + 3, y, "Bal ", st(th.text, th.face));
            let bw = 9;
            let k = ((self.s.balance + 1.0) / 2.0 * (bw - 1) as f32).round() as i32;
            for i in 0..bw {
                let (s, col) = if i == k { ("●", th.text) } else if i == bw / 2 { ("┼", th.dim) } else { ("─", th.dim) };
                c.put(x + i, y, s, st(col, th.face));
            }
            self.hit(x, y, bw, 1, Hit::Bal);
        }
        // toggles
        let y = y + 1;
        c.button(2, y, 9, "Shuffle", th, false, self.s.shuffle);
        self.hit(2, y, 9, 1, Hit::Btn("shuffle"));
        c.button(11, y, 8, "Repeat", th, false, self.s.repeat);
        self.hit(11, y, 8, 1, Hit::Btn("repeat"));
        let rx = w - 12;
        c.button(rx, y, 5, "EQ", th, false, self.s.show_eq);
        self.hit(rx, y, 5, 1, Hit::Btn("eq"));
        c.button(rx + 5, y, 5, "PL", th, false, self.s.show_pl);
        self.hit(rx + 5, y, 5, 1, Hit::Btn("pl"));
        y + 1
    }

    fn draw_vis(&mut self, c: &mut Canvas, th: &Theme, lcd: Lcd, x: i32, y: i32, w: i32) {
        self.hit(x, y, w, 2, Hit::Vis);
        if self.s.vis == 2 || w < 4 {
            return;
        }
        let l = self.vis.levels.lock().unwrap();
        if self.s.vis == 0 {
            // spectrum: a bar every other column, sixteen steps tall
            let n = (w as usize).div_ceil(2);
            for b in 0..n {
                let i = b * l.bands.len() / n;
                let v = (l.bands[i] * 16.0).round() as usize;
                let pk = (l.peaks[i] * 16.0).round() as usize;
                for row in 0..2usize {
                    let base = (1 - row) * 8;
                    let fill = v.saturating_sub(base).min(8);
                    let (mut sym, mut col) = (RAMP[fill], if row == 1 { lcd.text } else if v > 13 { th.red } else { th.yellow });
                    // the peak, falling behind the bar
                    if fill == 0 && pk > base && pk <= base + 8 {
                        (sym, col) = ("▔", lcd.dim);
                    }
                    c.put(x + b as i32 * 2, y + row as i32, sym, st(col, lcd.bg));
                }
            }
        } else {
            // scope: the wave as a line, four pixels tall
            for i in 0..w as usize {
                let s = l.wave.get(i * l.wave.len() / w as usize).copied().unwrap_or(0.0);
                let px = ((1.0 - (s * 2.5).clamp(-1.0, 1.0)) * 1.999) as i32;
                c.put(x + i as i32, y + px / 2, if px % 2 == 0 { "▀" } else { "▄" }, st(lcd.text, lcd.bg));
            }
        }
    }

    fn draw_eq(&mut self, c: &mut Canvas, th: &Theme, y0: i32) -> i32 {
        let w = self.size.0 as i32;
        let dim = st(th.dim, th.face);
        for x in 1..w - 1 {
            c.put(x, y0, "─", dim);
        }
        c.text(3, y0, " Equaliser ", st(th.text, th.face));
        let y = y0 + 1;
        c.button(2, y, 6, "On", th, false, self.s.eq_on);
        self.hit(2, y, 6, 1, Hit::Btn("eq-on"));
        c.button(9, y, 11, "Presets ▾", th, false, false);
        self.hit(9, y, 11, 1, Hit::Btn("presets"));
        let note = match self.eq_touch {
            Some(i) => format!("{} Hz  {:+.1} dB", FREQS[i], self.s.eq[i]),
            None => format!("Preamp  {:+.1} dB", self.s.preamp),
        };
        c.text(22, y, &note, dim);
        let top = y + 1;
        let rows = 5;
        let on = self.s.eq_on;
        let draw = |c: &mut Canvas, x: i32, v: f32| {
            let filled = ((v + 12.0) / 24.0 * (rows * 8) as f32).round() as i32;
            for r in 0..rows {
                let amount = (filled - r * 8).clamp(0, 8) as usize;
                let col = if !on { th.dim } else if r >= 4 { th.red } else if r >= 2 { th.yellow } else { th.green };
                for dx in 0..2 {
                    c.put(x + dx, top + rows - 1 - r, RAMP[amount], st(col, th.client));
                }
            }
        };
        draw(c, 2, self.s.preamp);
        self.hit(1, top, 4, rows, Hit::Preamp);
        c.text(5, top, "+12", dim);
        c.text(5, top + 2, "  0", dim);
        c.text(5, top + 4, "-12", dim);
        c.text(2, top + rows, "pre", dim);
        let bx = 11;
        let step = ((w - bx - 2) / 10).clamp(3, 6);
        for i in 0..10 {
            let x = bx + i as i32 * step;
            draw(c, x, self.s.eq[i]);
            self.hit(x - 1, top, 4.min(step + 1), rows, Hit::Eq(i));
            let lab = FREQS[i];
            c.text(x + 1 - lab.len() as i32 / 2, top + rows, lab, dim);
        }
        top + rows + 1
    }

    fn draw_lists(&mut self, c: &mut Canvas, th: &Theme, y0: i32) {
        let (w, h) = (self.size.0 as i32, self.size.1 as i32);
        // tabs, or the add-files title while browsing
        if let Overlay::Browse(b) = &self.overlay {
            c.fill(1, y0, w - 2, 1, th.sel());
            let d = b.dir.strip_prefix(home()).map(|r| format!("~/{}", r.display())).unwrap_or_else(|_| b.dir.display().to_string());
            c.text_max(2, y0, &cut(&format!("Add files: {d}"), (w - 4).max(0) as usize), th.sel().add_modifier(Modifier::BOLD), w - 1);
        } else {
            for x in 1..w - 1 {
                c.put(x, y0, "─", st(th.dim, th.face));
            }
            let mut x = 2;
            for (l, radio, cmd) in [(" Playlist ", false, "tab-pl"), (" Radio ", true, "tab-radio")] {
                let s = if self.s.radio_tab == radio { th.sel().add_modifier(Modifier::BOLD) } else { st(th.text, th.button) };
                c.text(x, y0, l, s);
                self.hit(x, y0, l.len() as i32, 1, Hit::Btn(cmd));
                x += l.len() as i32 + 1;
            }
        }
        let foot = h - 1;
        let mut top = y0 + 1;
        let browsing = matches!(self.overlay, Overlay::Browse(_));
        if !browsing && self.s.radio_tab {
            let x = c.text(2, top, "Find ", st(th.text, th.face));
            let fw = (w / 2 - x).max(10);
            c.fill(x, top, fw, 1, st(th.text, th.client));
            let typed = self.searching || !self.query.is_empty();
            let q = if self.searching { format!("{}▏", self.query) } else if typed { self.query.clone() } else { "station name".into() };
            let qc: Vec<char> = q.chars().collect();
            let qv: String = qc[qc.len().saturating_sub((fw - 2).max(0) as usize)..].iter().collect();
            c.text_max(x + 1, top, &qv, st(if typed { th.text } else { th.dim }, th.client), x + fw - 1);
            self.hit(x, top, fw, 1, Hit::Search);
            let mut bx = x + fw + 1;
            let g = format!("{} ▾", GENRES[self.genre]);
            let gw = width(&g) + 2;
            c.button(bx, top, gw, &g, th, false, false);
            self.hit(bx, top, gw, 1, Hit::Btn("genres"));
            bx += gw + 1;
            c.button(bx, top, 15, "★ Favourites", th, false, self.show_favs);
            self.hit(bx, top, 15, 1, Hit::Btn("favs"));
            top += 1;
        }
        let rows = (foot - top).max(0) as usize;
        self.rows = rows;
        let (lx, lw) = (1, w - 2);
        c.fill(lx, top, lw, rows as i32, st(th.text, th.client));
        self.hit(lx, top, lw, rows as i32, Hit::List);
        let tw = lw - 2;
        if let Overlay::Browse(b) = &mut self.overlay {
            draw_browser(c, th, b, lx, top, tw, rows, &mut self.hits);
            let (off, n) = (b.scroll, b.items.len());
            scrollbar(c, th, lx + lw - 1, top, rows, off, n);
            self.footer(c, th, foot, &[("Add", "b-add"), ("Add folder", "b-folder"), ("Up", "b-up"), ("Done", "close")], "Space adds, A the folder");
        } else if self.s.radio_tab {
            self.draw_stations(c, th, lx, top, tw, rows);
            let n = self.shown_stations().len();
            scrollbar(c, th, lx + lw - 1, top, rows, self.st_scroll, n);
            let note = if self.st_note.is_empty() || n == 0 { "via radio-browser.info".to_string() } else { self.st_note.clone() };
            self.footer(c, th, foot, &[("▶ Play", "play-sel"), ("★ Fav", "fav"), ("+ Playlist", "station-add")], &note);
        } else {
            self.draw_playlist(c, th, lx, top, tw, rows);
            scrollbar(c, th, lx + lw - 1, top, rows, self.scroll, self.list.len());
            let total: f64 = self.list.iter().filter_map(|e| e.dur).sum();
            let n = self.list.len();
            let info = format!("{n} {}  {}", if n == 1 { "track" } else { "tracks" }, fmt_time(total));
            self.footer(c, th, foot, &[("+ Add", "add"), ("URL", "url"), ("- Rem", "remove"), ("Clear", "clear")], &info);
        }
    }

    fn footer(&mut self, c: &mut Canvas, th: &Theme, y: i32, btns: &[(&str, &'static str)], info: &str) {
        let w = self.size.0 as i32;
        let mut x = 2;
        for (l, cmd) in btns {
            let bw = width(l) + 4;
            c.button(x, y, bw, l, th, false, false);
            self.hit(x, y, bw, 1, Hit::Btn(cmd));
            x += bw + 1;
        }
        let info = cut(info, (w - x - 3).max(0) as usize);
        c.text(w - 2 - width(&info), y, &info, st(th.dim, th.face));
    }

    fn draw_playlist(&mut self, c: &mut Canvas, th: &Theme, x: i32, y: i32, w: i32, rows: usize) {
        if self.list.is_empty() {
            let msg = cut("Empty. Press A to add music, U for a stream link, or drop files here.", (w - 2).max(0) as usize);
            c.text(x + 1, y + rows as i32 / 2, &msg, st(th.dim, th.client));
            return;
        }
        if self.seen[0] != self.sel {
            self.seen[0] = self.sel;
            self.scroll = keep_in_view(self.sel, self.scroll, rows);
        }
        self.scroll = self.scroll.min(self.list.len().saturating_sub(rows));
        let numw = self.list.len().to_string().len();
        for r in 0..rows {
            let i = self.scroll + r;
            let Some(e) = self.list.get(i) else { break };
            let yy = y + r as i32;
            let playing = self.cur == Some(i) && self.playing;
            let mut s = if i == self.sel { th.sel() } else { st(if playing { th.accent } else { th.text }, th.client) };
            if playing {
                s = s.add_modifier(Modifier::BOLD);
            }
            c.fill(x, yy, w, 1, s);
            let d = if e.stream { "live".to_string() } else { e.dur.map(fmt_time).unwrap_or_default() };
            let left = format!("{}{:>numw$}. ", if playing { "▶" } else { " " }, i + 1);
            let lx = c.text(x, yy, &left, s);
            let room = (w - (lx - x) - width(&d) - 2).max(0) as usize;
            c.text(lx, yy, &cut(&e.title, room), s);
            c.text(x + w - width(&d) - 1, yy, &d, s);
            self.hits.push((x, yy, w, 1, Hit::Row(i)));
        }
    }

    fn draw_stations(&mut self, c: &mut Canvas, th: &Theme, x: i32, y: i32, w: i32, rows: usize) {
        let n = self.shown_stations().len();
        if n == 0 {
            let msg = if self.show_favs {
                "No favourites yet. Find a station, then press F or right-click it.".to_string()
            } else if self.st_note.is_empty() {
                "Type a name and press Enter".into()
            } else {
                self.st_note.clone()
            };
            c.text(x + 1, y + rows as i32 / 2, &cut(&msg, (w - 2).max(0) as usize), st(th.dim, th.client));
            return;
        }
        if self.seen[1] != self.st_sel {
            self.seen[1] = self.st_sel;
            self.st_scroll = keep_in_view(self.st_sel, self.st_scroll, rows);
        }
        self.st_scroll = self.st_scroll.min(n.saturating_sub(rows));
        let now = self.now.as_ref().filter(|e| e.stream && self.playing).map(|e| e.loc.clone());
        for r in 0..rows {
            let i = self.st_scroll + r;
            let Some(stn) = self.shown_stations().get(i).cloned() else { break };
            let yy = y + r as i32;
            let on = now.as_deref() == Some(stn.url.as_str());
            let mut s = if i == self.st_sel { th.sel() } else { st(if on { th.accent } else { th.text }, th.client) };
            if on {
                s = s.add_modifier(Modifier::BOLD);
            }
            c.fill(x, yy, w, 1, s);
            let star = if i == self.st_sel { s } else { st(th.yellow, th.client) };
            c.text(x + 1, yy, if self.is_fav(&stn.url) { "★" } else { " " }, star);
            let br = if stn.bitrate > 0 { format!("{}k", stn.bitrate) } else { String::new() };
            let codec = stn.codec.to_lowercase();
            let codec = if codec.starts_with("unk") { "" } else { &codec };
            let info = format!("{br:>5} {:<4} {:<2}", cut(codec, 4), stn.country);
            let room = (w - 3 - width(&info) - 2).max(0) as usize;
            let name = if on { format!("▶ {}", stn.name) } else { stn.name.clone() };
            c.text(x + 3, yy, &cut(&name, room), s);
            c.text(x + w - width(&info) - 1, yy, &info, s);
            self.hits.push((x, yy, w, 1, Hit::Station(i)));
        }
    }

    fn draw_url(&mut self, c: &mut Canvas, th: &Theme, t: &str) {
        let (w, h) = (self.size.0 as i32, self.size.1 as i32);
        let dw = 60.min(w - 2);
        let (x, y) = ((w - dw) / 2, ((h - 7) / 2).max(0));
        c.fill(x, y, dw, 7, st(th.text, th.face));
        c.fill(x, y, dw, 1, th.sel());
        c.text(x + 1, y, "Add URL", th.sel().add_modifier(Modifier::BOLD));
        c.bevel(x, y + 1, dw, 6, th, true);
        self.hit(x, y, dw, 7, Hit::Dialog);
        c.text_max(x + 2, y + 2, "A radio stream, web link or file path:", st(th.text, th.face), x + dw - 1);
        let fw = dw - 4;
        c.field(x + 2, y + 3, fw, &format!("{t}▏"), th);
        let bx = x + dw - 22;
        c.button(bx, y + 5, 9, "Play", th, true, false);
        self.hit(bx, y + 5, 9, 1, Hit::Btn("url-ok"));
        c.button(bx + 10, y + 5, 10, "Cancel", th, false, false);
        self.hit(bx + 10, y + 5, 10, 1, Hit::Btn("close"));
    }
}

const HELP: &str = "Z X C V B   back, play, pause, stop, next
Space       pause
Left Right  skip 5 seconds (Shift for 30)
+ -         volume
Up Down     pick, Enter plays
Tab         playlist or radio
/           find a radio station
F           favourite the station
A  U        add files, add a link
Del         take a track off
S  R        shuffle, repeat
E  P  D     equaliser, playlist, compact
T           time left or gone

Click the clock for time left, the visualiser to change it,
and double-click the screen for compact. Right-click a
track or station for more. Drop files on the window to add them.";

#[derive(Clone, Copy)]
struct Lcd {
    bg: Color,
    text: Color,
    dim: Color,
}

fn rgb(c: Color) -> (u8, u8, u8) {
    match c {
        Color::Rgb(r, g, b) => (r, g, b),
        _ => (128, 128, 128),
    }
}

fn lcd(th: &Theme) -> Lcd {
    let (bg, g) = (th.desk_rgb, rgb(th.green));
    let m = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * 0.65).round() as u8;
    Lcd { bg: Color::Rgb(bg.0, bg.1, bg.2), text: th.green, dim: Color::Rgb(m(g.0, bg.0), m(g.1, bg.1), m(g.2, bg.2)) }
}

fn width(s: &str) -> i32 {
    s.chars().map(|c| c.width().unwrap_or(0) as i32).sum()
}

/// Cuts text to `w` columns, with an ellipsis when it doesn't fit.
fn cut(s: &str, w: usize) -> String {
    if width(s) as usize <= w {
        return s.into();
    }
    let mut out = String::new();
    let mut n = 0;
    for c in s.chars() {
        let cw = c.width().unwrap_or(0);
        if n + cw + 1 > w {
            break;
        }
        out.push(c);
        n += cw;
    }
    out + "…"
}

const RAMP: [&str; 9] = [" ", "▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];

// 3x5 pixel digits for the big clock, drawn two pixel rows to a line.
const FONT: [[&str; 5]; 12] = [
    ["###", "#.#", "#.#", "#.#", "###"],
    [".#.", "##.", ".#.", ".#.", "###"],
    ["###", "..#", "###", "#..", "###"],
    ["###", "..#", "###", "..#", "###"],
    ["#.#", "#.#", "###", "..#", "..#"],
    ["###", "#..", "###", "..#", "###"],
    ["###", "#..", "###", "#.#", "###"],
    ["###", "..#", "..#", "..#", "..#"],
    ["###", "#.#", "###", "#.#", "###"],
    ["###", "#.#", "###", "..#", "###"],
    [".", "#", ".", "#", "."],
    ["...", "...", "###", "...", "..."],
];

/// Draws big digits three lines high; returns the width used.
fn big(c: &mut Canvas, x: i32, y: i32, s: &str, fg: Color, bg: Color) -> i32 {
    let mut cx = x;
    for ch in s.chars() {
        let g = match ch {
            '0'..='9' => &FONT[ch as usize - '0' as usize],
            ':' => &FONT[10],
            _ => &FONT[11],
        };
        let w = g[0].len();
        // a blank pixel row on top, so the five rows sit in three lines
        let px = |r: usize, i: usize| r >= 1 && g[r - 1].as_bytes()[i] == b'#';
        for line in 0..3 {
            for i in 0..w {
                let sym = match (px(line * 2, i), px(line * 2 + 1, i)) {
                    (true, true) => "█",
                    (true, false) => "▀",
                    (false, true) => "▄",
                    _ => " ",
                };
                c.put(cx + i as i32, y + line as i32, sym, st(fg, bg));
            }
        }
        cx += w as i32 + 1;
    }
    cx - x - 1
}

fn keep_in_view(sel: usize, scroll: usize, rows: usize) -> usize {
    if sel < scroll {
        sel
    } else if rows > 0 && sel >= scroll + rows {
        sel + 1 - rows
    } else {
        scroll
    }
}

fn scrollbar(c: &mut Canvas, th: &Theme, x: i32, y: i32, rows: usize, off: usize, n: usize) {
    for r in 0..rows as i32 {
        c.put(x, y + r, "░", st(th.shadow, th.client));
    }
    if n > rows && rows > 0 {
        let len = (rows * rows / n).max(1);
        let pos = off * (rows - len) / (n - rows).max(1);
        for r in 0..len {
            c.put(x, y + (pos + r) as i32, "█", st(th.button, th.client));
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_browser(c: &mut Canvas, th: &Theme, b: &mut Browser, x: i32, y: i32, w: i32, rows: usize, hits: &mut Vec<(i32, i32, i32, i32, Hit)>) {
    b.scroll = keep_in_view(b.sel, b.scroll, rows).min(b.items.len().saturating_sub(rows));
    if b.items.is_empty() {
        c.text(x + 1, y, "Nothing to play here", st(th.dim, th.client));
    }
    for r in 0..rows {
        let i = b.scroll + r;
        let Some((path, kind)) = b.items.get(i) else { break };
        let yy = y + r as i32;
        let s = if i == b.sel { th.sel() } else { st(th.text, th.client) };
        c.fill(x, yy, w, 1, s);
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let (icon, label, ic) = match kind {
            Kind::Up => ("↰", "..".to_string(), th.dim),
            Kind::Dir => ("■", format!("{name}/"), th.yellow),
            Kind::File => ("♪", name, th.accent),
        };
        c.text(x + 1, yy, icon, if i == b.sel { s } else { st(ic, th.client) });
        c.text_max(x + 3, yy, &cut(&label, (w - 4).max(0) as usize), s, x + w);
        hits.push((x, yy, w, 1, Hit::File(i)));
    }
}

impl App for Amp {
    fn title(&self) -> String {
        match &self.now {
            Some(e) if self.playing => format!("{} - Amp", e.title),
            _ => "Amp".into(),
        }
    }

    fn tab_title(&self) -> String {
        "Amp".into()
    }

    fn icon(&self) -> Icon {
        Icon::Amp
    }

    fn size_hint(&self) -> (u16, u16) {
        (78, self.natural_h())
    }

    fn resize(&mut self, w: u16, h: u16) {
        self.size = (w, h);
    }

    fn render(&mut self, c: &mut Canvas, th: &Theme, _focused: bool) {
        self.hits.clear();
        let (w, h) = (self.size.0 as i32, self.size.1 as i32);
        c.fill(0, 0, w, h, st(th.text, th.face));
        if w < 36 {
            c.text(1, 0, "Make me wider", st(th.text, th.face));
            return;
        }
        let lcd = lcd(th);
        if self.s.compact || h < 4 {
            return self.draw_compact(c, th, lcd);
        }
        let mut y = 0;
        if h >= PLAYER_H as i32 {
            y = self.draw_player(c, th, lcd, y);
        }
        if self.s.show_eq && h >= y + EQ_H as i32 + if self.s.show_pl { 4 } else { 0 } {
            y = self.draw_eq(c, th, y);
        }
        if self.s.show_pl && h > y + 3 {
            self.draw_lists(c, th, y);
        }
        if let Overlay::Url(t) = &self.overlay {
            let t = t.clone();
            self.draw_url(c, th, &t);
        }
    }

    fn key(&mut self, k: KeyEvent) -> Action {
        match &mut self.overlay {
            Overlay::Url(t) => {
                match k.code {
                    KeyCode::Esc => self.overlay = Overlay::None,
                    KeyCode::Enter => return self.run("url-ok"),
                    KeyCode::Backspace => {
                        t.pop();
                    }
                    KeyCode::Char(ch) => t.push(ch),
                    _ => {}
                }
                return Action::None;
            }
            Overlay::Browse(b) => {
                let rows = self.rows.max(1);
                let last = b.items.len().saturating_sub(1);
                match k.code {
                    KeyCode::Esc => self.close_overlay(),
                    KeyCode::Up => b.sel = b.sel.saturating_sub(1),
                    KeyCode::Down => b.sel = (b.sel + 1).min(last),
                    KeyCode::PageUp => b.sel = b.sel.saturating_sub(rows),
                    KeyCode::PageDown => b.sel = (b.sel + rows).min(last),
                    KeyCode::Home => b.sel = 0,
                    KeyCode::End => b.sel = last,
                    KeyCode::Enter | KeyCode::Right => self.browse_enter(false),
                    KeyCode::Char(' ') => self.browse_enter(true),
                    KeyCode::Backspace | KeyCode::Left => return self.run("b-up"),
                    KeyCode::Char('a') => return self.run("b-folder"),
                    _ => return self.transport_key(k),
                }
                return Action::None;
            }
            Overlay::None => {}
        }
        if self.searching {
            match k.code {
                KeyCode::Esc | KeyCode::Tab | KeyCode::Down => self.searching = false,
                KeyCode::Enter => self.run_search(),
                KeyCode::Backspace => {
                    self.query.pop();
                }
                KeyCode::Char(ch) => self.query.push(ch),
                _ => {}
            }
            return Action::None;
        }
        let radio = self.radio_shown();
        let rows = self.rows.max(1);
        let (len, sel) = if radio { (self.shown_stations().len(), self.st_sel) } else { (self.list.len(), self.sel) };
        let moved = match k.code {
            KeyCode::Up => Some(sel.saturating_sub(1)),
            KeyCode::Down => Some(sel + 1),
            KeyCode::PageUp => Some(sel.saturating_sub(rows)),
            KeyCode::PageDown => Some(sel + rows),
            KeyCode::Home => Some(0),
            KeyCode::End => Some(len.saturating_sub(1)),
            _ => None,
        };
        if let Some(m) = moved {
            let m = m.min(len.saturating_sub(1));
            if radio { self.st_sel = m } else { self.sel = m }
            return Action::None;
        }
        match k.code {
            KeyCode::Enter => self.run("play-sel"),
            KeyCode::Delete if !radio => self.run("remove"),
            KeyCode::Tab | KeyCode::BackTab => self.run(if self.s.radio_tab { "tab-pl" } else { "tab-radio" }),
            KeyCode::Char('/') => {
                self.s.radio_tab = true;
                self.searching = true;
                if !self.s.show_pl || self.s.compact {
                    self.s.show_pl = true;
                    self.s.compact = false;
                    return self.refit();
                }
                Action::None
            }
            KeyCode::Char('a') => self.run("add"),
            KeyCode::Char('u') => self.run("url"),
            KeyCode::Char('s') => self.run("shuffle"),
            KeyCode::Char('r') => self.run("repeat"),
            KeyCode::Char('e') => self.run("eq"),
            KeyCode::Char('p') => self.run("pl"),
            KeyCode::Char('d') => self.run("compact"),
            KeyCode::Char('t') => self.run("remaining"),
            KeyCode::Char('f') => self.run("fav"),
            KeyCode::Char('?') | KeyCode::F(1) => self.run("help"),
            _ => self.transport_key(k),
        }
    }

    fn mouse(&mut self, kind: MouseEventKind, x: i32, y: i32, _mods: KeyModifiers) -> Action {
        match kind {
            MouseEventKind::Down(MouseButton::Left) => return self.press(x, y),
            MouseEventKind::Down(MouseButton::Right) => return self.context(x, y),
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(d) = self.drag {
                    self.drag_to(d, x, y);
                }
            }
            MouseEventKind::Up(_) => self.drag = None,
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let d: i32 = if kind == MouseEventKind::ScrollUp { -1 } else { 1 };
                match self.hit_at(x, y) {
                    Some(Hit::Vol) => self.set_volume(self.s.volume as i32 - d * 2),
                    Some(Hit::Bal) => {
                        self.s.balance = (self.s.balance - d as f32 * 0.1).clamp(-1.0, 1.0);
                        self.eq_changed();
                    }
                    Some(Hit::Eq(i)) => {
                        self.s.eq[i] = (self.s.eq[i] - d as f32).clamp(-12.0, 12.0);
                        self.eq_touch = Some(i);
                        self.eq_changed();
                    }
                    Some(Hit::Preamp) => {
                        self.s.preamp = (self.s.preamp - d as f32).clamp(-12.0, 12.0);
                        self.eq_touch = None;
                        self.eq_changed();
                    }
                    Some(Hit::Seek) => self.seek_by(-d as f64 * 5.0),
                    _ => self.scroll_list(d * 3),
                }
            }
            _ => {}
        }
        Action::None
    }

    fn paste(&mut self, s: &str) {
        if let Overlay::Url(t) = &mut self.overlay {
            t.push_str(s.trim());
            return;
        }
        if self.searching {
            self.query.push_str(s.trim());
            return;
        }
        let es = library::entries_from(s);
        if es.is_empty() {
            self.say("Nothing to play in that");
        } else {
            self.add(es);
        }
    }

    fn poll(&mut self) -> (bool, Action) {
        let mut dirty = self.update();
        // the clock, marquee and visualiser move about 25 times a second
        let moving = self.playing && !self.paused || self.marquee_text().chars().count() > 30;
        if moving && self.frame.elapsed() >= Duration::from_millis(40) {
            dirty = true;
        }
        if dirty {
            self.frame = Instant::now();
        }
        (dirty, Action::None)
    }

    fn menubar(&self) -> Vec<(&'static str, Vec<Item>)> {
        let s = &self.s;
        vec![
            ("File", vec![
                Item::new("Add files...", Cmd::App("add")).key("A"),
                Item::new("Add URL...", Cmd::App("url")).key("U"),
                Item::sep(),
                Item::new("Remove track", Cmd::App("remove")).key("Del"),
                Item::new("Sort by title", Cmd::App("sort")),
                Item::new("Clear playlist", Cmd::App("clear")),
                Item::sep(),
                Item::new("Close", Cmd::Sys(Sys::Close)),
            ]),
            ("Play", vec![
                Item::new("Play", Cmd::App("play")).key("X"),
                Item::new("Pause", Cmd::App("pause")).key("C"),
                Item::new("Stop", Cmd::App("stop")).key("V"),
                Item::new("Previous", Cmd::App("prev")).key("Z"),
                Item::new("Next", Cmd::App("next")).key("B"),
                Item::sep(),
                Item::new("Shuffle", Cmd::App("shuffle")).key("S").checked(s.shuffle),
                Item::new("Repeat", Cmd::App("repeat")).key("R").checked(s.repeat),
                Item::sep(),
                Item::new("Favourite station", Cmd::App("fav")).key("F"),
            ]),
            ("View", vec![
                Item::new("Equaliser", Cmd::App("eq")).key("E").checked(s.show_eq),
                Item::new("Playlist", Cmd::App("pl")).key("P").checked(s.show_pl),
                Item::new("Compact", Cmd::App("compact")).key("D").checked(s.compact),
                Item::sep(),
                Item::new("Spectrum", Cmd::App("vis0")).checked(s.vis == 0),
                Item::new("Scope", Cmd::App("vis1")).checked(s.vis == 1),
                Item::new("No visualiser", Cmd::App("vis2")).checked(s.vis == 2),
                Item::sep(),
                Item::new("Time remaining", Cmd::App("remaining")).key("T").checked(s.remaining),
            ]),
            ("Help", vec![Item::new("Keys", Cmd::App("help")).key("F1").icon(Icon::Help)]),
        ]
    }

    fn command(&mut self, cmd: &str) -> Action {
        self.run(cmd)
    }

    fn open_more(&mut self, files: &[PathBuf]) -> Action {
        self.open_files(files);
        Action::None
    }
}

impl Drop for Amp {
    fn drop(&mut self) {
        self.save();
    }
}

/// Whether a path is something Amp plays.
pub fn plays(p: &Path) -> bool {
    library::is_audio(p)
}
