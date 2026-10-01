// The window manager: desktop icons, overlapping windows, taskbar and menus.
use crate::{
    apps::{
        dialogs::{about_text, AddProgram, MsgBox, Run, ShutDown, TaskList},
        explorer::Explorer,
        mines::Mines,
        notepad::Notepad,
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
        !self.min && x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
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
        let dialog = self.app.dialog();
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
            return Some(if self.app.resizable() && !self.max { Part::Edge(left, right, bottom) } else { Part::Frame });
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
}

enum TaskHit {
    Start,
    Win(u64),
    Clock,
}

struct DeskIcon {
    icon: Icon,
    label: &'static str,
    launch: Launch,
}

pub struct Desktop {
    pub th: Theme,
    lite: bool,
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
    last_click: Option<(Instant, i32, i32)>,
    kbmode: Option<(u64, bool)>,
    pub quit: bool,
    docs: Vec<PathBuf>,
    clock: String,
}

/// A text web browser to open in a window, if one is installed.
fn browser() -> Option<String> {
    const START: &str = "https://lite.duckduckgo.com/lite/";
    ["lynx", "w3m"].iter().find(|b| has(b)).map(|b| format!("{b} {START}"))
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
Double-click title  Maximise\n\
Alt+L               Lite or Classic look\n\n\
Colours follow your Omarchy theme as it changes.";

const FIND: &str = r#"printf 'Named: '; read -r q; [ -n "" ] && find ~ -iname "**" -not -path '*/.*' 2>/dev/null | sed "s|^/home/rdf|~|" | head -500"#;

impl Desktop {
    pub fn new(w: u16, h: u16, lite: bool) -> Desktop {
        let mut icons = vec![
            DeskIcon { icon: Icon::Computer, label: "Computer", launch: Launch::Explorer(PathBuf::from("/")) },
            DeskIcon { icon: Icon::Folder, label: "Home", launch: Launch::Explorer(home()) },
            DeskIcon { icon: Icon::Terminal, label: "Terminal", launch: Launch::shell("Terminal", Icon::Terminal, None) },
            DeskIcon { icon: Icon::Notepad, label: "Notes", launch: Launch::Notepad(None) },
            DeskIcon { icon: Icon::Mines, label: "Mines", launch: Launch::Mines },
        ];
        if has("btop") {
            icons.push(DeskIcon { icon: Icon::Monitor, label: "Monitor", launch: Launch::shell("Monitor", Icon::Monitor, Some("btop")) });
        }
        if let Some(b) = browser() {
            icons.push(DeskIcon { icon: Icon::Web, label: "Web", launch: Launch::shell("Web", Icon::Web, Some(&b)) });
        }
        if has("lazygit") {
            icons.push(DeskIcon { icon: Icon::Git, label: "Git", launch: Launch::shell("Git", Icon::Git, Some("lazygit")) });
        }
        icons.push(DeskIcon { icon: Icon::Recycle, label: "Trash", launch: Launch::Explorer(home().join(".local/share/Trash/files")) });
        Desktop {
            th: Theme::load(lite),
            lite,
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
            icons,
            sel_icon: None,
            last_click: None,
            kbmode: None,
            quit: false,
            docs: vec![],
            clock: clock(),
        }
    }

    fn work_h(&self) -> i32 {
        if self.lite { self.h - 2 } else { self.h - 1 }
    }

    fn idx(&self, id: u64) -> Option<usize> {
        self.wins.iter().position(|w| w.id == id)
    }

    fn win(&mut self, id: u64) -> Option<&mut Win> {
        self.wins.iter_mut().find(|w| w.id == id)
    }

    fn top_visible(&self) -> Option<u64> {
        self.wins.iter().rev().find(|w| !w.min).map(|w| w.id)
    }

    // ------------------------------------------------------------ window ops

    fn focus_win(&mut self, id: u64) {
        if let Some(i) = self.idx(id) {
            let mut w = self.wins.remove(i);
            w.min = false;
            self.wins.push(w);
            self.focus = Some(id);
            self.sel_icon = None;
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
        }
    }

    pub fn launch(&mut self, l: Launch) {
        let single = match &l {
            Launch::Run => Some("Run"),
            Launch::AddProgram => Some("Add Program"),
            Launch::ShutDown => Some("Quit"),
            Launch::TaskList => Some("Tasks"),
            _ => None,
        };
        if let Some(t) = single {
            if let Some(id) = self.wins.iter().find(|w| w.title == t).map(|w| w.id) {
                self.focus_win(id);
                return;
            }
        }
        let app: Box<dyn App> = match l {
            Launch::Shell { cmd, cwd, title, icon, keep_open } => match TermApp::new(cmd.as_deref(), cwd, &title, icon, keep_open, &self.th) {
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
                Box::new(Notepad::new(p))
            }
            Launch::Mines => Box::new(Mines::new()),
            Launch::Explorer(p) => Box::new(Explorer::new(p)),
            Launch::Run => Box::new(Run::new()),
            Launch::AddProgram => Box::new(AddProgram::new()),
            Launch::About => Box::new(MsgBox::new("About".into(), about_text(&self.th))),
            Launch::ShutDown => Box::new(ShutDown::new()),
            Launch::TaskList => Box::new(TaskList::new(self.wins.iter().filter(|w| !w.app.dialog()).map(|w| (w.id, w.title.clone())).collect())),
            Launch::Msg { title, text } => Box::new(MsgBox::new(title, text)),
        };
        self.add(app);
    }

    fn add(&mut self, app: Box<dyn App>) {
        let has_bar = !app.menubar().is_empty();
        let (cw, ch) = app.size_hint();
        let (sw, sh) = (self.w, self.work_h());
        let w = (cw as i32 + 2).min(sw);
        let h = (ch as i32 + 2 + has_bar as i32).min(sh);
        let (x, y) = if app.dialog() {
            ((sw - w) / 2, (sh - h) / 2)
        } else {
            let n = (self.wins.iter().filter(|w| !w.app.dialog()).count() % 8) as i32;
            let x = (if self.lite { 20 } else { 14 } + n * 3).min((sw - w).max(0));
            let y = (1 + n).min((sh - h).max(0));
            (x, y)
        };
        let id = self.next_id;
        self.next_id += 1;
        let title = app.title();
        let big = app.resizable() && (cw as i32 + 2 > sw || ch as i32 + 2 + has_bar as i32 > sh);
        let mut win = Win { id, x, y, w, h, restore: None, max: false, snapped: false, min: false, app, has_bar, buf: Buffer::empty(Rect::new(0, 0, 1, 1)), title };
        win.sync();
        self.wins.push(win);
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
            Item::new("Mines", Cmd::Launch(Launch::Mines)).icon(Icon::Mines),
        ];
        if has("btop") {
            progs.push(Item::new("Monitor", shell("Monitor", Icon::Monitor, Some("btop"))).icon(Icon::Monitor));
        }
        if let Some(b) = browser() {
            progs.push(Item::new("Web", shell("Web", Icon::Web, Some(&b))).icon(Icon::Web));
        }
        if has("lazygit") {
            progs.push(Item::new("Git", shell("Git", Icon::Git, Some("lazygit"))).icon(Icon::Git));
        }
        progs.push(Item::new("Terminal", shell("Terminal", Icon::Terminal, None)).icon(Icon::Terminal));
        if has("nvim") {
            progs.push(Item::new("Vim", shell("Vim", Icon::Vim, Some("nvim"))).icon(Icon::Vim));
        }
        progs.push(Item::new("Files", Cmd::Launch(Launch::Explorer(home()))).icon(Icon::Folder));
        // the ones you added, then the way to add or take one off
        let mine = crate::programs::load();
        if !mine.is_empty() {
            progs.push(Item::sep());
            for (name, cmd) in &mine {
                progs.push(Item::new(name.clone(), shell(name, Icon::Run, Some(cmd))).icon(Icon::Run));
            }
        }
        progs.push(Item::sep());
        progs.push(Item::new("Add Program...", Cmd::Launch(Launch::AddProgram)).icon(Icon::Programs));
        if !mine.is_empty() {
            let forget = mine.iter().map(|(n, _)| Item::new(n.clone(), Cmd::Forget(n.clone())).icon(Icon::Run)).collect();
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
        vec![
            Item::new("Terminal", shell("Terminal", Icon::Terminal, None)).icon(Icon::Terminal),
            Item::sep(),
            Item::sub("Programs", progs).icon(Icon::Programs),
            Item::sub("Recent", docs).icon(Icon::Documents),
            Item::sub("Settings", vec![
                Item::new("Display", Cmd::Desk("props")).icon(Icon::Computer),
                Item::new("Reload Theme", Cmd::Desk("theme")).icon(Icon::Settings),
                Item::sep(),
                Item::new("Lite Look", Cmd::Desk("lite")).checked(self.lite).key("Alt+L"),
                Item::new("Classic Look", Cmd::Desk("classic")).checked(!self.lite),
            ])
            .icon(Icon::Settings),
            Item::sub("Find", vec![Item::new("Files or Folders...", Cmd::Desk("find")).icon(Icon::Folder)]).icon(Icon::Help),
            Item::new("Help", Cmd::Desk("help")).icon(Icon::Help),
            Item::new("Run...", Cmd::Launch(Launch::Run)).icon(Icon::Run),
            Item::sep(),
            Item::new("Quit", Cmd::Launch(Launch::ShutDown)).icon(Icon::Shutdown),
        ]
    }

    fn open_menu(&mut self, owner: Owner, items: Vec<Item>, x: i32, y: i32, banner: bool) {
        let mut l = Level::new(items, x, y, banner);
        if l.x + l.width() > self.w {
            l.x = (self.w - l.width()).max(0);
        }
        if l.y + l.height() > self.work_h() {
            l.y = (self.work_h() - l.height()).max(0);
        }
        self.menu = Some((MenuState { owner, levels: vec![l] }, Instant::now()));
    }

    fn open_start(&mut self) {
        let items = self.start_items();
        let h = items.len() as i32 + 2;
        self.open_menu(Owner::Start, items, 0, self.work_h() - h, false);
    }

    fn open_sys(&mut self, id: u64) {
        let Some(w) = self.wins.iter().find(|w| w.id == id) else { return };
        let (res, dialog) = (w.app.resizable(), w.app.dialog());
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
        let items = vec![
            Item::new("Terminal Here", Cmd::Launch(Launch::shell("Terminal", Icon::Terminal, None))).icon(Icon::Terminal),
            Item::new("New Text Document", Cmd::Launch(Launch::Notepad(None))).icon(Icon::Notepad),
            Item::sep(),
            Item::new("Refresh", Cmd::Desk("theme")),
            Item::new("Properties", Cmd::Desk("props")),
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
            Owner::Sys(id) | Owner::Bar(id, _) => Some(id),
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
            Cmd::Forget(name) => crate::programs::remove(&name),
            Cmd::Desk(s) => match s {
                "theme" => self.reload_theme(),
                "lite" => self.set_lite(true),
                "classic" => self.set_lite(false),
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
    }

    fn cycle(&mut self) {
        if self.wins.len() > 1 {
            let id = self.wins[0].id;
            self.focus_win(id);
        } else if let Some(w) = self.wins.first() {
            let id = w.id;
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
        if self.menu.is_some() {
            self.menu_key(k);
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
            KeyCode::Char('l') if alt && !ctrl => return self.set_lite(!self.lite),
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
        match k.code {
            KeyCode::Down | KeyCode::Right | KeyCode::Tab => self.sel_icon = Some(self.sel_icon.map(|i| (i + 1) % n).unwrap_or(0)),
            KeyCode::Up | KeyCode::Left | KeyCode::BackTab => self.sel_icon = Some(self.sel_icon.map(|i| (i + n - 1) % n).unwrap_or(0)),
            KeyCode::Enter => {
                if let Some(i) = self.sel_icon {
                    let l = self.icons[i].launch.clone();
                    self.launch(l);
                }
            }
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

    fn icon_slot(&self, i: usize) -> (i32, i32) {
        if self.lite {
            let per = ((self.work_h() - 1) / 5).max(1) as usize;
            return (1 + (i / per) as i32 * 12, 1 + (i % per) as i32 * 5);
        }
        let per = ((self.work_h() - 1) / 6).max(1) as usize;
        (1 + (i / per) as i32 * 13, 1 + (i % per) as i32 * 6)
    }

    fn icon_at(&self, x: i32, y: i32) -> Option<usize> {
        (0..self.icons.len()).find(|&i| {
            let (ix, iy) = self.icon_slot(i);
            let (w, h) = if self.lite { (11, 4) } else { (12, 5) };
            x >= ix && x < ix + w && y >= iy && y < iy + h
        })
    }

    fn taskbar_layout(&self) -> (Vec<(u64, i32, i32)>, i32) {
        let clock_x = self.w - 9;
        let x0 = 11;
        let mut ids: Vec<&Win> = self.wins.iter().collect();
        ids.sort_by_key(|w| w.id);
        let n = ids.len().max(1) as i32;
        let bw = ((clock_x - 1 - x0) / n - 1).clamp(4, 24);
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
        if let Some((m, _)) = &self.menu {
            match m.hit(x, y) {
                Some((lvl, Some(i))) => return self.activate(lvl, i),
                Some((_, None)) => return,
                None => {
                    let owner = m.owner;
                    self.menu = None;
                    if owner == Owner::Start && y == self.h - 1 && x < 10 {
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
        if y == self.h - 1 {
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
        match self.icon_at(x, y) {
            Some(i) => {
                self.sel_icon = Some(i);
                if b == MouseButton::Left && self.double(x, y) {
                    let l = self.icons[i].launch.clone();
                    self.launch(l);
                }
            }
            None => {
                self.sel_icon = None;
                if b == MouseButton::Right {
                    self.open_desk_menu(x, y);
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
        let (sw, sh) = (self.w, self.work_h());
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
                    w.y = (y - dy).clamp(0, sh - 1);
                }
                let can = self.wins.iter().find(|w| w.id == id).is_some_and(|w| w.app.resizable());
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
            Drag::Start | Drag::None => self.menu_hover(x, y),
            _ => {}
        }
    }

    // ------------------------------------------------------------ tick

    fn set_lite(&mut self, lite: bool) {
        self.lite = lite;
        theme::save_lite(lite);
        self.reload_theme();
        let (w, h) = (self.w as u16, self.h as u16);
        self.resize(w, h);
    }

    fn reload_theme(&mut self) {
        self.th = Theme::load(self.lite);
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
        let c = clock();
        if c != self.clock {
            self.clock = c;
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

    // ------------------------------------------------------------ drawing

    pub fn render(&mut self, f: &mut Frame) {
        let th = self.th.clone();
        let mut c = Canvas::new(f.buffer_mut());
        c.fill(0, 0, self.w, self.h, st(th.text, th.desk));
        let pal = palette(&th);
        for (i, ic) in self.icons.iter().enumerate() {
            let (x, y) = self.icon_slot(i);
            if th.lite {
                let on = self.sel_icon == Some(i);
                let (_, col) = ic.icon.glyph(&th);
                let art = Style::new().fg(if on { th.accent } else { col });
                for (j, line) in ic.icon.lines().iter().enumerate() {
                    c.text(x + 2, y + j as i32, line, art);
                }
                let s = if on { th.sel() } else { Style::new().fg(th.text) };
                let lw = ic.label.chars().count() as i32;
                c.text_max(x + (11 - lw) / 2, y + 3, ic.label, s, x + 11);
                continue;
            }
            c.pixels(x + 2, y, ic.icon.art(), &pal);
            let on = self.sel_icon == Some(i);
            let s = if on { st(th.on_accent, th.accent) } else { st(th.text, th.desk) };
            for (j, line) in wrap_label(ic.label).iter().enumerate() {
                let lw = line.chars().count() as i32;
                c.text_max(x + (12 - lw) / 2, y + 3 + j as i32, line, s, x + 12);
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
        for w in &mut self.wins {
            if w.min {
                continue;
            }
            let focused = self.focus == Some(w.id);
            draw_window(&mut c, &th, w, focused, pressed.filter(|p| p.0 == w.id).map(|p| p.1), open_bar.filter(|b| b.0 == w.id).map(|b| b.1));
        }
        // Where the window will land if it is let go here.
        if let (Some(s), Drag::Move { .. }) = (self.snap, &self.drag) {
            let (x, y, w, h) = self.snap_rect(s);
            c.frame(x, y, w, h, Style::new().fg(th.accent));
        }
        self.render_taskbar(&mut c, &th);
        if let Some((m, _)) = &self.menu {
            m.render(&mut c, &th);
        }
        if let Some((id, size)) = self.kbmode {
            let msg = if size { " Size: arrows resize, Shift for bigger steps, Enter when done " } else { " Move: arrows move, Shift for bigger steps, Enter when done " };
            let w = msg.chars().count() as i32;
            c.text((self.w - w) / 2, self.h - 2, msg, st(th.on_accent, th.accent));
            let _ = id;
        }
        // Text cursor for the focused app.
        if self.menu.is_none() && self.kbmode.is_none() {
            if let Some(w) = self.focus.and_then(|id| self.wins.iter().find(|w| w.id == id)) {
                if let Some((cx, cy)) = w.app.cursor() {
                    let (ox, oy, cw, ch) = w.client();
                    let (x, y) = (ox + cx as i32, oy + cy as i32);
                    if (cx as i32) < cw && (cy as i32) < ch && x >= 0 && y >= 0 && x < self.w && y < self.h - 1 {
                        f.set_cursor_position(Position::new(x as u16, y as u16));
                    }
                }
            }
        }
    }

    fn render_taskbar(&self, c: &mut Canvas, th: &Theme) {
        if th.lite {
            return self.render_taskbar_lite(c, th);
        }
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
        c.put(clock_x, y, "▏", st(th.shadow, th.face));
        c.text(clock_x + 1, y, &format!("  {}  ", self.clock), s);
        c.put(self.w - 1, y, "▕", st(th.hilite, th.face));
    }

    fn render_taskbar_lite(&self, c: &mut Canvas, th: &Theme) {
        let y = self.h - 1;
        let line = Style::new().fg(th.dim);
        c.fill(0, y - 1, self.w, 2, Style::new());
        for x in 0..self.w {
            c.put(x, y - 1, "─", line);
        }
        let start_open = matches!(&self.menu, Some((m, _)) if m.owner == Owner::Start);
        let s = if start_open { th.sel() } else { Style::new().fg(th.accent).add_modifier(Modifier::BOLD) };
        c.fill(0, y, 10, 1, s);
        c.text(2, y, "❖ Apps", s);
        c.put(10, y, "│", line);
        let (v, clock_x) = self.taskbar_layout();
        for (id, bx, bw) in v {
            let Some(w) = self.wins.iter().find(|w| w.id == id) else { continue };
            let active = self.focus == Some(id) && !w.min;
            let s = if active { th.sel() } else if w.min { Style::new().fg(th.dim) } else { Style::new().fg(th.text) };
            c.fill(bx, y, bw, 1, s);
            let (g, col) = w.app.icon().glyph(th);
            c.put_c(bx + 1, y, g, if active { s } else { Style::new().fg(col) });
            c.text_max(bx + 3, y, &w.title, s, bx + bw - 1);
            c.put(bx + bw, y, "│", line);
        }
        c.put(clock_x, y, "│", line);
        c.text(clock_x + 2, y, &self.clock, Style::new().fg(th.text));
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

fn draw_window(c: &mut Canvas, th: &Theme, w: &mut Win, focused: bool, pressed: Option<Part>, open_bar: Option<usize>) {
    let (cx, cy, _, _) = w.client();
    w.buf.reset();
    {
        let mut wc = Canvas::new(&mut w.buf);
        w.app.render(&mut wc, th, focused);
    }
    c.blit(&w.buf, cx, cy);
    if th.lite {
        return draw_frame_lite(c, th, w, focused, pressed, open_bar);
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

fn draw_frame_lite(c: &mut Canvas, th: &Theme, w: &Win, focused: bool, pressed: Option<Part>, open_bar: Option<usize>) {
    let (x, y, ww, hh) = (w.x, w.y, w.w, w.h);
    let line = Style::new().fg(if focused { th.accent } else { th.dim });
    c.frame(x, y, ww, hh, line);
    let ts = if focused { Style::new().fg(th.accent).add_modifier(Modifier::BOLD) } else { Style::new().fg(th.dim) };
    let r = w.buttons_x();
    let dialog = w.app.dialog();
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
        c.fill(x + 1, y + 1, ww - 2, 1, Style::new());
        for (i, ((bx, bw), (label, _))) in bar_layout(&menus).into_iter().zip(menus.iter()).enumerate() {
            if x + 1 + bx + bw > x + ww - 1 {
                break;
            }
            let s = if open_bar == Some(i) { th.sel() } else { Style::new().fg(th.text) };
            c.fill(x + 1 + bx, y + 1, bw, 1, s);
            let mut chars = label.chars();
            if let Some(first) = chars.next() {
                c.put_c(x + 2 + bx, y + 1, first, s.add_modifier(Modifier::UNDERLINED));
                c.text(x + 3 + bx, y + 1, chars.as_str(), s);
            }
        }
    }
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
    fn dragging_to_an_edge_snaps_and_pulling_off_restores() {
        let mut d = Desktop::new(120, 40, true);
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
    fn added_programs_join_the_apps_menu() {
        let dir = std::env::temp_dir().join(format!("win95-progs-{}", std::process::id()));
        unsafe { std::env::set_var("HOME", &dir) };
        crate::programs::add("Web", "lynx https://example.com");
        crate::programs::add("Top", "htop");
        crate::programs::add("web", "w3m https://example.com");
        assert_eq!(crate::programs::load(), vec![("Top".into(), "htop".into()), ("web".into(), "w3m https://example.com".into())]);
        let d = Desktop::new(120, 40, true);
        let progs = d.start_items().into_iter().find(|i| i.label == "Programs").unwrap().sub;
        let labels: Vec<String> = progs.iter().map(|i| i.label.clone()).collect();
        assert!(labels.contains(&"Top".to_string()) && labels.contains(&"Add Program...".to_string()), "{labels:?}");
        let mut d = d;
        d.exec(Cmd::Forget("Top".into()), Owner::Start);
        assert_eq!(crate::programs::load().len(), 1);
        assert_eq!(crate::programs::missing("surely-not-a-real-program --x").as_deref(), Some("surely-not-a-real-program"));
        assert_eq!(crate::programs::missing("sh -c true"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

