// The window manager: desktop icons, overlapping windows, taskbar and menus.
mod tiling;
mod tray;

use crate::{
    apps::{
        background::Background,
        dialogs::{about_text, parse_run, AddProgram, MsgBox, Run, ShutDown, TaskList},
        explorer::Explorer,
        launcher::{rank, Launcher},
        amp::Amp,
        mines::Mines,
        browser::Browser,
        solitaire::Solitaire,
        notepad::Notepad,
        paint::Paint,
        tabs::Tabs,
        term::TermApp,
        Action, App, Launch,
    },
    draw::{st, Canvas},
    icons::{palette, Icon},
    menu::{Cmd, Item, Level, MenuState, Owner, Sys},
    theme::{self, home, Theme},
};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    style::{Modifier, Style},
    Frame,
};
use std::{
    path::PathBuf,
    time::{Instant, SystemTime},
};

pub struct Win {
    pub id: u64,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    restore: Option<(i32, i32, i32, i32)>,
    pub max: bool,
    /// Snapped to half or a quarter of the screen; `restore` holds its old size.
    snapped: bool,
    pub min: bool,
    pub app: Box<dyn App>,
    has_bar: bool,
    buf: Buffer,
    title: String,
    /// tiling desktop: its workspace, and floating, tiled or fullscreen
    ws: usize,
    float: bool,
    tiled: bool,
    full: bool,
    /// on another workspace, or behind a fullscreen window
    hidden: bool,
    /// what the dock knows it as, and how to start another
    key: String,
    relaunch: Option<Launch>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Part {
    Title,
    Icon,
    Min,
    Max,
    Close,
    Frame,
    Bar(i32),
    Client(i32, i32),
    Edge(bool, bool, bool),
}

impl Win {
    fn client(&self) -> (i32, i32, i32, i32) {
        let b = self.has_bar as i32;
        (self.x + 1, self.y + 1 + b, self.w - 2, self.h - 2 - b)
    }

    fn contains(&self, x: i32, y: i32) -> bool {
        !self.min && !self.hidden && x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }

    fn sync(&mut self) {
        let (_, _, cw, ch) = self.client();
        let (cw, ch) = (cw.max(1) as u16, ch.max(1) as u16);
        if self.buf.area.width != cw || self.buf.area.height != ch {
            self.buf = Buffer::empty(Rect::new(0, 0, cw, ch));
        }
        self.app.resize(cw, ch);
    }

    fn buttons_x(&self) -> i32 {
        self.x + self.w - 2
    }

    fn hit(&self, x: i32, y: i32) -> Option<Part> {
        if !self.contains(x, y) {
            return None;
        }
        let r = self.buttons_x();
        let dialog = self.app.dialog() || self.tiled;
        if y == self.y {
            if x == self.x + 1 || x == self.x + 2 {
                return Some(Part::Icon);
            }
            if (r - 2..=r).contains(&x) {
                return Some(Part::Close);
            }
            if !dialog && (r - 6..=r - 4).contains(&x) && self.app.resizable() {
                return Some(Part::Max);
            }
            let min_x = if self.app.resizable() { r - 9 } else { r - 6 };
            if !dialog && (min_x..=min_x + 2).contains(&x) {
                return Some(Part::Min);
            }
            return Some(Part::Title);
        }
        let (left, right, bottom) = (x == self.x, x == self.x + self.w - 1, y == self.y + self.h - 1);
        if left || right || bottom {
            return Some(if self.app.resizable() && !self.max && !self.tiled { Part::Edge(left, right, bottom) } else { Part::Frame });
        }
        if self.has_bar && y == self.y + 1 {
            return Some(Part::Bar(x - self.x - 1));
        }
        let (cx, cy, _, _) = self.client();
        Some(Part::Client(x - cx, y - cy))
    }
}

/// Positions of menu bar labels, relative to the window's inner left edge.
fn bar_layout(menus: &[(&'static str, Vec<Item>)]) -> Vec<(i32, i32)> {
    let mut x = 0;
    menus
        .iter()
        .map(|(l, _)| {
            let w = l.chars().count() as i32 + 2;
            let r = (x, w);
            x += w;
            r
        })
        .collect()
}

/// Where a window lands when it is dropped against a screen edge.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Snap {
    Max,
    Left,
    Right,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

enum Drag {
    None,
    /// `pull`: the window is maximised or snapped and goes back to its own
    /// size as soon as it is dragged.
    Move { id: u64, dx: i32, dy: i32, pull: bool },
    Resize { id: u64, l: bool, r: bool, b: bool, mx: i32, my: i32, orig: (i32, i32, i32, i32) },
    Button { id: u64, part: Part },
    Client { id: u64 },
    Start,
    /// tiling: a window held by its title, to drop on another and swap
    Swap { id: u64 },
    /// tiling: the line between tiles, to make one bigger
    Split(tiling::Split),
    /// the selected desktop icons, picked up at mx, my. `one`: a click that
    /// doesn't move picks just the icon clicked out of the selection.
    Icon { mx: i32, my: i32, moved: bool, one: bool },
    /// a box dragged out over the desktop to select the icons inside it
    Band { x0: i32, y0: i32 },
    /// the volume slider
    Volume,
}

enum TaskHit {
    Start,
    Win(u64),
    Volume,
    Clock,
}

struct DeskIcon {
    /// what it's saved as: its name, or "@path" for a shortcut to a file
    key: String,
    icon: Icon,
    label: String,
    launch: Launch,
    x: i32,
    y: i32,
    sel: bool,
    /// where it was when picked up, and whether it was selected when a box was started
    orig: (i32, i32),
    was_sel: bool,
}

pub struct Desktop {
    pub th: Theme,
    theme_mtime: Option<SystemTime>,
    theme_check: Instant,
    wins: Vec<Win>,
    focus: Option<u64>,
    next_id: u64,
    w: i32,
    h: i32,
    menu: Option<(MenuState, Instant)>,
    drag: Drag,
    /// The snap the window being dragged would take if dropped now.
    snap: Option<Snap>,
    hover: Option<(i32, i32)>,
    icons: Vec<DeskIcon>,
    sel_icon: Option<usize>,
    /// where the desktop was right-clicked, for an icon added from there
    desk_click: (i32, i32),
    last_click: Option<(Instant, i32, i32)>,
    kbmode: Option<(u64, bool)>,
    /// the window the terminal is sending key releases for
    key_releases: Option<u64>,
    pub quit: bool,
    /// the session server should let go of this terminal
    pub detach: bool,
    /// show where the mouse is with a block
    pointer: bool,
    docs: Vec<PathBuf>,
    clock: String,
    /// the programs you added: name, command, icon
    mine: Vec<(String, String, Icon)>,
    /// the Omarchy desktop: tiled windows, a top bar and a dock
    tiling: bool,
    cur_ws: usize,
    /// windows in tiling order; each workspace takes its own in turn
    order: Vec<u64>,
    /// how each workspace's splits are shared out
    ratios: Vec<Vec<f32>>,
    dock_open: bool,
    battery: Option<String>,
    /// the speaker volume and whether it's muted; None without wpctl
    vol: Option<(u8, bool)>,
    /// where the volume popup is, when it's open
    vol_open: Option<(i32, i32)>,
    vol_check: Instant,
    back: crate::wallpaper::Back,
    painter: crate::wallpaper::Painter,
}

fn has(cmd: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(cmd).is_file()))
}

fn clock() -> String {
    chrono::Local::now().format("%H:%M").to_string()
}

const HELP: &str = "Click, drag and double-click like a normal desktop.\n\n\
Ctrl+Esc or Alt+S   Apps menu\n\
Alt+Tab or Alt+`    Switch window\n\
Alt+F4              Close window\n\
Alt+Space           Window menu (Move/Size with arrows)\n\
Ctrl+Alt+Del        Task list\n\
Shift+PgUp/PgDn     Scroll back in a terminal\n\
Double-click title  Maximise\n\n\
Omarchy desktop (Apps > Settings): tiled windows,\n\
a bar on top and a dock at the bottom edge.\n\
Alt+Enter           Terminal\n\
Alt+Space           Launcher\n\
Alt+W               Close window\n\
Alt+1..9            Workspace (Shift moves the window)\n\
Alt+Arrows          Focus (Shift swaps)\n\
Alt+F / Alt+T       Fullscreen / float\n\
Super works too where your terminal passes it on.\n\n\
Colours follow your Omarchy theme as it changes.";

const FIND: &str = r#"printf 'Named: '; read -r q; [ -n "$q" ] && find ~ -iname "*$q*" -not -path '*/.*' 2>/dev/null | sed "s|^$HOME|~|" | head -500"#;

impl Desktop {
    pub fn new(w: u16, h: u16) -> Desktop {
        let mut d = Desktop {
            th: Theme::load(),
            theme_mtime: theme::mtime(),
            theme_check: Instant::now(),
            wins: vec![],
            focus: None,
            next_id: 1,
            w: w as i32,
            h: h as i32,
            menu: None,
            drag: Drag::None,
            snap: None,
            hover: None,
            icons: vec![],
            sel_icon: None,
            desk_click: (0, 0),
            last_click: None,
            kbmode: None,
            key_releases: None,
            quit: false,
            detach: false,
            pointer: theme::saved_pointer(),
            docs: vec![],
            clock: clock(),
            mine: vec![],
            tiling: false,
            cur_ws: 0,
            order: vec![],
            ratios: vec![vec![]; tiling::WORKSPACES],
            dock_open: false,
            battery: None,
            vol: crate::volume::get(),
            vol_open: None,
            vol_check: Instant::now(),
            back: crate::wallpaper::Back::load(),
            painter: Default::default(),
        };
        d.tick_tiling();
        d.refresh_programs();
        d
    }

    /// Reads the programs you added and lays out the desktop icons again.
    fn refresh_programs(&mut self) {
        self.mine = crate::programs::load()
            .into_iter()
            .map(|(name, cmd)| {
                let icon = crate::programs::icon(&name).map(|r| crate::icons::custom(&r)).unwrap_or(Icon::Run);
                (name, cmd, icon)
            })
            .collect();
        let all = self.desk_catalogue();
        let make = |(icon, label, launch): (Icon, String, Launch), x, y| DeskIcon { key: label.clone(), icon, label, launch, x, y, sel: false, orig: (x, y), was_sel: false };
        self.icons = match crate::desktop::load() {
            Some(saved) => saved
                .into_iter()
                .filter_map(|(name, x, y)| match name.strip_prefix('@') {
                    Some(p) => Some(shortcut(PathBuf::from(p), x, y)),
                    None => all.iter().find(|(_, l, _)| *l == name).cloned().map(|e| make(e, x, y)),
                })
                .collect(),
            None => {
                let v = all.into_iter().filter(|(_, l, _)| l != "Files");
                v.enumerate().map(|(i, e)| {
                    let (x, y) = self.icon_slot(i);
                    make(e, x, y)
                }).collect()
            }
        };
        self.sel_icon = None;
    }

    /// Everything that can go on the desktop: the built-in icons, then the
    /// programs you added, Trash last.
    fn desk_catalogue(&self) -> Vec<(Icon, String, Launch)> {
        let d = |icon: Icon, label: &str, launch: Launch| (icon, label.to_string(), launch);
        let mut v = vec![
            d(Icon::Computer, "Computer", Launch::Explorer(PathBuf::from("/"))),
            d(Icon::Folder, "Home", Launch::Explorer(home())),
            d(Icon::Terminal, "Terminal", Launch::shell("Terminal", Icon::Terminal, None)),
            d(Icon::Notepad, "Notes", Launch::Notepad(None)),
            d(Icon::Mines, "Mines", Launch::Mines),
            d(Icon::Amp, "Amp", Launch::Amp(vec![])),
            d(Icon::Solitaire, "Solitaire", Launch::Solitaire),
            d(Icon::Doom, "Doom", Launch::Doom),
            d(Icon::Browser, "Browser", Launch::Browser(None)),
            d(Icon::Paint, "Paint", Launch::Paint(None)),
        ];
        if has("btop") {
            v.push(d(Icon::Monitor, "Monitor", Launch::shell("Monitor", Icon::Monitor, Some("btop"))));
        }
        v.push(d(Icon::Folder, "Files", Launch::Explorer(home())));
        for (name, cmd, icon) in &self.mine {
            if !v.iter().any(|(_, l, _)| l == name) {
                v.push(d(*icon, name, Launch::shell(name, *icon, Some(cmd))));
            }
        }
        v.push(d(Icon::Recycle, "Trash", Launch::Explorer(home().join(".local/share/Trash/files"))));
        v
    }

    fn save_icons(&self) {
        crate::desktop::save(&self.icons.iter().map(|i| (i.key.clone(), i.x, i.y)).collect::<Vec<_>>());
    }

    /// Puts an icon on the desktop at x, y, or the first free place.
    fn add_icon(&mut self, name: &str, at: Option<(i32, i32)>) {
        if self.icons.iter().any(|i| i.key == name) {
            return;
        }
        let Some((icon, label, launch)) = self.desk_catalogue().into_iter().find(|(_, l, _)| l == name) else { return };
        let (x, y) = match at {
            Some((x, y)) => self.snap(x, y, &|_| false),
            None => (0..).map(|i| self.icon_slot(i)).find(|&(x, y)| self.icon_at(x, y).is_none()).unwrap(),
        };
        self.icons.push(DeskIcon { key: label.clone(), icon, label, launch, x, y, sel: false, orig: (x, y), was_sel: false });
        self.save_icons();
    }

    /// Puts a shortcut to a file or folder on the desktop, in the first free place.
    fn add_shortcut(&mut self, p: PathBuf) {
        let key = format!("@{}", p.display());
        if self.icons.iter().any(|i| i.key == key) {
            return;
        }
        let (x, y) = (0..).map(|i| self.icon_slot(i)).find(|&(x, y)| self.icon_at(x, y).is_none()).unwrap();
        self.icons.push(shortcut(p, x, y));
        self.save_icons();
    }

    /// Selects just icon i, or nothing.
    fn select_only(&mut self, i: Option<usize>) {
        for (j, ic) in self.icons.iter_mut().enumerate() {
            ic.sel = Some(j) == i;
        }
        self.sel_icon = i;
    }

    /// Takes the selected icons off the desktop.
    fn remove_selected(&mut self) {
        self.icons.retain(|i| !i.sel);
        self.sel_icon = None;
        self.save_icons();
    }

    /// Lines the icons up in columns, by name or in the order they're in now.
    fn arrange_icons(&mut self, by_name: bool) {
        if by_name {
            self.icons.sort_by_key(|i| (i.label == "Trash", i.label.to_lowercase()));
        } else {
            let (cw, _) = self.icon_box();
            self.icons.sort_by_key(|i| (i.x / (cw + 1), i.y));
        }
        for i in 0..self.icons.len() {
            (self.icons[i].x, self.icons[i].y) = self.icon_slot(i);
        }
        self.select_only(None);
        self.save_icons();
    }

    fn work_h(&self) -> i32 {
        if self.tiling {
            self.h
        } else {
            self.h - 1
        }
    }

    fn idx(&self, id: u64) -> Option<usize> {
        self.wins.iter().position(|w| w.id == id)
    }

    fn win(&mut self, id: u64) -> Option<&mut Win> {
        self.wins.iter_mut().find(|w| w.id == id)
    }

    fn top_visible(&self) -> Option<u64> {
        self.wins.iter().rev().find(|w| !w.min && !w.hidden).map(|w| w.id)
    }

    // ------------------------------------------------------------ window ops

    fn focus_win(&mut self, id: u64) {
        if let Some(i) = self.idx(id) {
            let mut w = self.wins.remove(i);
            w.min = false;
            let (ws, tiled) = (w.ws, !w.float);
            self.wins.push(w);
            self.focus = Some(id);
            self.select_only(None);
            if self.tiling {
                if ws != self.cur_ws {
                    self.cur_ws = ws;
                }
                // focusing a tile behind a fullscreen window brings it out
                if tiled && self.full_win().is_some_and(|f| f != id) {
                    for w in &mut self.wins {
                        w.full = false;
                    }
                }
                self.relayout();
            }
        }
    }

    fn minimize(&mut self, id: u64) {
        if let Some(w) = self.win(id) {
            w.min = true;
        }
        if self.focus == Some(id) {
            self.focus = self.top_visible();
        }
    }

    fn toggle_max(&mut self, id: u64) {
        if self.tiling {
            return self.toggle_full(id);
        }
        let (sw, sh) = (self.w, self.work_h());
        let Some(w) = self.win(id) else { return };
        if !w.app.resizable() {
            return;
        }
        if w.max {
            if let Some((x, y, ww, hh)) = w.restore.take() {
                (w.x, w.y, w.w, w.h) = (x, y, ww, hh);
            }
            w.max = false;
        } else {
            if !w.snapped {
                w.restore = Some((w.x, w.y, w.w, w.h));
            }
            (w.x, w.y, w.w, w.h) = (0, 0, sw, sh);
            w.max = true;
            w.snapped = false;
        }
        w.sync();
    }

    /// The rectangle a snap fills: halves and quarters of the work area.
    fn snap_rect(&self, s: Snap) -> (i32, i32, i32, i32) {
        let (sw, sh) = (self.w, self.work_h());
        let (hw, hh) = (sw / 2, sh / 2);
        match s {
            Snap::Max => (0, 0, sw, sh),
            Snap::Left => (0, 0, hw, sh),
            Snap::Right => (hw, 0, sw - hw, sh),
            Snap::TopLeft => (0, 0, hw, hh),
            Snap::TopRight => (hw, 0, sw - hw, hh),
            Snap::BottomLeft => (0, hh, hw, sh - hh),
            Snap::BottomRight => (hw, hh, sw - hw, sh - hh),
        }
    }

    /// Which snap the pointer is asking for: the far left or right edge for a
    /// half, its top or bottom few rows for a quarter, the top edge to maximise.
    fn snap_at(&self, x: i32, y: i32) -> Option<Snap> {
        let (sw, sh) = (self.w, self.work_h());
        let corner = (sh / 6).max(2);
        if x <= 0 {
            Some(if y < corner { Snap::TopLeft } else if y >= sh - corner { Snap::BottomLeft } else { Snap::Left })
        } else if x >= sw - 1 {
            Some(if y < corner { Snap::TopRight } else if y >= sh - corner { Snap::BottomRight } else { Snap::Right })
        } else if y <= 0 {
            Some(Snap::Max)
        } else {
            None
        }
    }

    fn snap_win(&mut self, id: u64, s: Snap) {
        if s == Snap::Max {
            if !self.win(id).is_some_and(|w| w.max) {
                self.toggle_max(id);
            }
            return;
        }
        let (x, y, ww, hh) = self.snap_rect(s);
        let Some(w) = self.win(id) else { return };
        if !w.snapped && !w.max {
            w.restore = Some((w.x, w.y, w.w, w.h));
        }
        (w.x, w.y, w.w, w.h) = (x, y, ww, hh);
        w.max = false;
        w.snapped = true;
        w.sync();
    }

    fn close(&mut self, id: u64) {
        if let Some(i) = self.idx(id) {
            self.wins.remove(i);
        }
        self.order.retain(|o| *o != id);
        if let Some((m, _)) = &self.menu {
            if matches!(m.owner, Owner::Sys(o) | Owner::Bar(o, _) if o == id) {
                self.menu = None;
            }
        }
        if self.kbmode.is_some_and(|(k, _)| k == id) {
            self.kbmode = None;
        }
        if self.focus == Some(id) {
            self.focus = self.top_visible();
        }
        self.relayout();
    }

    fn sys(&mut self, id: u64, s: Sys) {
        let Some(w) = self.win(id) else { return };
        match s {
            Sys::Restore => {
                if w.max {
                    self.toggle_max(id);
                }
                self.focus_win(id);
            }
            Sys::Move => self.kbmode = Some((id, false)),
            Sys::Size => {
                if w.app.resizable() && !w.max {
                    self.kbmode = Some((id, true));
                }
            }
            Sys::Minimize => self.minimize(id),
            Sys::Maximize => {
                if !w.max {
                    self.toggle_max(id);
                }
            }
            Sys::Close => self.close(id),
            Sys::Float => self.toggle_float(id),
            Sys::Full => self.toggle_full(id),
            Sys::ToWs(n) => self.move_to_ws(id, n),
        }
    }

    pub fn launch(&mut self, l: Launch) {
        let single = match &l {
            Launch::Run => Some("Run"),
            Launch::AddProgram => Some("Add Program"),
            Launch::Background => Some("Background"),
            Launch::ShutDown => Some("Quit"),
            Launch::TaskList => Some("Tasks"),
            Launch::Launcher => Some("Launch"),
            _ => None,
        };
        // there's one music player: more files go to the one that's open
        if let Launch::Amp(files) = &l {
            if let Some(w) = self.wins.iter_mut().find(|w| w.key == "Amp") {
                w.app.open_more(files);
                let id = w.id;
                self.focus_win(id);
                return;
            }
        }
        if let Some(t) = single {
            if let Some(id) = self.wins.iter().find(|w| w.title == t).map(|w| w.id) {
                self.focus_win(id);
                return;
            }
        }
        let key = tiling::launch_key(&l);
        let relaunch = key.as_ref().map(|_| l.clone());
        let app: Box<dyn App> = match l {
            Launch::Shell { cmd, cwd, title, icon, keep_open } => match TermApp::new(cmd.as_deref(), cwd, &title, icon, keep_open, &self.th) {
                // a plain shell can open more in tabs; a program like btop stays as it is
                Ok(t) if cmd.is_none() => Box::new(Tabs::new(Box::new(t), &self.th)),
                Ok(t) => Box::new(t),
                Err(e) => Box::new(MsgBox::new("Terminal".into(), format!("Cannot start {}:\n{e}", cmd.unwrap_or("the shell".into())))),
            },
            Launch::Notepad(p) => {
                if let Some(p) = &p {
                    self.docs.retain(|d| d != p);
                    self.docs.push(p.clone());
                    if self.docs.len() > 12 {
                        self.docs.remove(0);
                    }
                }
                Box::new(Tabs::new(Box::new(Notepad::new(p)), &self.th))
            }
            Launch::Mines => Box::new(Mines::new()),
            Launch::Amp(files) => Box::new(Amp::new(files)),
            Launch::Solitaire => Box::new(Solitaire::new()),
            Launch::Doom => Box::new(crate::apps::doom::Doom::new()),
            Launch::Browser(u) => Box::new(Tabs::new(Box::new(Browser::new(u)), &self.th)),
            Launch::Paint(p) => Box::new(Paint::new(p)),
            Launch::External(p) => {
                let _ = std::process::Command::new("xdg-open")
                    .arg(&p)
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn();
                return;
            }
            Launch::Explorer(p) => Box::new(Tabs::new(Box::new(Explorer::new(p)), &self.th)),
            Launch::Run => Box::new(Run::new()),
            Launch::AddProgram => Box::new(AddProgram::new()),
            Launch::Background => Box::new(Background::new(&self.th)),
            Launch::About => Box::new(MsgBox::new("About".into(), about_text(&self.th))),
            Launch::ShutDown => Box::new(ShutDown::new()),
            Launch::TaskList => Box::new(TaskList::new(self.wins.iter().filter(|w| !w.app.dialog()).map(|w| (w.id, w.title.clone())).collect())),
            Launch::Launcher => Box::new(Launcher::new(self.launcher_items())),
            Launch::Msg { title, text } => Box::new(MsgBox::new(title, text)),
        };
        self.add(app);
        if let Some(w) = self.wins.last_mut() {
            if let Some(k) = key {
                w.key = k;
            }
            w.relaunch = relaunch;
        }
    }

    fn add(&mut self, app: Box<dyn App>) {
        let has_bar = !app.menubar().is_empty();
        let (cw, ch) = app.size_hint();
        let (sw, sh) = (self.w, self.work_h());
        let w = (cw as i32 + 2).min(sw);
        let h = (ch as i32 + 2 + has_bar as i32).min(sh);
        let float = app.dialog() || !app.resizable();
        let (x, y) = if app.dialog() || self.tiling && float {
            ((sw - w) / 2, ((sh - h) / 2).max(self.tiling as i32))
        } else {
            let n = (self.wins.iter().filter(|w| !w.app.dialog()).count() % 8) as i32;
            let x = (14 + n * 3).min((sw - w).max(0));
            let y = (1 + n).min((sh - h).max(0));
            (x, y)
        };
        let id = self.next_id;
        self.next_id += 1;
        let title = app.title();
        let big = !self.tiling && app.resizable() && (cw as i32 + 2 > sw || ch as i32 + 2 + has_bar as i32 > sh);
        let key = title.clone();
        let mut win = Win {
            id,
            x,
            y,
            w,
            h,
            restore: None,
            max: false,
            snapped: false,
            min: false,
            app,
            has_bar,
            buf: Buffer::empty(Rect::new(0, 0, 1, 1)),
            title,
            ws: self.cur_ws,
            float,
            tiled: false,
            full: false,
            hidden: false,
            key,
            relaunch: None,
        };
        win.sync();
        self.wins.push(win);
        // a new tile goes in after the focused one, like Hyprland splitting it
        let after = self.focus.and_then(|f| self.order.iter().position(|o| *o == f));
        match after {
            Some(i) => self.order.insert(i + 1, id),
            None => self.order.push(id),
        }
        self.focus_win(id);
        if big {
            self.toggle_max(id);
        }
    }

    fn apply(&mut self, id: u64, a: Action) {
        match a {
            Action::None => {}
            Action::Close => self.close(id),
            Action::CloseWin(other) => self.close(other),
            Action::Launch(l) => self.launch(l),
            Action::Resize(cw, ch) => {
                let (sw, sh) = (self.w, self.work_h());
                if let Some(w) = self.win(id) {
                    let b = w.has_bar as i32;
                    w.w = (cw as i32 + 2).min(sw);
                    w.h = (ch as i32 + 2 + b).min(sh);
                    w.x = w.x.min(sw - w.w).max(0);
                    w.y = w.y.min(sh - w.h).max(0);
                    w.sync();
                }
            }
            Action::Refresh => {
                // a program you've just added gets a desktop shortcut too
                let old: Vec<String> = self.mine.iter().map(|(n, _, _)| n.clone()).collect();
                self.refresh_programs();
                let new: Vec<String> = self.mine.iter().map(|(n, _, _)| n.clone()).filter(|n| !old.contains(n)).collect();
                for n in new {
                    self.add_icon(&n, None);
                }
            }
            Action::Background => self.back = crate::wallpaper::Back::load(),
            Action::Detach => self.detach = true,
            // a window without tabs opens it on its own
            Action::OpenTab(l) => self.launch(l),
            Action::Menu(items, x, y) => {
                if let Some(w) = self.wins.iter().find(|w| w.id == id) {
                    let (cx, cy, _, _) = w.client();
                    self.open_menu(Owner::Ctx(id), items, cx + x, cy + y, false);
                }
            }
            Action::Shortcut(paths) => {
                for p in paths {
                    self.add_shortcut(p);
                }
            }
            Action::Quit => {
                self.wins.clear();
                self.menu = None;
                self.quit = true;
            }
            Action::Many(v) => {
                for a in v {
                    self.apply(id, a);
                }
            }
        }
    }

    // ------------------------------------------------------------ menus

    fn start_items(&self) -> Vec<Item> {
        let shell = |t: &str, i: Icon, c: Option<&str>| Cmd::Launch(Launch::shell(t, i, c));
        let mut progs = vec![
            Item::new("Notes", Cmd::Launch(Launch::Notepad(None))).icon(Icon::Notepad),
            Item::new("Amp", Cmd::Launch(Launch::Amp(vec![]))).icon(Icon::Amp),
            Item::new("Mines", Cmd::Launch(Launch::Mines)).icon(Icon::Mines),
            Item::new("Solitaire", Cmd::Launch(Launch::Solitaire)).icon(Icon::Solitaire),
            Item::new("Doom", Cmd::Launch(Launch::Doom)).icon(Icon::Doom),
            Item::new("Browser", Cmd::Launch(Launch::Browser(None))).icon(Icon::Browser),
            Item::new("Paint", Cmd::Launch(Launch::Paint(None))).icon(Icon::Paint),
        ];
        if has("btop") {
            progs.push(Item::new("Monitor", shell("Monitor", Icon::Monitor, Some("btop"))).icon(Icon::Monitor));
        }
        progs.push(Item::new("Terminal", shell("Terminal", Icon::Terminal, None)).icon(Icon::Terminal));
        progs.push(Item::new("Files", Cmd::Launch(Launch::Explorer(home()))).icon(Icon::Folder));
        // the programs you added go in with the rest
        for (name, cmd, icon) in &self.mine {
            progs.push(Item::new(name.clone(), shell(name, *icon, Some(cmd))).icon(*icon));
        }
        progs.sort_by_key(|it| it.label.to_lowercase());
        progs.push(Item::sep());
        progs.push(Item::new("Add Program...", Cmd::Launch(Launch::AddProgram)).icon(Icon::Programs));
        if !self.mine.is_empty() {
            let forget = self.mine.iter().map(|(n, _, i)| Item::new(n.clone(), Cmd::Forget(n.clone())).icon(*i)).collect();
            progs.push(Item::sub("Remove Program", forget).icon(Icon::Recycle));
        }
        let docs: Vec<Item> = if self.docs.is_empty() {
            vec![Item::new("(Empty)", Cmd::None).enabled(false)]
        } else {
            self.docs
                .iter()
                .rev()
                .map(|p| Item::new(p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(), Cmd::Launch(Launch::Notepad(Some(p.clone())))).icon(Icon::File))
                .collect()
        };
        let mut top = vec![Item::new("Terminal", shell("Terminal", Icon::Terminal, None)).icon(Icon::Terminal)];
        top.extend(vec![
            Item::sep(),
            Item::sub("Programs", progs).icon(Icon::Programs),
            Item::sub("Recent", docs).icon(Icon::Documents),
            Item::sub("Settings", vec![
                Item::new("Display", Cmd::Desk("props")).icon(Icon::Computer),
                Item::new("Background...", Cmd::Launch(Launch::Background)).icon(Icon::Computer),
                Item::new("Reload Theme", Cmd::Desk("theme")).icon(Icon::Settings),
                Item::sep(),
                Item::new("Mouse Block", Cmd::Desk("pointer")).checked(self.pointer),
                Item::sep(),
                Item::new("Windows Desktop", Cmd::Desk("windows")).checked(!self.tiling),
                Item::new("Omarchy Desktop", Cmd::Desk("omarchy")).checked(self.tiling),
            ])
            .icon(Icon::Settings),
            Item::sub("Find", vec![Item::new("Files or Folders...", Cmd::Desk("find")).icon(Icon::Folder)]).icon(Icon::Help),
            Item::new("Help", Cmd::Desk("help")).icon(Icon::Help),
            Item::new("Add Program...", Cmd::Launch(Launch::AddProgram)).icon(Icon::Programs),
            Item::new("Run...", Cmd::Launch(Launch::Run)).icon(Icon::Run),
            Item::sep(),
            Item::new("Quit", Cmd::Launch(Launch::ShutDown)).icon(Icon::Shutdown),
        ]);
        top
    }

    fn open_menu(&mut self, owner: Owner, items: Vec<Item>, x: i32, y: i32, banner: bool) {
        let mut l = Level::new(items, x, y, banner);
        if l.x + l.width() > self.w {
            l.x = (self.w - l.width()).max(0);
        }
        if l.y + l.height() > self.work_h() {
            l.y = (self.work_h() - l.height()).max(0);
        }
        self.menu = Some((MenuState { owner, levels: vec![l], query: String::new() }, Instant::now()));
    }

    fn open_start(&mut self) {
        let items = self.start_items();
        let h = items.len() as i32 + 2;
        let y = if self.tiling { 1 } else { self.work_h() - h };
        self.open_menu(Owner::Start, items, 0, y, false);
    }

    /// Shows what matches the search typed into the Apps menu, in place of
    /// the menu, or the menu again once the search is cleared.
    fn search_start(&mut self) {
        let Some((m, _)) = &self.menu else { return };
        let q = m.query.trim().to_string();
        let all = self.start_items();
        let items = if q.is_empty() {
            all
        } else {
            fn leaves(items: &[Item], out: &mut Vec<Item>) {
                for it in items {
                    if !it.sub.is_empty() {
                        leaves(&it.sub, out);
                    } else if it.enabled && !it.sep && !matches!(it.cmd, Cmd::None) && !out.iter().any(|o| o.label == it.label) {
                        out.push(it.clone());
                    }
                }
            }
            let mut found = vec![];
            leaves(&all, &mut found);
            let hits = rank(found.iter().map(|it| it.label.trim_end_matches("...")), &q);
            let mut v = vec![Item::heading(format!("{}▏", m.query)).icon(Icon::Run), Item::sep()];
            if hits.is_empty() {
                // nothing by that name: run it as a command, like Run... does
                v.push(Item::new(format!("Run {q}"), Cmd::Launch(parse_run(&q))).icon(Icon::Run));
            }
            v.extend(hits.into_iter().take(14).map(|i| found[i].clone()));
            v
        };
        let y = if self.tiling { 1 } else { self.work_h() - items.len() as i32 - 2 };
        let Some((m, _)) = &mut self.menu else { return };
        m.levels.truncate(1);
        let l = &mut m.levels[0];
        (l.items, l.y, l.sel) = (items, y.max(0), None);
        if !q.is_empty() {
            l.step(1);
        }
    }

    /// Everything on the Apps menu that starts something, for the launcher.
    fn launcher_items(&self) -> Vec<(String, Icon, Launch)> {
        fn walk(items: &[Item], out: &mut Vec<(String, Icon, Launch)>) {
            for it in items {
                if let Cmd::Launch(l) = &it.cmd {
                    let name = it.label.trim_end_matches("...").to_string();
                    if !matches!(l, Launch::Launcher) && !out.iter().any(|(n, _, _)| *n == name) {
                        out.push((name, it.icon.unwrap_or(Icon::Run), l.clone()));
                    }
                }
                walk(&it.sub, out);
            }
        }
        let mut out = vec![];
        walk(&self.start_items(), &mut out);
        out
    }

    fn open_sys(&mut self, id: u64) {
        let Some(w) = self.wins.iter().find(|w| w.id == id) else { return };
        let (res, dialog) = (w.app.resizable(), w.app.dialog());
        if self.tiling {
            let to = (0..tiling::WORKSPACES).map(|n| Item::new(format!("Workspace {}", n + 1), Cmd::Sys(Sys::ToWs(n))).checked(w.ws == n)).collect();
            let mut items = vec![
                Item::new("Float", Cmd::Sys(Sys::Float)).checked(w.float).enabled(res && !dialog).key("Alt+T"),
                Item::new("Fullscreen", Cmd::Sys(Sys::Full)).checked(w.full).enabled(!dialog).key("Alt+F"),
                Item::sub("Move to", to).enabled(!dialog),
            ];
            if w.float {
                items.push(Item::new("Move", Cmd::Sys(Sys::Move)));
            }
            items.extend([Item::sep(), Item::new("Close", Cmd::Sys(Sys::Close)).key("Alt+W")]);
            let (x, y) = (w.x + 1, w.y + 1);
            return self.open_menu(Owner::Sys(id), items, x, y, false);
        }
        let items = vec![
            Item::new("Restore", Cmd::Sys(Sys::Restore)).enabled(w.max),
            Item::new("Move", Cmd::Sys(Sys::Move)).enabled(!w.max),
            Item::new("Size", Cmd::Sys(Sys::Size)).enabled(res && !w.max),
            Item::new("Minimize", Cmd::Sys(Sys::Minimize)).enabled(!dialog),
            Item::new("Maximize", Cmd::Sys(Sys::Maximize)).enabled(res && !w.max),
            Item::sep(),
            Item::new("Close", Cmd::Sys(Sys::Close)).key("Alt+F4"),
        ];
        let (x, y) = (w.x + 1, w.y + 1);
        self.open_menu(Owner::Sys(id), items, x, y, false);
    }

    fn open_bar(&mut self, id: u64, idx: usize) {
        let Some(w) = self.wins.iter().find(|w| w.id == id) else { return };
        let menus = w.app.menubar();
        let Some((bx, _)) = bar_layout(&menus).get(idx).copied() else { return };
        let (x, y) = (w.x + 1 + bx, w.y + 2);
        let items = menus[idx].1.clone();
        self.open_menu(Owner::Bar(id, idx), items, x, y, false);
    }

    fn open_desk_menu(&mut self, x: i32, y: i32) {
        self.desk_click = (x, y);
        let mut add: Vec<Item> = self
            .desk_catalogue()
            .into_iter()
            .filter(|(_, l, _)| !self.icons.iter().any(|i| i.key == *l))
            .map(|(icon, l, _)| Item::new(l.clone(), Cmd::AddIcon(l)).icon(icon))
            .collect();
        if add.is_empty() {
            add.push(Item::new("(All there)", Cmd::None).enabled(false));
        }
        let items = vec![
            Item::new("Terminal Here", Cmd::Launch(Launch::shell("Terminal", Icon::Terminal, None))).icon(Icon::Terminal),
            Item::new("New Text Document", Cmd::Launch(Launch::Notepad(None))).icon(Icon::Notepad),
            Item::sep(),
            Item::sub("Add Icon", add).icon(Icon::Programs),
            Item::sub("Arrange Icons", vec![
                Item::new("By Name", Cmd::Desk("arrange-name")),
                Item::new("Keep Order", Cmd::Desk("arrange")),
            ]),
            Item::new("Refresh", Cmd::Desk("theme")),
            Item::new("Background...", Cmd::Launch(Launch::Background)),
            Item::new("Properties", Cmd::Desk("props")),
        ];
        self.open_menu(Owner::Desk, items, x, y, false);
    }

    fn open_icon_menu(&mut self, x: i32, y: i32) {
        let items = vec![
            Item::new("Open", Cmd::Desk("open-icon")),
            Item::sep(),
            Item::new("Remove from Desktop", Cmd::Desk("remove-icon")).icon(Icon::Recycle).key("Del"),
        ];
        self.open_menu(Owner::Desk, items, x, y, false);
    }

    fn activate(&mut self, lvl: usize, idx: usize) {
        let Some((m, _)) = &mut self.menu else { return };
        let item = m.levels[lvl].items[idx].clone();
        if !item.enabled || item.sep {
            return;
        }
        if !item.sub.is_empty() {
            m.select(lvl, idx, self.w, self.h - 1);
            if let Some(l) = m.levels.last_mut() {
                if l.sel.is_none() {
                    l.step(1);
                }
            }
            return;
        }
        let owner = m.owner;
        self.menu = None;
        self.exec(item.cmd, owner);
    }

    fn exec(&mut self, cmd: Cmd, owner: Owner) {
        let target = match owner {
            Owner::Sys(id) | Owner::Bar(id, _) | Owner::Ctx(id) => Some(id),
            _ => self.focus,
        };
        match cmd {
            Cmd::None => {}
            Cmd::Launch(l) => self.launch(l),
            Cmd::App(s) => {
                if let Some(id) = target {
                    if let Some(w) = self.win(id) {
                        let a = w.app.command(s);
                        self.apply(id, a);
                    }
                }
            }
            Cmd::Sys(s) => {
                if let Some(id) = target {
                    self.sys(id, s);
                }
            }
            Cmd::Forget(name) => {
                crate::programs::remove(&name);
                self.refresh_programs();
                self.save_icons();
            }
            Cmd::AddIcon(name) => {
                let (x, y) = self.desk_click;
                let (cw, _) = self.icon_box();
                self.add_icon(&name, Some(((x - cw / 2).max(0), (y - 1).max(0))));
            }
            Cmd::Desk(s) => match s {
                "theme" => self.reload_theme(),
                "windows" => self.set_tiling(false, true),
                "omarchy" => self.set_tiling(true, true),
                "pointer" => {
                    self.pointer = !self.pointer;
                    theme::save_pointer(self.pointer);
                }
                "arrange" => self.arrange_icons(false),
                "arrange-name" => self.arrange_icons(true),
                "open-icon" => {
                    if let Some(l) = self.sel_icon.and_then(|i| self.icons.get(i)).map(|i| i.launch.clone()) {
                        self.launch(l);
                    }
                }
                "remove-icon" => self.remove_selected(),
                "props" => self.launch(Launch::Msg {
                    title: "Display Properties".into(),
                    text: format!("Theme: {}\n\nColours are read from your current Omarchy theme\nand update live when you switch themes.\n\nScreen area: {} by {} characters", self.th.name, self.w, self.h),
                }),
                "find" => self.launch(Launch::Shell {
                    cmd: Some(FIND.into()),
                    cwd: None,
                    title: "Find: Files or Folders".into(),
                    icon: Icon::Folder,
                    keep_open: true,
                }),
                "help" => self.launch(Launch::Msg { title: "Help".into(), text: HELP.into() }),
                _ => {}
            },
        }
    }

    fn menu_key(&mut self, k: KeyEvent) {
        let (w, h) = (self.w, self.h - 1);
        let Some((m, _)) = &mut self.menu else { return };
        // typing into the Apps menu searches it
        if m.owner == Owner::Start && !k.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) {
            match k.code {
                KeyCode::Char(c) if c != ' ' || !m.query.is_empty() => {
                    m.query.push(c);
                    return self.search_start();
                }
                KeyCode::Backspace | KeyCode::Esc if !m.query.is_empty() => {
                    if k.code == KeyCode::Esc {
                        m.query.clear();
                    } else {
                        m.query.pop();
                    }
                    return self.search_start();
                }
                _ => {}
            }
        }
        let lvl = m.levels.len() - 1;
        let sel = m.levels[lvl].sel;
        match k.code {
            KeyCode::Esc => {
                if lvl == 0 {
                    self.menu = None;
                } else {
                    m.levels.pop();
                }
            }
            KeyCode::Up => m.levels[lvl].step(-1),
            KeyCode::Down => m.levels[lvl].step(1),
            KeyCode::Right => {
                if let Some(s) = sel.filter(|&s| !m.levels[lvl].items[s].sub.is_empty()) {
                    m.select(lvl, s, w, h);
                    if let Some(l) = m.levels.last_mut() {
                        l.step(1);
                    }
                } else if let Owner::Bar(id, i) = m.owner {
                    let n = self.wins.iter().find(|w| w.id == id).map(|w| w.app.menubar().len()).unwrap_or(1);
                    self.open_bar(id, (i + 1) % n);
                    self.menu_step_first();
                }
            }
            KeyCode::Left => {
                if lvl > 0 {
                    m.levels.pop();
                } else if let Owner::Bar(id, i) = m.owner {
                    let n = self.wins.iter().find(|w| w.id == id).map(|w| w.app.menubar().len()).unwrap_or(1);
                    self.open_bar(id, (i + n - 1) % n);
                    self.menu_step_first();
                }
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                if let Some(s) = sel {
                    self.activate(lvl, s);
                }
            }
            KeyCode::Char(c) => {
                let c = c.to_ascii_lowercase();
                if let Some(i) = m.levels[lvl].items.iter().position(|it| it.enabled && !it.sep && it.label.to_lowercase().starts_with(c)) {
                    self.activate(lvl, i);
                }
            }
            _ => {}
        }
    }

    fn menu_step_first(&mut self) {
        if let Some((m, _)) = &mut self.menu {
            m.levels[0].step(1);
        }
    }

    // ------------------------------------------------------------ input

    pub fn event(&mut self, ev: Event) {
        match ev {
            Event::Key(k) if k.kind != KeyEventKind::Release => self.key(k),
            Event::Key(k) => {
                if let Some(id) = self.key_releases {
                    if let Some(w) = self.win(id) {
                        let a = w.app.key(k);
                        self.apply(id, a);
                    }
                }
            }
            Event::Mouse(m) => self.mouse(m),
            Event::Paste(s) => {
                if let Some(id) = self.focus {
                    if let Some(w) = self.win(id) {
                        w.app.paste(&s);
                    }
                }
            }
            Event::Resize(w, h) => self.resize(w, h),
            _ => {}
        }
    }

    fn resize(&mut self, w: u16, h: u16) {
        self.w = w as i32;
        self.h = h as i32;
        let (sw, sh) = (self.w, self.work_h());
        for win in &mut self.wins {
            if win.max {
                (win.x, win.y, win.w, win.h) = (0, 0, sw, sh);
            } else {
                win.w = win.w.min(sw);
                win.h = win.h.min(sh);
                win.x = win.x.min(sw - 4).max(4 - win.w);
                win.y = win.y.min(sh - 1).max(0);
            }
            win.sync();
        }
        self.menu = None;
        self.relayout();
    }

    fn cycle(&mut self) {
        // the window furthest back comes to the front
        if let Some(id) = self.wins.iter().find(|w| !w.hidden && !(self.tiling && w.ws != self.cur_ws)).map(|w| w.id) {
            self.focus_win(id);
        }
    }

    fn key(&mut self, k: KeyEvent) {
        if let Some((id, size)) = self.kbmode {
            let (sw, sh) = (self.w, self.work_h());
            let step = if k.modifiers.contains(KeyModifiers::SHIFT) { 4 } else { 1 };
            let Some(w) = self.win(id) else {
                self.kbmode = None;
                return;
            };
            let (dx, dy) = match k.code {
                KeyCode::Left => (-step, 0),
                KeyCode::Right => (step, 0),
                KeyCode::Up => (0, -step),
                KeyCode::Down => (0, step),
                KeyCode::Enter | KeyCode::Esc => {
                    self.kbmode = None;
                    return;
                }
                _ => (0, 0),
            };
            if size {
                w.w = (w.w + dx).clamp(20, sw);
                w.h = (w.h + dy).clamp(5, sh);
                w.sync();
            } else {
                w.x = (w.x + dx).clamp(4 - w.w, sw - 4);
                w.y = (w.y + dy).clamp(0, sh - 1);
            }
            return;
        }
        if self.vol_key(k) {
            return;
        }
        if self.menu.is_some() {
            self.menu_key(k);
            return;
        }
        if self.tiling && self.tiling_key(k) {
            return;
        }
        let (alt, ctrl) = (k.modifiers.contains(KeyModifiers::ALT), k.modifiers.contains(KeyModifiers::CONTROL));
        match k.code {
            KeyCode::Esc if ctrl => return self.open_start(),
            KeyCode::Char('s') if alt && !ctrl => return self.open_start(),
            KeyCode::Tab | KeyCode::BackTab | KeyCode::Char('`') if alt => return self.cycle(),
            KeyCode::F(4) if alt => {
                if let Some(id) = self.focus {
                    self.close(id);
                }
                return;
            }
            KeyCode::Char(' ') if alt => {
                match self.focus {
                    Some(id) => self.open_sys(id),
                    None => self.open_start(),
                }
                self.menu_step_first();
                return;
            }
            KeyCode::Delete if ctrl && alt => return self.launch(Launch::TaskList),
            _ => {}
        }
        if let Some(id) = self.focus {
            if let Some(w) = self.win(id) {
                let a = w.app.key(k);
                self.apply(id, a);
            }
            return;
        }
        let n = self.icons.len();
        if self.tiling || n == 0 {
            return;
        }
        match k.code {
            KeyCode::Down | KeyCode::Right | KeyCode::Tab => self.select_only(Some(self.sel_icon.map(|i| (i + 1) % n).unwrap_or(0))),
            KeyCode::Up | KeyCode::Left | KeyCode::BackTab => self.select_only(Some(self.sel_icon.map(|i| (i + n - 1) % n).unwrap_or(0))),
            KeyCode::Enter => {
                if let Some(i) = self.sel_icon {
                    let l = self.icons[i].launch.clone();
                    self.launch(l);
                }
            }
            KeyCode::Delete => self.remove_selected(),
            _ => {}
        }
    }

    fn win_at(&self, x: i32, y: i32) -> Option<usize> {
        self.wins.iter().rposition(|w| w.contains(x, y))
    }

    fn double(&mut self, x: i32, y: i32) -> bool {
        let d = self.last_click.is_some_and(|(t, lx, ly)| lx == x && ly == y && t.elapsed().as_millis() < 450);
        self.last_click = if d { None } else { Some((Instant::now(), x, y)) };
        d
    }

    /// The spacing of the icon grid.
    fn grid(&self) -> (i32, i32) {
        (13, 6)
    }

    fn icon_slot(&self, i: usize) -> (i32, i32) {
        let (gw, gh) = self.grid();
        let per = ((self.work_h() - 1) / gh).max(1) as usize;
        (1 + (i / per) as i32 * gw, 1 + (i % per) as i32 * gh)
    }

    /// The free grid place nearest x, y, not counting the icons `skip` says.
    fn snap(&self, x: i32, y: i32, skip: &dyn Fn(usize) -> bool) -> (i32, i32) {
        let ((gw, gh), (iw, ih)) = (self.grid(), self.icon_box());
        let cols = ((self.w - 1 - iw) / gw + 1).max(1);
        let rows = ((self.work_h() - 1) / gh).max(1);
        let taken = |cx: i32, cy: i32| {
            (0..self.icons.len()).filter(|&i| !skip(i)).any(|i| {
                let (ix, iy) = self.icon_pos(i);
                ix < cx + iw && cx < ix + iw && iy < cy + ih && cy < iy + ih
            })
        };
        (0..cols)
            .flat_map(|c| (0..rows).map(move |r| (1 + c * gw, 1 + r * gh)))
            .filter(|&(cx, cy)| !taken(cx, cy))
            .min_by_key(|&(cx, cy)| (cx - x).pow(2) + (2 * (cy - y)).pow(2))
            .unwrap_or((x, y))
    }

    /// How big an icon is, label and all.
    fn icon_box(&self) -> (i32, i32) {
        (12, 5)
    }

    /// Where icon i is drawn: where you put it, kept on the screen.
    fn icon_pos(&self, i: usize) -> (i32, i32) {
        self.icon_pos_of(&self.icons[i])
    }

    fn icon_pos_of(&self, ic: &DeskIcon) -> (i32, i32) {
        let (w, h) = self.icon_box();
        (ic.x.clamp(0, (self.w - w).max(0)), ic.y.clamp(0, (self.work_h() - h).max(0)))
    }

    /// The icon at x, y; the one drawn last when they overlap.
    fn icon_at(&self, x: i32, y: i32) -> Option<usize> {
        if self.tiling {
            return None;
        }
        let (w, h) = self.icon_box();
        (0..self.icons.len()).rev().find(|&i| {
            let (ix, iy) = self.icon_pos(i);
            x >= ix && x < ix + w && y >= iy && y < iy + h
        })
    }

    fn taskbar_layout(&self) -> (Vec<(u64, i32, i32)>, i32) {
        let clock_x = self.w - 9;
        let end = self.tray_x().unwrap_or(clock_x);
        let x0 = 11;
        let mut ids: Vec<&Win> = self.wins.iter().collect();
        ids.sort_by_key(|w| w.id);
        let n = ids.len().max(1) as i32;
        let bw = ((end - 1 - x0) / n - 1).clamp(4, 24);
        let v = ids.iter().enumerate().map(|(k, w)| (w.id, x0 + k as i32 * (bw + 1), bw)).collect();
        (v, clock_x)
    }

    fn taskbar_hit(&self, x: i32) -> Option<TaskHit> {
        if x < 10 {
            return Some(TaskHit::Start);
        }
        let (v, clock_x) = self.taskbar_layout();
        if x >= clock_x {
            return Some(TaskHit::Clock);
        }
        if self.tray_x().is_some_and(|t| x >= t) {
            return Some(TaskHit::Volume);
        }
        v.into_iter().find(|&(_, bx, bw)| x >= bx && x < bx + bw).map(|(id, _, _)| TaskHit::Win(id))
    }

    fn forward(&mut self, id: u64, kind: MouseEventKind, x: i32, y: i32, mods: KeyModifiers) {
        if let Some(w) = self.win(id) {
            let (cx, cy, _, _) = w.client();
            let a = w.app.mouse(kind, x - cx, y - cy, mods);
            self.apply(id, a);
        }
    }

    fn mouse(&mut self, m: MouseEvent) {
        let (x, y) = (m.column as i32, m.row as i32);
        self.hover = Some((x, y));
        match m.kind {
            MouseEventKind::Down(b) => self.mouse_down(b, x, y, m.modifiers),
            MouseEventKind::Up(b) => self.mouse_up(b, x, y, m.modifiers),
            MouseEventKind::Drag(b) => self.mouse_drag(b, x, y, m.modifiers),
            MouseEventKind::Moved => {
                self.dock_hover(x, y);
                self.menu_hover(x, y);
                if self.menu.is_none() {
                    if let (Some(id), Some(i)) = (self.focus, self.win_at(x, y)) {
                        if self.wins[i].id == id && matches!(self.wins[i].hit(x, y), Some(Part::Client(..))) {
                            self.forward(id, m.kind, x, y, m.modifiers);
                        }
                    }
                }
            }
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                if self.on_speaker(x, y) || self.in_vol_popup(x, y) {
                    return self.nudge_vol(if m.kind == MouseEventKind::ScrollUp { 5 } else { -5 });
                }
                if self.menu.is_some() {
                    return;
                }
                if let Some(i) = self.win_at(x, y) {
                    let id = self.wins[i].id;
                    if matches!(self.wins[i].hit(x, y), Some(Part::Client(..))) {
                        self.forward(id, m.kind, x, y, m.modifiers);
                    }
                }
            }
            _ => {}
        }
    }

    fn menu_hover(&mut self, x: i32, y: i32) {
        let (sw, sh) = (self.w, self.h - 1);
        let Some((m, _)) = &mut self.menu else { return };
        match m.hit(x, y) {
            Some((lvl, Some(i))) => {
                if m.levels[lvl].sel != Some(i) || m.levels.len() > lvl + 1 && m.levels[lvl + 1].items.as_ptr() != m.levels[lvl].items[i].sub.as_ptr() {
                    if m.levels[lvl].sel != Some(i) {
                        m.select(lvl, i, sw, sh);
                    }
                }
            }
            Some((lvl, None)) => {
                if m.levels.len() == lvl + 1 {
                    m.levels[lvl].sel = None;
                }
            }
            None => {
                // Slide across a menu bar to switch menus.
                if let Owner::Bar(id, cur) = m.owner {
                    if let Some(w) = self.wins.iter().find(|w| w.id == id) {
                        if y == w.y + 1 && w.has_bar {
                            let rel = x - w.x - 1;
                            if let Some(i) = bar_layout(&w.app.menubar()).iter().position(|&(bx, bw)| rel >= bx && rel < bx + bw) {
                                if i != cur {
                                    self.open_bar(id, i);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    fn mouse_down(&mut self, b: MouseButton, x: i32, y: i32, mods: KeyModifiers) {
        if self.vol_open.is_some() {
            if self.in_vol_popup(x, y) {
                self.vol_click(x, y, false);
                self.drag = Drag::Volume;
                return;
            }
            self.vol_open = None;
            if self.on_speaker(x, y) {
                return;
            }
        }
        if let Some((m, _)) = &self.menu {
            match m.hit(x, y) {
                Some((lvl, Some(i))) => return self.activate(lvl, i),
                Some((_, None)) => return,
                None => {
                    let owner = m.owner;
                    self.menu = None;
                    if owner == Owner::Start && (!self.tiling && y == self.h - 1 && x < 10 || self.tiling && y == 0 && x < 3) {
                        return;
                    }
                    if let Owner::Bar(id, _) = owner {
                        if let Some(w) = self.wins.iter().find(|w| w.id == id) {
                            if matches!(w.hit(x, y), Some(Part::Bar(_))) {
                                return;
                            }
                        }
                    }
                }
            }
        }
        if self.kbmode.is_some() {
            self.kbmode = None;
        }
        if self.tiling {
            if self.dock_visible() && self.dock_layout().contains(x, y) {
                return self.dock_click(b, x, y);
            }
            if y == 0 && self.full_win().is_none() {
                if let Some(h) = self.top_hit(x) {
                    self.top_click(h);
                }
                return;
            }
            let on_frame = self.win_at(x, y).is_none_or(|i| matches!(self.wins[i].hit(x, y), Some(Part::Frame)));
            if on_frame && b == MouseButton::Left {
                if let Some(s) = self.split_at(x, y) {
                    self.drag = Drag::Split(s);
                    return;
                }
            }
        }
        if !self.tiling && y == self.h - 1 {
            match self.taskbar_hit(x) {
                Some(TaskHit::Start) => {
                    self.open_start();
                    self.drag = Drag::Start;
                }
                Some(TaskHit::Win(id)) => {
                    if self.focus == Some(id) && !self.win(id).is_some_and(|w| w.min) {
                        self.minimize(id);
                    } else {
                        self.focus_win(id);
                    }
                }
                Some(TaskHit::Volume) => self.toggle_vol(),
                Some(TaskHit::Clock) => self.launch(Launch::Msg {
                    title: "Date/Time Properties".into(),
                    text: chrono::Local::now().format("%A %-d %B %Y\n%H:%M:%S").to_string(),
                }),
                None => {}
            }
            return;
        }
        if let Some(i) = self.win_at(x, y) {
            let id = self.wins[i].id;
            let part = self.wins[i].hit(x, y);
            self.focus_win(id);
            let Some(part) = part else { return };
            match part {
                Part::Close | Part::Max | Part::Min if b == MouseButton::Left => self.drag = Drag::Button { id, part },
                Part::Icon => {
                    if self.double(x, y) {
                        self.close(id);
                    } else {
                        self.open_sys(id);
                    }
                }
                Part::Title | Part::Close | Part::Max | Part::Min => {
                    if b == MouseButton::Right {
                        self.open_sys(id);
                        if let Some((m, _)) = &mut self.menu {
                            m.levels[0].x = x;
                        }
                    } else if self.double(x, y) {
                        self.toggle_max(id);
                    } else if self.wins.last().unwrap().tiled {
                        self.drag = Drag::Swap { id };
                    } else {
                        let w = self.wins.last().unwrap();
                        let pull = (w.max || w.snapped) && w.app.resizable();
                        self.drag = Drag::Move { id, dx: x - w.x, dy: y - w.y, pull };
                    }
                }
                Part::Edge(l, r, bot) => {
                    let w = self.wins.last().unwrap();
                    self.drag = Drag::Resize { id, l, r, b: bot, mx: x, my: y, orig: (w.x, w.y, w.w, w.h) };
                }
                Part::Bar(rel) => {
                    let menus = self.wins.last().unwrap().app.menubar();
                    if let Some(i) = bar_layout(&menus).iter().position(|&(bx, bw)| rel >= bx && rel < bx + bw) {
                        self.open_bar(id, i);
                    }
                }
                Part::Client(..) => {
                    self.drag = Drag::Client { id };
                    self.forward(id, MouseEventKind::Down(b), x, y, mods);
                }
                Part::Frame => {}
            }
            return;
        }
        // The desktop itself.
        self.focus = None;
        let ctrl = mods.contains(KeyModifiers::CONTROL);
        match self.icon_at(x, y) {
            Some(i) if ctrl && b == MouseButton::Left => {
                self.icons[i].sel = !self.icons[i].sel;
                self.sel_icon = self.icons[i].sel.then_some(i);
            }
            Some(i) => {
                let one = self.icons[i].sel;
                if !one {
                    self.select_only(Some(i));
                }
                // the ones you pick up come to the top, the one clicked last
                let held = self.icons.remove(i);
                let (mut sel, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut self.icons).into_iter().partition(|i| i.sel);
                sel.push(held);
                self.icons = rest;
                for mut ic in sel {
                    ic.orig = self.icon_pos_of(&ic);
                    (ic.x, ic.y) = ic.orig;
                    self.icons.push(ic);
                }
                let i = self.icons.len() - 1;
                self.sel_icon = Some(i);
                if b == MouseButton::Right {
                    self.open_icon_menu(x, y);
                } else if b == MouseButton::Left && self.double(x, y) {
                    self.select_only(Some(i));
                    let l = self.icons[i].launch.clone();
                    self.launch(l);
                } else if b == MouseButton::Left {
                    self.drag = Drag::Icon { mx: x, my: y, moved: false, one };
                }
            }
            None => {
                if !ctrl {
                    self.select_only(None);
                }
                if b == MouseButton::Right {
                    self.open_desk_menu(x, y);
                } else if b == MouseButton::Left && !self.tiling {
                    for ic in &mut self.icons {
                        ic.was_sel = ic.sel;
                    }
                    self.drag = Drag::Band { x0: x, y0: y };
                }
            }
        }
    }

    fn mouse_up(&mut self, b: MouseButton, x: i32, y: i32, mods: KeyModifiers) {
        let drag = std::mem::replace(&mut self.drag, Drag::None);
        match drag {
            Drag::Button { id, part } => {
                let still = self.wins.iter().find(|w| w.id == id).and_then(|w| w.hit(x, y)) == Some(part);
                if still {
                    match part {
                        Part::Close => self.close(id),
                        Part::Max => self.toggle_max(id),
                        Part::Min => self.minimize(id),
                        _ => {}
                    }
                }
            }
            Drag::Move { id, .. } => {
                if let Some(s) = self.snap.take() {
                    self.snap_win(id, s);
                }
            }
            Drag::Client { id } => self.forward(id, MouseEventKind::Up(b), x, y, mods),
            Drag::Icon { moved: true, .. } => {
                // dropped: each settles on the nearest free place on the grid
                let mut placed = vec![false; self.icons.len()];
                for i in 0..self.icons.len() {
                    if self.icons[i].sel {
                        let (x, y) = self.icon_pos(i);
                        let skip = |j: usize| self.icons[j].sel && !placed[j];
                        (self.icons[i].x, self.icons[i].y) = self.snap(x, y, &skip);
                        placed[i] = true;
                    }
                }
                self.save_icons();
            }
            Drag::Icon { moved: false, one: true, .. } => {
                let i = self.icons.len() - 1;
                self.select_only(Some(i));
            }
            Drag::Swap { id } => {
                if let Some(other) = self.swap_target(id, x, y) {
                    self.swap(id, other);
                }
            }
            Drag::Start | Drag::None => {
                // Press on the Apps button, slide up and release on an item.
                if let Some((m, opened)) = &self.menu {
                    if opened.elapsed().as_millis() > 300 {
                        if let Some((lvl, Some(i))) = m.hit(x, y) {
                            if m.levels[lvl].items[i].sub.is_empty() {
                                self.activate(lvl, i);
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn mouse_drag(&mut self, b: MouseButton, x: i32, y: i32, mods: KeyModifiers) {
        let (sw, sh, top) = (self.w, self.work_h(), self.tiling as i32);
        match self.drag {
            Drag::Move { id, mut dx, dy, pull } => {
                if pull {
                    // Pulled off a snap or out of maximised: back to its own size,
                    // still held at the same point along the title bar.
                    if let Some(w) = self.win(id) {
                        if let Some((_, _, rw, rh)) = w.restore.take() {
                            dx = (dx * rw / w.w.max(1)).clamp(3, (rw - 8).max(3));
                            (w.w, w.h) = (rw, rh);
                        }
                        w.max = false;
                        w.snapped = false;
                        w.sync();
                    }
                    self.drag = Drag::Move { id, dx, dy, pull: false };
                }
                if let Some(w) = self.win(id) {
                    w.x = (x - dx).clamp(4 - w.w, sw - 4);
                    w.y = (y - dy).clamp(top, sh - 1);
                }
                let can = !self.tiling && self.wins.iter().find(|w| w.id == id).is_some_and(|w| w.app.resizable());
                self.snap = if can { self.snap_at(x, y) } else { None };
            }
            Drag::Resize { id, l, r, b: bot, mx, my, orig: (ox, oy, ow, oh) } => {
                if let Some(w) = self.win(id) {
                    let _ = oy;
                    let (ddx, ddy) = (x - mx, y - my);
                    if r {
                        w.w = (ow + ddx).max(20);
                    }
                    if l {
                        let nw = (ow - ddx).max(20);
                        w.x = ox + ow - nw;
                        w.w = nw;
                    }
                    if bot {
                        w.h = (oh + ddy).clamp(5, sh - w.y);
                    }
                    w.sync();
                }
            }
            Drag::Client { id } => self.forward(id, MouseEventKind::Drag(b), x, y, mods),
            Drag::Split(s) => self.drag_split(s, x, y),
            Drag::Icon { mx, my, one, .. } => {
                let (iw, ih) = self.icon_box();
                for ic in self.icons.iter_mut().filter(|i| i.sel) {
                    ic.x = (ic.orig.0 + x - mx).clamp(0, (sw - iw).max(0));
                    ic.y = (ic.orig.1 + y - my).clamp(0, (sh - ih).max(0));
                }
                self.drag = Drag::Icon { mx, my, moved: true, one };
                self.last_click = None;
            }
            Drag::Band { x0, y0 } => {
                let (bx, by, bw, bh) = band(x0, y0, x, y);
                let (iw, ih) = self.icon_box();
                for i in 0..self.icons.len() {
                    let (ix, iy) = self.icon_pos(i);
                    let inside = ix < bx + bw && bx < ix + iw && iy < by + bh && by < iy + ih;
                    self.icons[i].sel = self.icons[i].was_sel || inside;
                }
                self.sel_icon = self.icons.iter().rposition(|i| i.sel);
                self.last_click = None;
            }
            Drag::Volume => self.vol_click(x, y, true),
            Drag::Start | Drag::None => self.menu_hover(x, y),
            _ => {}
        }
    }

    // ------------------------------------------------------------ tick

    fn reload_theme(&mut self) {
        self.th = Theme::load();
        self.theme_mtime = theme::mtime();
        for w in &mut self.wins {
            w.app.theme_changed(&self.th);
        }
    }

    /// Housekeeping between events. Returns true when the screen needs redrawing.
    pub fn tick(&mut self) -> bool {
        let mut dirty = false;
        let mut acts = vec![];
        for w in &mut self.wins {
            let (d, a) = w.app.poll();
            dirty |= d;
            if !matches!(a, Action::None) {
                acts.push((w.id, a));
            }
            let t = w.app.title();
            if t != w.title {
                w.title = t;
                dirty = true;
            }
        }
        for (id, a) in acts {
            self.apply(id, a);
            dirty = true;
        }
        dirty |= self.tick_vol();
        self.tick_key_releases();
        let c = clock();
        if c != self.clock {
            self.clock = c;
            self.tick_tiling();
            dirty = true;
        }
        if self.theme_check.elapsed().as_millis() > 1000 {
            self.theme_check = Instant::now();
            if theme::mtime() != self.theme_mtime {
                self.reload_theme();
                dirty = true;
            }
        }
        dirty
    }

    /// Asks the terminal for key releases (kitty keyboard protocol: release
    /// events, every key as a code) while a window that wants them has the
    /// focus, and puts it back after.
    fn tick_key_releases(&mut self) {
        let want = self.focus.filter(|&id| self.wins.iter().any(|w| w.id == id && w.app.key_releases()));
        if want == self.key_releases {
            return;
        }
        if let Some(w) = self.key_releases.and_then(|id| self.win(id)) {
            w.app.focus_lost();
        }
        match (self.key_releases.is_some(), want.is_some()) {
            (false, true) => crate::session::raw(b"\x1b[>11u"),
            (true, false) => crate::session::raw(b"\x1b[<u"),
            _ => {}
        }
        self.key_releases = want;
    }

    /// A new terminal attached: it starts without key releases on.
    pub fn terminal_changed(&mut self) {
        if let Some(w) = self.key_releases.take().and_then(|id| self.win(id)) {
            w.app.focus_lost();
        }
    }

    // ------------------------------------------------------------ drawing

    pub fn render(&mut self, f: &mut Frame) {
        let th = self.th.clone();
        let mut c = Canvas::new(f.buffer_mut());
        self.render_back(&mut c, &th);
        let pal = palette(&th);
        if self.tiling && !self.wins.iter().any(|w| !w.hidden && !w.min) {
            self.render_empty_hint(&mut c, &th);
        }
        let icons = if self.tiling { &[][..] } else { &self.icons[..] };
        let shade = self.back.shade && self.back.custom();
        for (i, ic) in icons.iter().enumerate() {
            let (x, y) = self.icon_pos(i);
            if shade {
                shade_icon(&mut c, &th, ic, x, y);
            }
            c.pixels(x + 2, y, ic.icon.art(), &pal);
            let on = ic.sel;
            let s = if on { st(th.on_accent, th.accent) } else { st(th.text, th.desk) };
            for (j, line) in wrap_label(&ic.label).iter().enumerate() {
                let lw = line.chars().count() as i32;
                c.text_max(x + (12 - lw) / 2, y + 3 + j as i32, line, s, x + 12);
            }
        }
        if let (Drag::Band { x0, y0 }, Some((hx, hy))) = (&self.drag, self.hover) {
            let (bx, by, bw, bh) = band(*x0, *y0, hx, hy);
            if bw > 1 || bh > 1 {
                c.frame(bx, by, bw, bh, Style::new().fg(th.dim));
            }
        }
        let pressed = match self.drag {
            Drag::Button { id, part } => self.hover.and_then(|(x, y)| self.wins.iter().find(|w| w.id == id).and_then(|w| w.hit(x, y))).filter(|p| *p == part).map(|p| (id, p)),
            _ => None,
        };
        let open_bar = match &self.menu {
            Some((m, _)) => match m.owner {
                Owner::Bar(id, i) => Some((id, i)),
                _ => None,
            },
            None => None,
        };
        // tiles first, floating windows over them
        let mut zs: Vec<usize> = (0..self.wins.len()).filter(|&i| !self.wins[i].min && !self.wins[i].hidden).collect();
        zs.sort_by_key(|&i| !self.wins[i].tiled);
        let swap_to = match self.drag {
            Drag::Swap { id } => self.hover.and_then(|(x, y)| self.swap_target(id, x, y)),
            _ => None,
        };
        for i in zs {
            let w = &mut self.wins[i];
            let focused = self.focus == Some(w.id) || swap_to == Some(w.id);
            draw_window(&mut c, &th, w, self.tiling, focused, pressed.filter(|p| p.0 == w.id).map(|p| p.1), open_bar.filter(|b| b.0 == w.id).map(|b| b.1));
        }
        // Where the window will land if it is let go here.
        if let (Some(s), Drag::Move { .. }) = (self.snap, &self.drag) {
            let (x, y, w, h) = self.snap_rect(s);
            c.frame(x, y, w, h, Style::new().fg(th.accent));
        }
        if !self.tiling {
            self.render_taskbar(&mut c, &th);
        } else {
            if self.full_win().is_none() {
                self.render_top(&mut c, &th);
            }
            if self.dock_visible() {
                self.render_dock(&mut c, &th);
            }
        }
        if let Some((m, _)) = &self.menu {
            m.render(&mut c, &th);
        }
        self.render_vol(&mut c, &th);
        if let Some((id, size)) = self.kbmode {
            let msg = if size { " Size: arrows resize, Shift for bigger steps, Enter when done " } else { " Move: arrows move, Shift for bigger steps, Enter when done " };
            let w = msg.chars().count() as i32;
            c.text((self.w - w) / 2, self.h - 2, msg, st(th.on_accent, th.accent));
            let _ = id;
        }
        // The mouse pointer: a solid block, or the letter under it inverted.
        if let Some((x, y)) = self.hover.filter(|_| self.pointer) {
            if let Some(cell) = c.cell(x, y) {
                if matches!(cell.symbol(), " " | "▀" | "▄" | "█") {
                    cell.set_symbol("█").set_fg(th.text);
                } else {
                    cell.modifier.toggle(Modifier::REVERSED);
                }
            }
        }
        // Text cursor for the focused app.
        if self.menu.is_none() && self.kbmode.is_none() {
            if let Some(w) = self.focus.and_then(|id| self.wins.iter().find(|w| w.id == id)) {
                if let Some((cx, cy)) = w.app.cursor() {
                    let (ox, oy, cw, ch) = w.client();
                    let (x, y) = (ox + cx as i32, oy + cy as i32);
                    let bottom = if !self.tiling { self.h - 1 } else if self.dock_visible() { self.dock_layout().y - 1 } else { self.h };
                    if (cx as i32) < cw && (cy as i32) < ch && x >= 0 && y >= 0 && x < self.w && y < bottom {
                        f.set_cursor_position(Position::new(x as u16, y as u16));
                    }
                }
            }
        }
    }

    /// The desktop behind everything: the picture, the colour you picked, or
    /// the theme's own.
    fn render_back(&mut self, c: &mut Canvas, th: &Theme) {
        let bg = self.back.colour.unwrap_or(th.desk_rgb);
        let rgb = |p: (u8, u8, u8)| ratatui::style::Color::Rgb(p.0, p.1, p.2);
        if let Some(px) = self.painter.pixels(&self.back, self.w, self.h, bg) {
            for y in 0..self.h {
                for x in 0..self.w {
                    let (t, b) = (px[(2 * y * self.w + x) as usize], px[((2 * y + 1) * self.w + x) as usize]);
                    if t == b {
                        c.put(x, y, " ", Style::new().bg(rgb(b)));
                    } else {
                        c.put(x, y, "▀", st(rgb(t), rgb(b)));
                    }
                }
            }
        } else if let Some(col) = self.back.colour {
            c.fill(0, 0, self.w, self.h, st(th.text, rgb(col)));
        } else {
            c.fill(0, 0, self.w, self.h, st(th.text, th.desk));
        }
    }

    fn render_taskbar(&self, c: &mut Canvas, th: &Theme) {
        let y = self.h - 1;
        c.fill(0, y, self.w, 1, st(th.text, th.face));
        let start_open = matches!(&self.menu, Some((m, _)) if m.owner == Owner::Start);
        c.button(0, y, 10, "", th, true, start_open);
        let bg = if start_open { th.shadow } else { th.button };
        c.put(2, y, "▀", st(th.red, th.blue));
        c.put(3, y, "▀", st(th.green, th.yellow));
        c.text(5, y, "Apps", st(th.text, bg).add_modifier(Modifier::BOLD));
        let (v, clock_x) = self.taskbar_layout();
        for (id, bx, bw) in v {
            let Some(w) = self.wins.iter().find(|w| w.id == id) else { continue };
            let active = self.focus == Some(id) && !w.min;
            c.button(bx, y, bw, "", th, active, active);
            let bg = if active { th.shadow } else { th.button };
            let (g, col) = w.app.icon().glyph(th);
            c.put_c(bx + 1, y, g, st(col, bg));
            let mut s = st(th.text, bg);
            if active {
                s = s.add_modifier(Modifier::BOLD);
            }
            c.text_max(bx + 3, y, &w.title, s, bx + bw - 1);
        }
        let s = st(th.text, th.face);
        if let Some(t) = self.tray_x() {
            let muted = self.vol.is_some_and(|v| v.1);
            c.put(t, y, "▏", st(th.shadow, th.face));
            c.text(t + 1, y, "♪", if muted { st(th.dim, th.face) } else { s });
        }
        c.put(clock_x, y, "▏", st(th.shadow, th.face));
        c.text(clock_x + 1, y, &format!("  {}  ", self.clock), s);
        c.put(self.w - 1, y, "▕", st(th.hilite, th.face));
    }

}

fn wrap_label(label: &str) -> Vec<String> {
    let mut lines: Vec<String> = vec![];
    for word in label.split(' ') {
        match lines.last_mut() {
            Some(l) if l.chars().count() + 1 + word.chars().count() <= 12 => {
                l.push(' ');
                l.push_str(word);
            }
            _ => lines.push(word.chars().take(12).collect()),
        }
    }
    lines.truncate(2);
    lines
}

fn draw_window(c: &mut Canvas, th: &Theme, w: &mut Win, tiling: bool, focused: bool, pressed: Option<Part>, open_bar: Option<usize>) {
    let (cx, cy, _, _) = w.client();
    w.buf.reset();
    {
        let mut wc = Canvas::new(&mut w.buf);
        w.app.render(&mut wc, th, focused);
    }
    c.blit(&w.buf, cx, cy);
    // the Omarchy desktop always has line borders, whatever the look
    if tiling {
        return draw_frame_lines(c, th, w, focused, pressed, open_bar);
    }
    let (x, y, ww, hh) = (w.x, w.y, w.w, w.h);
    // Title bar.
    let (tbg, tfg) = if focused { (th.accent, th.on_accent) } else { (th.inactive, th.on_inactive) };
    c.fill(x + 1, y, ww - 2, 1, st(tfg, tbg));
    let (g, _) = w.app.icon().glyph(th);
    c.put_c(x + 2, y, g, st(tfg, tbg).add_modifier(Modifier::BOLD));
    let r = w.buttons_x();
    let dialog = w.app.dialog();
    let title_end = if dialog { r - 3 } else if w.app.resizable() { r - 10 } else { r - 7 };
    c.text_max(x + 4, y, &w.title, st(tfg, tbg).add_modifier(Modifier::BOLD), title_end);
    c.button(r - 2, y, 3, "×", th, false, pressed == Some(Part::Close));
    if !dialog {
        if w.app.resizable() {
            c.button(r - 6, y, 3, if w.max { "❐" } else { "□" }, th, false, pressed == Some(Part::Max));
        }
        let min_x = if w.app.resizable() { r - 9 } else { r - 6 };
        c.button(min_x, y, 3, "_", th, false, pressed == Some(Part::Min));
    }
    // Frame: light left edge, dark right and bottom edges.
    for yy in y..y + hh {
        c.put(x, yy, "▏", st(th.hilite, th.face));
        c.put(x + ww - 1, yy, "▕", st(th.shadow, th.face));
    }
    for xx in x + 1..x + ww - 1 {
        c.put(xx, y + hh - 1, "▁", st(th.shadow, th.face));
    }
    if w.app.resizable() && !w.max {
        c.put(x + ww - 1, y + hh - 1, "◢", st(th.hilite, th.face));
    }
    // Menu bar.
    if w.has_bar {
        let menus = w.app.menubar();
        c.fill(x + 1, y + 1, ww - 2, 1, st(th.text, th.face));
        for (i, ((bx, bw), (label, _))) in bar_layout(&menus).into_iter().zip(menus.iter()).enumerate() {
            if x + 1 + bx + bw > x + ww - 1 {
                break;
            }
            let on = open_bar == Some(i);
            let s = if on { st(th.on_accent, th.accent) } else { st(th.text, th.face) };
            c.fill(x + 1 + bx, y + 1, bw, 1, s);
            let mut chars = label.chars();
            if let Some(first) = chars.next() {
                c.put_c(x + 2 + bx, y + 1, first, s.add_modifier(Modifier::UNDERLINED));
                c.text(x + 3 + bx, y + 1, chars.as_str(), s);
            }
        }
    }
}

fn draw_frame_lines(c: &mut Canvas, th: &Theme, w: &Win, focused: bool, pressed: Option<Part>, open_bar: Option<usize>) {
    let (x, y, ww, hh) = (w.x, w.y, w.w, w.h);
    let line = st(if focused { th.accent } else { th.dim }, th.desk);
    c.frame(x, y, ww, hh, line);
    let ts = if focused { st(th.accent, th.desk).add_modifier(Modifier::BOLD) } else { st(th.dim, th.desk) };
    let r = w.buttons_x();
    let dialog = w.app.dialog() || w.tiled;
    let title_end = if dialog { r - 3 } else if w.app.resizable() { r - 10 } else { r - 7 };
    let (g, _) = w.app.icon().glyph(th);
    c.put(x + 1, y, " ", ts);
    c.put_c(x + 2, y, g, ts);
    c.put(x + 3, y, " ", ts);
    let end = c.text_max(x + 4, y, &w.title, ts, title_end);
    if end < title_end {
        c.put(end, y, " ", ts);
    }
    let btn = |c: &mut Canvas, bx: i32, label: &str, part: Part| {
        let s = if pressed == Some(part) { ts.add_modifier(Modifier::REVERSED) } else { ts };
        c.text(bx, y, label, s);
    };
    btn(c, r - 2, " × ", Part::Close);
    if !dialog {
        c.put(r - 3, y, " ", ts);
        if w.app.resizable() {
            btn(c, r - 6, if w.max { " ❐ " } else { " □ " }, Part::Max);
        }
        btn(c, if w.app.resizable() { r - 9 } else { r - 6 }, " _ ", Part::Min);
    }
    if w.has_bar {
        let menus = w.app.menubar();
        c.fill(x + 1, y + 1, ww - 2, 1, Style::new().bg(th.client));
        for (i, ((bx, bw), (label, _))) in bar_layout(&menus).into_iter().zip(menus.iter()).enumerate() {
            if x + 1 + bx + bw > x + ww - 1 {
                break;
            }
            let s = if open_bar == Some(i) { th.sel() } else { st(th.text, th.client) };
            c.fill(x + 1 + bx, y + 1, bw, 1, s);
            let mut chars = label.chars();
            if let Some(first) = chars.next() {
                c.put_c(x + 2 + bx, y + 1, first, s.add_modifier(Modifier::UNDERLINED));
                c.text(x + 3 + bx, y + 1, chars.as_str(), s);
            }
        }
    }
}

/// A desktop shortcut to a file or folder, opened by what opens that kind of file.
fn shortcut(p: PathBuf, x: i32, y: i32) -> DeskIcon {
    DeskIcon {
        key: format!("@{}", p.display()),
        icon: crate::assoc::icon(&p),
        label: crate::assoc::label(&p),
        launch: crate::assoc::launch(&p),
        x,
        y,
        sel: false,
        orig: (x, y),
        was_sel: false,
    }
}

/// A dark outline hugging an icon's pixels, and a patch behind its label, so
/// it shows up on any picture. Works in half-cell pixels like the art does.
fn shade_icon(c: &mut Canvas, th: &Theme, ic: &DeskIcon, x: i32, y: i32) {
    let mut px = std::collections::HashSet::new();
    let cell = |cx: i32, cy: i32, px: &mut std::collections::HashSet<(i32, i32)>| {
        px.insert((cx, 2 * cy));
        px.insert((cx, 2 * cy + 1));
    };
    let pal = palette(th);
    for (r, row) in ic.icon.art().iter().enumerate() {
        for (i, ch) in row.chars().enumerate() {
            if pal(ch).is_some() {
                px.insert((x + 2 + i as i32, 2 * y + r as i32));
            }
        }
    }
    let labels = wrap_label(&ic.label);
    let bw = 12;
    for (j, line) in labels.iter().enumerate() {
        let lw = (line.chars().count() as i32).min(bw);
        let lx = x + (bw - lw) / 2;
        for cx in lx..lx + lw {
            cell(cx, y + 3 + j as i32, &mut px);
        }
    }
    let (r, g, b) = th.desk_rgb;
    let col = ratatui::style::Color::Rgb(r, g, b);
    let grown: std::collections::HashSet<(i32, i32)> = px.iter().flat_map(|&(a, b)| (-1..=1).flat_map(move |dx| (-1..=1).map(move |dy| (a + dx, b + dy)))).collect();
    for (a, b) in grown {
        c.half(a, b.div_euclid(2), b.rem_euclid(2) == 0, col);
    }
}

/// The box between two corners: x, y, width and height.
fn band(x0: i32, y0: i32, x1: i32, y1: i32) -> (i32, i32, i32, i32) {
    (x0.min(x1), y0.min(y1), (x1 - x0).abs() + 1, (y1 - y0).abs() + 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::{MouseEvent, MouseEventKind};

    fn m(d: &mut Desktop, kind: MouseEventKind, x: i32, y: i32) {
        d.mouse(MouseEvent { kind, column: x as u16, row: y as u16, modifiers: KeyModifiers::NONE });
    }

    fn drag(d: &mut Desktop, from: (i32, i32), to: (i32, i32)) {
        let l = MouseButton::Left;
        m(d, MouseEventKind::Down(l), from.0, from.1);
        m(d, MouseEventKind::Drag(l), (from.0 + to.0) / 2, (from.1 + to.1) / 2);
        m(d, MouseEventKind::Drag(l), to.0, to.1);
        m(d, MouseEventKind::Up(l), to.0, to.1);
    }

    fn r(d: &Desktop) -> (i32, i32, i32, i32) {
        let w = d.wins.last().unwrap();
        (w.x, w.y, w.w, w.h)
    }

    #[test]
    fn typing_in_the_apps_menu_searches_it() {
        let mut d = Desktop::new(120, 40);
        let key = |d: &mut Desktop, c: KeyCode| d.key(KeyEvent::new(c, KeyModifiers::NONE));
        d.open_start();
        for c in "soli".chars() {
            key(&mut d, KeyCode::Char(c));
        }
        let shown = |d: &Desktop| d.menu.as_ref().unwrap().0.levels[0].items.iter().map(|i| i.label.clone()).collect::<Vec<_>>();
        assert_eq!(shown(&d)[2], "Solitaire");
        // Esc clears the search and brings the menu back, a second Esc closes it
        key(&mut d, KeyCode::Esc);
        assert!(shown(&d).iter().any(|l| l == "Programs"));
        key(&mut d, KeyCode::Esc);
        assert!(d.menu.is_none());
        d.open_start();
        for c in "soli".chars() {
            key(&mut d, KeyCode::Char(c));
        }
        key(&mut d, KeyCode::Enter);
        assert_eq!(d.wins.last().unwrap().app.title(), "Solitaire");
    }

    #[test]
    fn dragging_to_an_edge_snaps_and_pulling_off_restores() {
        let mut d = Desktop::new(120, 40);
        d.launch(Launch::Notepad(None));
        let (x, y, w, h) = { let w = d.wins.last().unwrap(); (w.x, w.y, w.w, w.h) };
        let sh = d.work_h();
        // far right edge: the right half
        drag(&mut d, (x + 6, y), (119, 20));
        let n = r(&d);
        assert_eq!((n.0, n.1, n.2, n.3), (60, 0, 60, sh));
        assert!(d.snap.is_none());
        // pulled back out, it has its own size again
        drag(&mut d, (n.0 + 6, 0), (40, 15));
        let n = r(&d);
        assert_eq!((n.2, n.3), (w, h));
        // far left edge: the left half; top-left corner: a quarter
        drag(&mut d, (n.0 + 6, n.1), (0, 20));
        let n = r(&d);
        assert_eq!((n.0, n.1, n.2, n.3), (0, 0, 60, sh));
        drag(&mut d, (n.0 + 6, 0), (0, 0));
        let n = r(&d);
        assert_eq!((n.0, n.1, n.2, n.3), (0, 0, 60, sh / 2));
        // top edge maximises
        drag(&mut d, (n.0 + 6, 0), (50, 0));
        assert!(d.wins.last().unwrap().max);
        // and a plain move doesn't snap
        drag(&mut d, (30, 0), (40, 10));
        let n = r(&d);
        assert!(!d.wins.last().unwrap().max && !d.wins.last().unwrap().snapped);
        assert_eq!((n.2, n.3), (w, h));
    }

    #[test]
    fn tiling_splits_the_screen_and_keeps_workspaces_apart() {
        let mut d = Desktop::new(120, 40);
        d.set_tiling(true, false);
        d.launch(Launch::Notepad(None));
        // one window fills everything under the bar
        assert_eq!(r(&d), (1, 1, 118, 39));
        d.launch(Launch::Notepad(None));
        d.launch(Launch::Notepad(None));
        // dwindle: left half, then the right half split top and bottom
        let rects: Vec<_> = d.order.iter().map(|id| d.wins.iter().find(|w| w.id == *id).map(|w| (w.x, w.y, w.w, w.h)).unwrap()).collect();
        assert_eq!(rects, vec![(1, 1, 59, 39), (61, 1, 58, 20), (61, 21, 58, 19)]);
        // send the focused one to workspace 2: the other two share the screen
        let id = d.focus.unwrap();
        d.key(KeyEvent::new(KeyCode::Char('@'), KeyModifiers::ALT | KeyModifiers::SHIFT));
        assert!(d.wins.iter().find(|w| w.id == id).unwrap().hidden);
        assert_eq!(d.wins.iter().filter(|w| !w.hidden).count(), 2);
        assert!(d.wins.iter().filter(|w| !w.hidden).all(|w| w.w == 58 || w.w == 59));
        // and Alt+2 shows it alone
        d.key(KeyEvent::new(KeyCode::Char('2'), KeyModifiers::ALT));
        assert_eq!(d.cur_ws, 1);
        assert_eq!(d.focus, Some(id));
        assert_eq!(r(&d), (1, 1, 118, 39));
        // fullscreen covers the bar too; dialogs float over the tiles
        d.key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::ALT));
        assert_eq!(r(&d), (0, 0, 120, 40));
        d.launch(Launch::Run);
        assert!(d.wins.last().unwrap().float && !d.wins.last().unwrap().tiled);
        // back to overlapping windows, nothing hidden or tiled
        d.set_tiling(false, false);
        assert!(d.wins.iter().all(|w| !w.hidden && !w.tiled));
    }

    #[test]
    fn notes_files_and_terminals_open_more_in_tabs() {
        let mut d = Desktop::new(120, 40);
        d.launch(Launch::Notepad(None));
        let id = d.focus.unwrap();
        let tabs = |d: &Desktop| {
            let bar = d.wins.last().unwrap().app.menubar();
            let file = &bar.iter().find(|(n, _)| *n == "File").unwrap().1;
            if file.iter().find(|i| i.label == "Close Tab").unwrap().enabled { 2 } else { 1 }
        };
        assert_eq!(tabs(&d), 1);
        d.exec(Cmd::App("tab-new"), Owner::Bar(id, 0));
        assert_eq!((d.wins.len(), tabs(&d)), (1, 2));
        // the strip shows along the top: clicking the first tab's × closes it
        let (cx, cy, _, _) = d.wins.last().unwrap().client();
        let shift_ctrl = KeyModifiers::CONTROL | KeyModifiers::SHIFT;
        let tw = ((d.wins.last().unwrap().client().2 - 4) / 2).clamp(6, 26);
        m(&mut d, MouseEventKind::Down(MouseButton::Left), cx + tw - 2, cy);
        assert_eq!((d.wins.len(), tabs(&d)), (1, 1));
        // Ctrl+Shift+T and W from the keyboard; closing the last tab closes the window
        d.key(KeyEvent::new(KeyCode::Char('T'), shift_ctrl));
        assert_eq!(tabs(&d), 2);
        d.key(KeyEvent::new(KeyCode::Char('W'), shift_ctrl));
        d.key(KeyEvent::new(KeyCode::Char('W'), shift_ctrl));
        assert!(d.idx(id).is_none());
    }

    #[test]
    fn the_tray_speaker_opens_a_volume_slider() {
        let mut d = Desktop::new(120, 40);
        d.vol = Some((50, false));
        let t = d.tray_x().unwrap();
        let l = MouseButton::Left;
        m(&mut d, MouseEventKind::Down(l), t + 1, 39);
        m(&mut d, MouseEventKind::Up(l), t + 1, 39);
        let (px, py) = d.vol_open.unwrap();
        // top of the slider is full volume, dragging to the bottom is silence
        m(&mut d, MouseEventKind::Down(l), px + 5, py + 2);
        assert_eq!(d.vol, Some((100, false)));
        m(&mut d, MouseEventKind::Drag(l), px + 5, py + 30);
        m(&mut d, MouseEventKind::Up(l), px + 5, py + 30);
        assert_eq!(d.vol, Some((0, false)));
        // the Mute box, the wheel and the arrow keys
        m(&mut d, MouseEventKind::Down(l), px + 2, py + 13);
        assert_eq!(d.vol, Some((0, true)));
        m(&mut d, MouseEventKind::ScrollUp, t + 1, 39);
        d.key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(d.vol, Some((10, true)));
        // clicking elsewhere closes it
        m(&mut d, MouseEventKind::Down(l), 60, 10);
        assert!(d.vol_open.is_none());
    }

    #[test]
    fn added_programs_join_the_apps_menu_and_the_desktop() {
        let dir = std::env::temp_dir().join(format!("win95-progs-{}", std::process::id()));
        unsafe { std::env::set_var("HOME", &dir) };
        let mut d = Desktop::new(120, 40);
        d.launch(Launch::AddProgram);
        let id = d.focus.unwrap();
        for c in "htop -t".chars() {
            d.key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        // paint two pixels, rub one out, paint with another colour
        let (cx, cy, _, _) = d.wins.last().unwrap().client();
        let l = MouseButton::Left;
        m(&mut d, MouseEventKind::Down(l), cx + 12, cy + 7);
        m(&mut d, MouseEventKind::Drag(l), cx + 14, cy + 7);
        m(&mut d, MouseEventKind::Up(l), cx + 14, cy + 7);
        m(&mut d, MouseEventKind::Down(MouseButton::Right), cx + 14, cy + 7);
        m(&mut d, MouseEventKind::Up(MouseButton::Right), cx + 14, cy + 7);
        m(&mut d, MouseEventKind::Down(l), cx + 33, cy + 7);
        m(&mut d, MouseEventKind::Up(l), cx + 33, cy + 7);
        m(&mut d, MouseEventKind::Down(l), cx + 12, cy + 8);
        m(&mut d, MouseEventKind::Up(l), cx + 12, cy + 8);
        d.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(d.idx(id).is_none(), "the dialog closed");
        assert_eq!(crate::programs::load(), vec![("Htop".into(), "htop -t".into())]);
        let icon = crate::programs::icon("Htop").unwrap();
        assert_eq!(&icon[..2], &["y.......".to_string(), "f.......".to_string()]);
        // in Apps > Programs, not on the Apps menu itself, and on the desktop
        let top = d.start_items();
        assert!(!top.iter().any(|i| i.label == "Htop"));
        let progs = &top.iter().find(|i| i.label == "Programs").unwrap().sub;
        assert!(progs.iter().any(|i| i.label == "Htop"));
        assert!(d.icons.iter().any(|i| i.label == "Htop" && matches!(i.icon, Icon::Custom(_))));
        // drag an icon somewhere else: it snaps to the grid and stays there
        let (x, y) = d.icon_pos(0);
        let name = d.icons[0].label.clone();
        drag(&mut d, (x + 3, y + 1), (70, 20));
        let ic = d.icons.iter().find(|i| i.label == name).unwrap();
        assert_eq!((ic.x, ic.y), (66, 19));
        d.refresh_programs();
        let ic = d.icons.iter().find(|i| i.label == name).unwrap();
        assert_eq!((ic.x, ic.y), (66, 19));
        // dropped on another icon, it takes the nearest free place instead
        let (ox, oy) = d.icon_pos(0);
        let other = d.icons[0].label.clone();
        drag(&mut d, (68, 20), (ox + 2, oy + 1));
        let (nx, ny) = d.icons.iter().find(|i| i.label == name).map(|i| (i.x, i.y)).unwrap();
        assert_ne!((nx, ny), (ox, oy));
        assert_eq!(((nx - 1) % 13, (ny - 1) % 6), (0, 0));
        assert_eq!(d.icons.iter().find(|i| i.label == other).map(|i| (i.x, i.y)), Some((ox, oy)));
        drag(&mut d, (nx + 3, ny + 1), (70, 20));
        // take it off the desktop with Delete, put it back from the desktop menu
        m(&mut d, MouseEventKind::Down(l), 68, 20);
        m(&mut d, MouseEventKind::Up(l), 68, 20);
        d.key(KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE));
        assert!(!d.icons.iter().any(|i| i.label == name));
        d.refresh_programs();
        assert!(!d.icons.iter().any(|i| i.label == name));
        d.open_desk_menu(90, 30);
        d.menu = None;
        d.exec(Cmd::AddIcon(name.clone()), Owner::Desk);
        assert!(d.icons.iter().any(|i| i.label == name && (i.x, i.y) == (79, 31)));
        // Arrange by name lines them all up in columns again, Trash last
        d.exec(Cmd::Desk("arrange-name"), Owner::Desk);
        assert_eq!((d.icons[0].x, d.icons[0].y), d.icon_slot(0));
        assert_eq!(d.icons.last().unwrap().label, "Trash");
        let mut names: Vec<String> = d.icons.iter().map(|i| i.label.to_lowercase()).collect();
        names.pop();
        assert!(names.windows(2).all(|w| w[0] <= w[1]));
        // drag a box over the first two to select them, then move both at once
        let (a, b2) = (d.icons[0].label.clone(), d.icons[1].label.clone());
        let pos = |d: &Desktop, n: &str| d.icons.iter().find(|i| i.label == n).map(|i| (i.x, i.y)).unwrap();
        let rest: Vec<_> = d.icons[2..].iter().map(|i| (i.label.clone(), i.x, i.y)).collect();
        drag(&mut d, (0, 0), (12, 7));
        assert_eq!(d.icons.iter().filter(|i| i.sel).count(), 2);
        drag(&mut d, (3, 2), (51, 12));
        assert_eq!((pos(&d, &a), pos(&d, &b2)), ((53, 13), (53, 19)));
        assert!(rest.iter().all(|(n, x, y)| pos(&d, n) == (*x, *y)));
        // a plain click on one of them picks just that one
        m(&mut d, MouseEventKind::Down(l), 55, 14);
        m(&mut d, MouseEventKind::Up(l), 55, 14);
        assert_eq!(d.icons.iter().filter(|i| i.sel).map(|i| i.label.clone()).collect::<Vec<_>>(), vec![a.clone()]);
        // Ctrl+click adds another, and Delete takes both off
        d.mouse(MouseEvent { kind: MouseEventKind::Down(l), column: 55, row: 20, modifiers: KeyModifiers::CONTROL });
        d.mouse(MouseEvent { kind: MouseEventKind::Up(l), column: 55, row: 20, modifiers: KeyModifiers::CONTROL });
        assert_eq!(d.icons.iter().filter(|i| i.sel).count(), 2);
        d.key(KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE));
        assert!(!d.icons.iter().any(|i| i.label == a || i.label == b2));
        // Files: right-click opens a menu, and Create Shortcut puts the file on
        // the desktop, opening in Notes or Paint by its kind
        let docs = dir.join("docs");
        std::fs::create_dir_all(&docs).unwrap();
        std::fs::write(docs.join("a.txt"), "hello\n").unwrap();
        image::RgbImage::new(4, 4).save_with_format(docs.join("b.bmp"), image::ImageFormat::Bmp).unwrap();
        d.launch(Launch::Explorer(docs.clone()));
        let id = d.focus.unwrap();
        let (cx, cy, _, _) = d.wins.last().unwrap().client();
        m(&mut d, MouseEventKind::Down(MouseButton::Right), cx + 4, cy + 2);
        assert!(matches!(&d.menu, Some((mm, _)) if mm.owner == Owner::Ctx(id)));
        d.menu = None;
        for name in ["a", "b"] {
            d.key(KeyEvent::new(KeyCode::Char(name.chars().next().unwrap()), KeyModifiers::NONE));
            d.exec(Cmd::App("shortcut"), Owner::Ctx(id));
        }
        let a_ic = d.icons.iter().find(|i| i.label == "a.txt").unwrap();
        assert!(matches!(&a_ic.launch, Launch::Notepad(Some(p)) if *p == docs.join("a.txt")));
        let b_ic = d.icons.iter().find(|i| i.label == "b.bmp").unwrap();
        assert!(matches!(&b_ic.launch, Launch::Paint(Some(_))) && b_ic.icon == Icon::Picture);
        // and they're still there after a reload
        d.refresh_programs();
        assert!(d.icons.iter().any(|i| i.label == "b.bmp"));
        // the same command again replaces it; Remove takes it off both
        assert_eq!(crate::programs::add("htop", None), "Htop");
        assert_eq!(crate::programs::load().len(), 1);
        assert!(crate::programs::icon("Htop").is_none());
        d.exec(Cmd::Forget("Htop".into()), Owner::Start);
        assert!(crate::programs::load().is_empty());
        assert!(!d.icons.iter().any(|i| i.label == "Htop"));
        assert_eq!(crate::programs::name_of("/usr/bin/scribe-tui --x"), "Scribe");
        assert_eq!(crate::programs::missing("surely-not-a-real-program --x").as_deref(), Some("surely-not-a-real-program"));
        assert_eq!(crate::programs::missing("sh -c true"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

