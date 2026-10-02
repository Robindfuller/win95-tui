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
    Amp,
    Solitaire,
    Monitor,
    Vim,
    /// Painted in Add Program: 8 by 6 pixels of palette letters.
    Custom(&'static [&'static str]),
    Recycle,
    Run,
    Help,
    Info,
    Shutdown,
    Settings,
    Programs,
    Documents,
    Paint,
    Picture,
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
            Icon::Amp => ('♫', th.green),
            Icon::Solitaire => ('♠', th.text),
            Icon::Monitor => ('▟', th.green),
            Icon::Vim => ('V', th.green),
            Icon::Custom(rows) => ('■', main_colour(rows, th)),
            Icon::Recycle => ('♻', th.text),
            Icon::Run => ('»', th.accent),
            Icon::Help => ('?', th.accent),
            Icon::Info => ('i', th.accent),
            Icon::Shutdown => ('⏻', th.red),
            Icon::Paint => ('✎', th.magenta),
            Icon::Picture => ('▨', th.cyan),
            Icon::Settings => ('⚙', th.dim),
        }
    }

    pub fn art(self) -> &'static [&'static str] {
        match self {
            Icon::Custom(rows) => rows,
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
            Icon::Solitaire => &[
                "bbbbb...",
                "bybyb...",
                "bybffff.",
                "bbbfrrf.",
                "...frrf.",
                "...ffff.",
            ],
            Icon::Amp => &[
                "..gggggg",
                "..g....g",
                "..g....g",
                "ggg..ggg",
                "ggg..ggg",
                "........",
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
            Icon::Paint => &[
                "..llll..",
                ".lrlolll",
                "lllllgll",
                "lbllll..",
                ".llpll..",
                "...ll...",
            ],
            Icon::Picture => &[
                "ffffffff",
                "fyyyyonf",
                "fyyyyyyf",
                "fyygyyyf",
                "fggggggf",
                "ffffffff",
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
        'b' => Some(th.blue),
        'p' => Some(th.magenta),
        'n' => Some(th.orange),
        _ => None,
    }
}

/// The colours the icon painter offers, as palette letters.
pub const PAINT: [char; 12] = ['f', 'm', 'k', 'l', 'r', 'n', 'o', 'g', 'y', 'b', 'p', 'a'];

/// A painted icon from its pixel rows. The rows live as long as the program
/// does, so the icon can stay a plain Copy value like the built-in ones.
pub fn custom(rows: &[String]) -> Icon {
    let rows: Vec<&'static str> = (0..6)
        .map(|i| {
            let r: String = rows.get(i).map(|r| r.chars().chain(std::iter::repeat('.')).take(8).collect()).unwrap_or_else(|| ".".repeat(8));
            &*Box::leak(r.into_boxed_str())
        })
        .collect();
    Icon::Custom(Box::leak(rows.into_boxed_slice()))
}

fn main_colour(rows: &[&str], th: &Theme) -> Color {
    let mut counts = std::collections::HashMap::new();
    for c in rows.iter().flat_map(|r| r.chars()).filter(|c| *c != '.') {
        *counts.entry(c).or_insert(0) += 1;
    }
    counts.into_iter().max_by_key(|(c, n)| (*n, *c)).and_then(|(c, _)| palette(th)(c)).unwrap_or(th.text)
}
