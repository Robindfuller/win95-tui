pub mod amp;
pub mod background;
pub mod browser;
pub mod dialogs;
pub mod doom;
pub mod explorer;
pub mod launcher;
pub mod mines;
pub mod mplayer;
pub mod notepad;
pub mod paint;
pub mod solitaire;
pub mod tabs;
pub mod term;

use crate::{draw::Canvas, icons::Icon, menu::Item, theme::Theme};
use crossterm::event::{KeyEvent, KeyModifiers, MouseEventKind};
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub enum Launch {
    Shell { cmd: Option<String>, cwd: Option<PathBuf>, title: String, icon: Icon, keep_open: bool },
    Notepad(Option<PathBuf>),
    Mines,
    /// the music player, with files to add and play
    Amp(Vec<PathBuf>),
    Solitaire,
    Doom,
    /// the video player, with a file to play
    MediaPlayer(Option<PathBuf>),
    /// the web browser, at this address or its home page
    Browser(Option<String>),
    Paint(Option<PathBuf>),
    /// a file nothing in here opens, handed to the system (xdg-open)
    External(PathBuf),
    Explorer(PathBuf),
    Run,
    AddProgram,
    Background,
    About,
    ShutDown,
    TaskList,
    /// the tiling desktop's app launcher
    Launcher,
    Msg { title: String, text: String },
}

impl Launch {
    pub fn shell(title: &str, icon: Icon, cmd: Option<&str>) -> Launch {
        Launch::Shell { cmd: cmd.map(Into::into), cwd: None, title: title.into(), icon, keep_open: false }
    }
}

#[derive(Debug)]
pub enum Action {
    None,
    Close,
    CloseWin(u64),
    Launch(Launch),
    Resize(u16, u16),
    Quit,
    /// a right-click menu, at x, y in the window
    Menu(Vec<Item>, i32, i32),
    /// open this in a new tab of the same window
    OpenTab(Launch),
    /// put shortcuts to these on the desktop
    Shortcut(Vec<PathBuf>),
    /// leave the desktop running in the background and close this terminal's view of it
    Detach,
    /// the programs you added changed: rebuild the menu and desktop
    Refresh,
    /// the desktop background settings changed
    Background,
    Many(Vec<Action>),
}

pub trait App {
    fn title(&self) -> String;
    fn icon(&self) -> Icon;
    /// Client size the app would like when it opens.
    fn size_hint(&self) -> (u16, u16);
    fn render(&mut self, c: &mut Canvas, th: &Theme, focused: bool);
    fn cursor(&self) -> Option<(u16, u16)> {
        None
    }
    fn key(&mut self, _k: KeyEvent) -> Action {
        Action::None
    }
    fn mouse(&mut self, _kind: MouseEventKind, _x: i32, _y: i32, _mods: KeyModifiers) -> Action {
        Action::None
    }
    fn paste(&mut self, _s: &str) {}
    fn resize(&mut self, _w: u16, _h: u16) {}
    /// Called every loop. Returns whether the app needs a redraw.
    fn poll(&mut self) -> (bool, Action) {
        (false, Action::None)
    }
    fn menubar(&self) -> Vec<(&'static str, Vec<Item>)> {
        vec![]
    }
    fn command(&mut self, _cmd: &str) -> Action {
        Action::None
    }
    fn resizable(&self) -> bool {
        true
    }
    fn dialog(&self) -> bool {
        false
    }
    fn theme_changed(&mut self, _th: &Theme) {}
    /// A short name for its tab.
    fn tab_title(&self) -> String {
        self.title()
    }
    /// What a new tab of this window opens, for windows that have tabs.
    fn new_tab(&self) -> Option<Launch> {
        None
    }
    /// More files for a window that's already open, like the music player.
    fn open_more(&mut self, _files: &[PathBuf]) {}
    /// Wants key release events (and lone Ctrl, Shift and Alt) while focused.
    fn key_releases(&self) -> bool {
        false
    }
    /// It had key releases on and lost the focus, so keys held may never be let go.
    fn focus_lost(&mut self) {}
}
