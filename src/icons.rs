// Small glyph icons for title bars and menus, and 8x6 pixel art for the desktop.
use crate::theme::Theme;
use ratatui::style::Color;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Icon {
    Computer,
    Folder,
    File,
    Terminal,
    Notepad,
    Mines,
    Monitor,
    Git,
    Vim,
    Recycle,
    Run,
    Help,
    Info,
    Shutdown,
    Settings,
    Programs,
    Documents,
}

impl Icon {
    pub fn glyph(self, th: &Theme) -> (char, Color) {
        match self {
            Icon::Computer => ('▣', th.accent),
            Icon::Folder | Icon::Programs => ('■', th.yellow),
            Icon::File | Icon::Documents => ('▤', th.text),
            Icon::Terminal => ('▶', th.text),
            Icon::Notepad => ('≡', th.text),
            Icon::Mines => ('✱', th.text),
            Icon::Monitor => ('▟', th.green),
            Icon::Git => ('±', th.orange),
            Icon::Vim => ('V', th.green),
            Icon::Recycle => ('♻', th.text),
            Icon::Run => ('»', th.accent),
            Icon::Help => ('?', th.accent),
            Icon::Info => ('i', th.accent),
            Icon::Shutdown => ('⏻', th.red),
            Icon::Settings => ('⚙', th.dim),
        }
    }

    /// Line and braille drawing for the Lite desktop, 7 wide by 3 tall.
    pub fn lines(self) -> [&'static str; 3] {
        match self {
            Icon::Computer => ["╭─────╮", "╰──┬──╯", " ──┴── "],
            Icon::Folder | Icon::Programs | Icon::Documents => ["╭──╮   ", "│  ╰──╮", "╰─────╯"],
            Icon::Terminal => ["╭─────╮", "│ ❯ _ │", "╰─────╯"],
            Icon::Notepad | Icon::File => ["┌────╮ ", "│ ══ │ ", "└────┘ "],
            Icon::Mines => ["   ╲   ", " ⣴⣿⣿⣦  ", " ⠻⣿⣿⠟  "],
            Icon::Monitor => ["╭─────╮", "│⣀⡠⠔⠊⠉│", "╰─────╯"],
            Icon::Git => [" ●──╮  ", " │  ●  ", " ●──╯  "],
            Icon::Recycle => ["╶─────╴", " │┊┊┊│ ", " ╰───╯ "],
            Icon::Vim => [" ╲   ╱ ", "  ╲ ╱  ", "   V   "],
            _ => ["╭─────╮", "│  ?  │", "╰─────╯"],
        }
    }

    pub fn art(self) -> &'static [&'static str] {
        match self {
            Icon::Computer => &[
                ".ffffff.",
                ".faaaaf.",
                ".faaaaf.",
                ".ffffff.",
                "...ll...",
                ".llllll.",
            ],
            Icon::Folder | Icon::Programs | Icon::Documents => &[
                "........",
                ".yyy....",
                ".oooooo.",
                ".yyyyyy.",
                ".yyyyyy.",
                ".oooooo.",
            ],
            Icon::Terminal => &[
                "aaaaaaaa",
                "kkkkkkkk",
                "kfkkkkkk",
                "kkfkkkkk",
                "kfkkfffk",
                "kkkkkkkk",
            ],
            Icon::Notepad | Icon::File => &[
                ".aaaaaa.",
                ".ffffff.",
                ".fmmmmf.",
                ".ffffff.",
                ".fmmmff.",
                ".ffffff.",
            ],
            Icon::Mines => &[
                ".....y..",
                "....o...",
                "..lll...",
                ".lflll..",
                ".lllll..",
                "..lll...",
            ],
            Icon::Monitor => &[
                "llllllll",
                "lkkkkgkl",
                "lkgkggkl",
                "lggggggl",
                "llllllll",
                "..llll..",
            ],
            Icon::Recycle => &[
                ".ffffff.",
                "..llll..",
                "..lmlm..",
                "..lmlm..",
                "..llll..",
                "........",
            ],
            Icon::Git => &[
                "..o.....",
                "..o..o..",
                "..o.o...",
                "..oo....",
                "..o.....",
                "..o.....",
            ],
            Icon::Vim => &[
                "gg....gg",
                ".gg..gg.",
                ".gg..gg.",
                "..gggg..",
                "..gggg..",
                "...gg...",
            ],
            _ => &[
                ".aaaaaa.",
                "aaffffaa",
                "aaaafaaa",
                "aaafaaaa",
                "aaaaaaaa",
                ".aaafaa.",
            ],
        }
    }
}

pub fn palette(th: &Theme) -> impl Fn(char) -> Option<Color> + '_ {
    move |c| match c {
        'a' => Some(th.accent),
        'f' => Some(th.text),
        'l' => Some(th.on_inactive),
        'm' => Some(th.dim),
        'k' => Some(th.shadow),
        'y' => Some(th.cyan),
        'o' => Some(th.yellow),
        'g' => Some(th.green),
        'r' => Some(th.red),
        _ => None,
    }
}
