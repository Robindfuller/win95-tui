pub mod dialogs;
pub mod explorer;
pub mod mines;
pub mod notepad;
pub mod term;

use crate::{draw::Canvas, icons::Icon, menu::Item, theme::Theme};
use crossterm::event::{KeyEvent, KeyModifiers, MouseEventKind};
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub enum Launch {
    Shell { cmd: Option<String>, cwd: Option<PathBuf>, title: String, icon: Icon, keep_open: bool },
    Notepad(Option<PathBuf>),
    Mines,
    Explorer(PathBuf),
    Run,
    About,
    ShutDown,
    TaskList,
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
}
