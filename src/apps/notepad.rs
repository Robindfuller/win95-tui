use super::{Action, App, Launch};
use crate::{clip, draw::{st, Canvas}, icons::Icon, menu::{Cmd, Item, Sys}, theme::{home, Theme}};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::style::Modifier;
use std::{
    cell::RefCell,
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    rc::Rc,
    time::Instant,
};

/// One line of the folder list: a file or folder, how deep it is, and its name.
struct Row {
    path: PathBuf,
    depth: usize,
    dir: bool,
    name: String,
}

/// The folder down the left, like Sublime Text's side bar. Every tab in a
/// Notes window shares the one, so they all show it and Close Folder shuts it
/// for all of them.
#[derive(Default)]
pub struct Tree {
    pub(crate) root: Option<PathBuf>,
    /// folders opened out
    open: HashSet<PathBuf>,
    rows: Vec<Row>,
    sel: usize,
    top: usize,
    /// when the folders were last read, to pick up changes made elsewhere
    read: Option<Instant>,
    /// goes up whenever the rows change, so each tab knows to redraw
    version: u64,
}

pub type SharedTree = Rc<RefCell<Tree>>;

/// Too many rows to be any use, and a folder like / would take ages.
const MAX_ROWS: usize = 5000;

impl Tree {
    fn set_root(&mut self, p: PathBuf) {
        self.open = HashSet::from([p.clone()]);
        self.root = Some(p);
        (self.sel, self.top) = (0, 0);
        self.refresh();
    }

    fn close(&mut self) {
        *self = Tree { version: self.version + 1, ..Tree::default() };
    }

    /// Read the open folders again.
    fn refresh(&mut self) {
        self.read = Some(Instant::now());
        let mut rows = vec![];
        if let Some(root) = &self.root {
            let name = root.file_name().map_or(root.display().to_string(), |n| n.to_string_lossy().to_string());
            rows.push(Row { path: root.clone(), depth: 0, dir: true, name });
            self.walk(root, 1, &mut rows);
        }
        let same = rows.len() == self.rows.len() && rows.iter().zip(&self.rows).all(|(a, b)| a.path == b.path && a.dir == b.dir);
        if !same {
            // keep the same file picked if it's still there
            let was = self.rows.get(self.sel).map(|r| r.path.clone());
            self.rows = rows;
            self.sel = was.and_then(|w| self.rows.iter().position(|r| r.path == w)).unwrap_or(self.sel).min(self.rows.len().saturating_sub(1));
            self.version += 1;
        }
    }

    fn walk(&self, dir: &Path, depth: usize, rows: &mut Vec<Row>) {
        let Ok(rd) = fs::read_dir(dir) else { return };
        let mut v: Vec<(bool, String, PathBuf)> = rd
            .flatten()
            .map(|e| (e.path(), e.file_name().to_string_lossy().to_string()))
            .filter(|(_, n)| !n.starts_with('.'))
            .map(|(p, n)| (p.is_dir(), n, p))
            .collect();
        v.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.to_lowercase().cmp(&b.1.to_lowercase())));
        for (dir, name, path) in v {
            if rows.len() >= MAX_ROWS {
                return;
            }
            let open = dir && self.open.contains(&path);
            rows.push(Row { path: path.clone(), depth, dir, name });
            if open {
                self.walk(&path, depth + 1, rows);
            }
        }
    }

    /// Open a folder out, or fold it away.
    fn toggle(&mut self, i: usize) {
        let Some(r) = self.rows.get(i).filter(|r| r.dir && r.depth > 0) else { return };
        let p = r.path.clone();
        if !self.open.remove(&p) {
            self.open.insert(p);
        }
        self.refresh();
    }

    fn keep_visible(&mut self, rows: usize) {
        if self.sel < self.top {
            self.top = self.sel;
        }
        if self.sel >= self.top + rows {
            self.top = self.sel + 1 - rows;
        }
    }
}

pub struct Notepad {
    lines: Vec<Vec<char>>,
    cx: usize,
    cy: usize,
    top: usize,
    left: usize,
    path: Option<PathBuf>,
    modified: bool,
    size: (u16, u16),
    prompt: Option<String>,
    status: Option<String>,
    /// where a selection started (line, column); it runs to the cursor
    anchor: Option<(usize, usize)>,
    last_click: Option<(Instant, usize, usize)>,
    tree: SharedTree,
    /// the tree version last drawn
    drawn: u64,
    /// keys go to the folder list rather than the text
    in_tree: bool,
}

type Span = ((usize, usize), (usize, usize));

fn wordy(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

pub fn expand(p: &str) -> PathBuf {
    match p.strip_prefix("~/") {
        Some(rest) => home().join(rest),
        None if p == "~" => home(),
        None => PathBuf::from(p),
    }
}

/// A file's lines, and a word for the status bar if it couldn't be read.
fn read(p: &Path) -> (Vec<Vec<char>>, Option<String>) {
    match fs::read(p) {
        Ok(bytes) => {
            let txt = String::from_utf8_lossy(&bytes);
            let mut lines: Vec<Vec<char>> = txt.split('\n').map(|l| l.trim_end_matches('\r').chars().collect()).collect();
            if lines.len() > 1 && lines.last().is_some_and(|l| l.is_empty()) {
                lines.pop();
            }
            (lines, None)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (vec![vec![]], Some("New file".into())),
        Err(e) => (vec![vec![]], Some(format!("Cannot open: {e}"))),
    }
}

impl Notepad {
    pub fn new(path: Option<PathBuf>) -> Notepad {
        let (lines, status) = path.as_deref().map(read).unwrap_or((vec![vec![]], None));
        Notepad {
            lines,
            cx: 0,
            cy: 0,
            top: 0,
            left: 0,
            path,
            modified: false,
            size: (60, 18),
            prompt: None,
            status,
            anchor: None,
            last_click: None,
            tree: SharedTree::default(),
            drawn: 0,
            in_tree: false,
        }
    }

    /// The folder File > Open starts in.
    fn here(&self) -> PathBuf {
        self.path.as_ref().and_then(|p| p.parent().map(Path::to_path_buf)).unwrap_or_else(home)
    }

    /// Nothing typed and no file: a file opened from here can just replace it.
    fn blank(&self) -> bool {
        self.path.is_none() && !self.modified && self.lines.len() == 1 && self.lines[0].is_empty()
    }

    /// Open a file from the folder list or File > Open: here if this tab is
    /// empty, else in a tab of its own, and things that aren't text in
    /// whatever they belong to.
    fn open_file(&mut self, p: &Path) -> Action {
        if self.path.as_deref() == Some(p) {
            return Action::None;
        }
        let l = crate::assoc::launch(p);
        if !matches!(l, Launch::Notepad(_)) {
            return Action::Launch(l);
        }
        if !self.blank() {
            return Action::OpenTab(l);
        }
        (self.lines, self.status) = read(p);
        self.path = Some(p.to_path_buf());
        (self.cx, self.cy, self.top, self.left, self.anchor) = (0, 0, 0, 0, None);
        Action::None
    }

    /// Columns the folder list takes, with the line after it; none with no folder open.
    fn side(&self) -> usize {
        if self.tree.borrow().root.is_none() {
            return 0;
        }
        let w = self.size.0 as usize;
        (w / 3).clamp(12, 30).min(w / 2) + 1
    }

    fn text_w(&self) -> usize {
        (self.size.0 as usize).saturating_sub(self.side()).max(1)
    }

    /// A click or key on row i of the folder list.
    fn tree_pick(&mut self, i: usize) -> Action {
        let (dir, p) = {
            let t = self.tree.borrow();
            let Some(r) = t.rows.get(i) else { return Action::None };
            (r.dir, r.path.clone())
        };
        if dir {
            self.tree.borrow_mut().toggle(i);
            Action::None
        } else {
            self.in_tree = false;
            self.open_file(&p)
        }
    }

    fn tree_key(&mut self, k: KeyEvent) -> Option<Action> {
        let rows = self.text_rows();
        let mut t = self.tree.borrow_mut();
        let n = t.rows.len();
        match k.code {
            KeyCode::Up => t.sel = t.sel.saturating_sub(1),
            KeyCode::Down => t.sel = (t.sel + 1).min(n.saturating_sub(1)),
            KeyCode::PageUp => t.sel = t.sel.saturating_sub(rows),
            KeyCode::PageDown => t.sel = (t.sel + rows).min(n.saturating_sub(1)),
            KeyCode::Home => t.sel = 0,
            KeyCode::End => t.sel = n.saturating_sub(1),
            KeyCode::Right | KeyCode::Left => {
                let i = t.sel;
                let Some(r) = t.rows.get(i) else { return Some(Action::None) };
                let (open, dir, depth) = (t.open.contains(&r.path), r.dir, r.depth);
                if dir && depth > 0 && open != (k.code == KeyCode::Right) {
                    t.toggle(i);
                } else if k.code == KeyCode::Left && depth > 0 {
                    // already folded: go up to the folder it's in
                    t.sel = (0..i).rev().find(|&j| t.rows[j].depth < depth).unwrap_or(0);
                }
            }
            KeyCode::Enter => {
                let i = t.sel;
                drop(t);
                return Some(self.tree_pick(i));
            }
            KeyCode::Esc | KeyCode::Tab => {
                drop(t);
                self.in_tree = false;
                return Some(Action::None);
            }
            // anything else goes back to the text
            _ => {
                drop(t);
                self.in_tree = false;
                return None;
            }
        }
        t.keep_visible(rows);
        Some(Action::None)
    }

    fn render_tree(&self, c: &mut Canvas, th: &Theme, focused: bool) {
        let side = self.side();
        if side == 0 {
            return;
        }
        let (w, rows) = (side as i32 - 1, self.text_rows());
        let bg = st(th.text, th.face);
        c.fill(0, 0, w, rows as i32, bg);
        for y in 0..rows as i32 {
            c.put(w, y, "│", st(th.shadow, th.face));
        }
        let t = self.tree.borrow();
        for (row, (i, r)) in t.rows.iter().enumerate().skip(t.top).take(rows).enumerate() {
            let y = row as i32;
            let showing = self.path.as_deref() == Some(r.path.as_path());
            let s = if i == t.sel && self.in_tree && focused { th.sel() } else if showing { st(th.text, th.client) } else { bg };
            if s != bg {
                c.fill(0, y, w, 1, s);
            }
            let arrow = if !r.dir { " " } else if t.open.contains(&r.path) { "▾" } else { "▸" };
            let x = c.text_max(r.depth as i32 * 2, y, arrow, s, w);
            let s = if r.dir { s.add_modifier(Modifier::BOLD) } else { s };
            c.text_max(x + 1, y, &r.name, s, w);
        }
    }

    fn text_rows(&self) -> usize {
        (self.size.1 as usize).saturating_sub(if self.prompt.is_some() || self.status.is_some() { 1 } else { 0 }).max(1)
    }

    fn clamp(&mut self) {
        self.cy = self.cy.min(self.lines.len() - 1);
        self.cx = self.cx.min(self.lines[self.cy].len());
        let rows = self.text_rows();
        if self.cy < self.top {
            self.top = self.cy;
        }
        if self.cy >= self.top + rows {
            self.top = self.cy + 1 - rows;
        }
        let w = self.text_w();
        if self.cx < self.left {
            self.left = self.cx;
        }
        if self.cx >= self.left + w {
            self.left = self.cx + 1 - w;
        }
    }

    /// The selected span, start first, if there's anything in it.
    fn sel(&self) -> Option<Span> {
        let (a, b) = (self.anchor?, (self.cy, self.cx));
        (a != b).then(|| if a < b { (a, b) } else { (b, a) })
    }

    fn selected(&self) -> Option<String> {
        let ((y0, x0), (y1, x1)) = self.sel()?;
        let mut s = String::new();
        for y in y0..=y1 {
            let l = &self.lines[y];
            s.extend(&l[if y == y0 { x0 } else { 0 }..if y == y1 { x1 } else { l.len() }]);
            if y < y1 {
                s.push('\n');
            }
        }
        Some(s)
    }

    /// Remove the selected text, if any. Returns whether there was some.
    fn delete_sel(&mut self) -> bool {
        let Some(((y0, x0), (y1, x1))) = self.sel() else {
            self.anchor = None;
            return false;
        };
        let tail = self.lines[y1].split_off(x1);
        self.lines[y0].truncate(x0);
        self.lines[y0].extend(tail);
        self.lines.drain(y0 + 1..=y1);
        (self.cy, self.cx) = (y0, x0);
        self.anchor = None;
        self.modified = true;
        true
    }

    fn copy(&mut self, cut: bool) {
        if let Some(t) = self.selected() {
            clip::copy_text(&t);
            if cut {
                self.delete_sel();
            }
        }
    }

    fn select_all(&mut self) {
        self.anchor = Some((0, 0));
        self.cy = self.lines.len() - 1;
        self.cx = self.lines[self.cy].len();
    }

    /// The text position under a point in the window, scrolling a line when
    /// a drag goes past the top or bottom.
    fn at(&self, x: i32, y: i32) -> (usize, usize) {
        let cy = if y < 0 { self.top.saturating_sub(1) } else { (self.top + y as usize).min(self.top + self.text_rows()) };
        let cy = cy.min(self.lines.len() - 1);
        (cy, (self.left + x.max(0) as usize).min(self.lines[cy].len()))
    }

    fn insert(&mut self, ch: char) {
        self.delete_sel();
        if ch == '\n' {
            let rest = self.lines[self.cy].split_off(self.cx);
            self.lines.insert(self.cy + 1, rest);
            self.cy += 1;
            self.cx = 0;
        } else {
            self.lines[self.cy].insert(self.cx, ch);
            self.cx += 1;
        }
        self.modified = true;
    }

    fn save(&mut self) -> Action {
        let Some(p) = self.path.clone() else {
            self.prompt = Some("~/untitled.txt".into());
            return Action::None;
        };
        let mut out: String = self.lines.iter().map(|l| l.iter().collect::<String>()).collect::<Vec<_>>().join("\n");
        out.push('\n');
        self.status = Some(match fs::write(&p, out) {
            Ok(_) => {
                self.modified = false;
                format!("Saved {}", p.display())
            }
            Err(e) => format!("Cannot save: {e}"),
        });
        Action::None
    }
}

impl App for Notepad {
    fn title(&self) -> String {
        let name = self.path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or("Untitled".into());
        format!("{}{} - Notes", name, if self.modified { "*" } else { "" })
    }
    fn icon(&self) -> Icon {
        Icon::Notepad
    }
    fn tab_title(&self) -> String {
        let name = self.path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or("Untitled".into());
        format!("{}{}", name, if self.modified { "*" } else { "" })
    }
    fn size_hint(&self) -> (u16, u16) {
        (64, 18)
    }

    fn render(&mut self, c: &mut Canvas, th: &Theme, focused: bool) {
        let s = st(th.text, th.client);
        c.fill(0, 0, self.size.0 as i32, self.size.1 as i32, s);
        self.drawn = self.tree.borrow().version;
        self.render_tree(c, th, focused);
        let (off, tw) = (self.side() as i32, self.text_w());
        let rows = self.text_rows();
        let sel = self.sel();
        for (i, line) in self.lines.iter().skip(self.top).take(rows).enumerate() {
            let shown: String = line.iter().skip(self.left).take(tw).map(|&ch| if ch == '\t' { ' ' } else { ch }).collect();
            c.text_max(off, i as i32, &shown, s, off + tw as i32);
            // selected text, and a cell past the end where the line break is selected
            let li = self.top + i;
            if let Some(((y0, x0), (y1, x1))) = sel.filter(|((y0, _), (y1, _))| (*y0..=*y1).contains(&li)) {
                let (a, b) = (if li == y0 { x0 } else { 0 }, if li == y1 { x1 } else { line.len() + 1 });
                for ci in a.max(self.left)..b.min(self.left + tw) {
                    let ch = match line.get(ci) {
                        Some('\t') | None => ' ',
                        Some(&ch) => ch,
                    };
                    c.put_c(off + (ci - self.left) as i32, i as i32, ch, th.sel());
                }
            }
        }
        let y = self.size.1 as i32 - 1;
        if let Some(p) = &self.prompt {
            c.fill(0, y, self.size.0 as i32, 1, st(th.text, th.face));
            let x = c.text(1, y, "Save as:", st(th.text, th.face));
            c.field(x + 1, y, self.size.0 as i32 - x - 2, p, th);
        } else if let Some(m) = &self.status {
            c.fill(0, y, self.size.0 as i32, 1, st(th.text, th.face));
            c.text_max(1, y, m, st(th.text, th.face), self.size.0 as i32);
        }
    }

    fn cursor(&self) -> Option<(u16, u16)> {
        if let Some(p) = &self.prompt {
            let x = 11 + p.chars().count().min(self.size.0 as usize - 13);
            return Some((x as u16, self.size.1 - 1));
        }
        if self.in_tree {
            return None;
        }
        let (x, y) = (self.cx.checked_sub(self.left)?, self.cy.checked_sub(self.top)?);
        (x < self.text_w() && y < self.text_rows()).then_some(((x + self.side()) as u16, y as u16))
    }

    fn key(&mut self, k: KeyEvent) -> Action {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        if let Some(p) = &mut self.prompt {
            match k.code {
                KeyCode::Enter => {
                    self.path = Some(expand(p.trim()));
                    self.prompt = None;
                    return self.save();
                }
                KeyCode::Esc => self.prompt = None,
                KeyCode::Backspace => {
                    p.pop();
                }
                KeyCode::Char(ch) if !ctrl => p.push(ch),
                _ => {}
            }
            return Action::None;
        }
        self.status = None;
        if k.code == KeyCode::Char('e') && ctrl {
            return self.command("tree");
        }
        if self.in_tree {
            if let Some(a) = self.tree_key(k) {
                return a;
            }
        }
        let rows = self.text_rows();
        let shift = k.modifiers.contains(KeyModifiers::SHIFT);
        let moving = matches!(k.code, KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down | KeyCode::Home | KeyCode::End | KeyCode::PageUp | KeyCode::PageDown);
        if moving && shift {
            self.anchor.get_or_insert((self.cy, self.cx));
        } else if moving {
            self.anchor = None;
        }
        match k.code {
            KeyCode::Char('s') if ctrl => return self.save(),
            KeyCode::Char('c') | KeyCode::Insert if ctrl => self.copy(false),
            KeyCode::Char('x') if ctrl => self.copy(true),
            KeyCode::Delete if shift => self.copy(true),
            KeyCode::Char('v') if ctrl => return self.command("paste"),
            KeyCode::Insert if shift => return self.command("paste"),
            KeyCode::Char('a') if ctrl => self.select_all(),
            KeyCode::Char('n') if ctrl => return Action::Launch(Launch::Notepad(None)),
            KeyCode::F(5) => {
                for ch in chrono::Local::now().format("%H:%M %d/%m/%Y").to_string().chars() {
                    self.insert(ch);
                }
            }
            KeyCode::Char(ch) if !ctrl => self.insert(ch),
            KeyCode::Tab => self.insert('\t'),
            KeyCode::Enter => self.insert('\n'),
            KeyCode::Backspace if self.delete_sel() => {}
            KeyCode::Delete if self.delete_sel() => {}
            KeyCode::Backspace => {
                if self.cx > 0 {
                    self.cx -= 1;
                    self.lines[self.cy].remove(self.cx);
                    self.modified = true;
                } else if self.cy > 0 {
                    let line = self.lines.remove(self.cy);
                    self.cy -= 1;
                    self.cx = self.lines[self.cy].len();
                    self.lines[self.cy].extend(line);
                    self.modified = true;
                }
            }
            KeyCode::Delete => {
                if self.cx < self.lines[self.cy].len() {
                    self.lines[self.cy].remove(self.cx);
                    self.modified = true;
                } else if self.cy + 1 < self.lines.len() {
                    let line = self.lines.remove(self.cy + 1);
                    self.lines[self.cy].extend(line);
                    self.modified = true;
                }
            }
            KeyCode::Left => {
                if self.cx > 0 {
                    self.cx -= 1;
                } else if self.cy > 0 {
                    self.cy -= 1;
                    self.cx = self.lines[self.cy].len();
                }
            }
            KeyCode::Right => {
                if self.cx < self.lines[self.cy].len() {
                    self.cx += 1;
                } else if self.cy + 1 < self.lines.len() {
                    self.cy += 1;
                    self.cx = 0;
                }
            }
            KeyCode::Up => self.cy = self.cy.saturating_sub(1),
            KeyCode::Down => self.cy += 1,
            KeyCode::Home => self.cx = 0,
            KeyCode::End => self.cx = usize::MAX,
            KeyCode::PageUp => self.cy = self.cy.saturating_sub(rows),
            KeyCode::PageDown => self.cy += rows,
            _ => {}
        }
        self.clamp();
        Action::None
    }

    fn mouse(&mut self, kind: MouseEventKind, x: i32, y: i32, mods: KeyModifiers) -> Action {
        let side = self.side() as i32;
        if x < side && (y as usize) < self.text_rows() {
            let rows = self.text_rows();
            match kind {
                MouseEventKind::Down(MouseButton::Left) if x < side - 1 => {
                    let i = self.tree.borrow().top + y.max(0) as usize;
                    if i < self.tree.borrow().rows.len() {
                        self.tree.borrow_mut().sel = i;
                        self.in_tree = true;
                        return self.tree_pick(i);
                    }
                }
                MouseEventKind::ScrollUp => {
                    let mut t = self.tree.borrow_mut();
                    t.top = t.top.saturating_sub(3);
                }
                MouseEventKind::ScrollDown => {
                    let mut t = self.tree.borrow_mut();
                    t.top = (t.top + 3).min(t.rows.len().saturating_sub(rows));
                }
                _ => {}
            }
            return Action::None;
        }
        let x = x - side;
        if matches!(kind, MouseEventKind::Down(_)) {
            self.in_tree = false;
        }
        match kind {
            MouseEventKind::Down(MouseButton::Left) if self.prompt.is_none() => {
                if y >= 0 && (y as usize) < self.text_rows() {
                    let (cy, cx) = self.at(x, y);
                    let double = self.last_click.is_some_and(|(t, ly, lx)| (ly, lx) == (cy, cx) && t.elapsed().as_millis() < 450);
                    self.last_click = if double { None } else { Some((Instant::now(), cy, cx)) };
                    if mods.contains(KeyModifiers::SHIFT) {
                        self.anchor.get_or_insert((self.cy, self.cx));
                    } else {
                        self.anchor = Some((cy, cx));
                    }
                    (self.cy, self.cx) = (cy, cx);
                    if double {
                        // a double click picks the word under it
                        let l = &self.lines[cy];
                        let start = (0..cx).rev().take_while(|&i| wordy(l[i])).last().unwrap_or(cx);
                        let end = (cx..l.len()).take_while(|&i| wordy(l[i])).last().map(|i| i + 1).unwrap_or(cx);
                        self.anchor = Some((cy, start));
                        self.cx = end;
                    }
                    self.clamp();
                }
            }
            MouseEventKind::Drag(MouseButton::Left) if self.prompt.is_none() && self.anchor.is_some() => {
                (self.cy, self.cx) = self.at(x, y);
                self.clamp();
            }
            MouseEventKind::ScrollUp => {
                self.top = self.top.saturating_sub(3);
                self.cy = self.cy.min(self.top + self.text_rows() - 1);
                self.clamp();
            }
            MouseEventKind::ScrollDown => {
                self.top = (self.top + 3).min(self.lines.len().saturating_sub(1));
                self.cy = self.cy.max(self.top);
                self.clamp();
            }
            _ => {}
        }
        Action::None
    }

    fn paste(&mut self, s: &str) {
        for ch in s.replace("\r\n", "\n").chars() {
            self.insert(ch);
        }
        self.clamp();
    }

    fn resize(&mut self, w: u16, h: u16) {
        self.size = (w.max(1), h.max(1));
        self.clamp();
    }

    fn menubar(&self) -> Vec<(&'static str, Vec<Item>)> {
        vec![
            ("File", vec![
                Item::new("New", Cmd::App("new")).key("Ctrl+N"),
                Item::new("Open...", Cmd::App("open")),
                Item::new("Open Folder...", Cmd::App("openfolder")),
                Item::new("Close Folder", Cmd::App("closefolder")).enabled(self.side() > 0),
                Item::new("Save", Cmd::App("save")).key("Ctrl+S"),
                Item::new("Save As...", Cmd::App("saveas")),
                Item::sep(),
                Item::new("Exit", Cmd::Sys(Sys::Close)),
            ]),
            ("Edit", vec![
                Item::new("Cut", Cmd::App("cut")).key("Ctrl+X"),
                Item::new("Copy", Cmd::App("copy")).key("Ctrl+C"),
                Item::new("Paste", Cmd::App("paste")).key("Ctrl+V"),
                Item::new("Delete", Cmd::App("del")).key("Del"),
                Item::sep(),
                Item::new("Select All", Cmd::App("all")).key("Ctrl+A"),
                Item::new("Time/Date", Cmd::App("date")).key("F5"),
            ]),
            ("View", vec![Item::new("Folder List", Cmd::App("tree")).key("Ctrl+E").enabled(self.side() > 0)]),
            ("Help", vec![Item::new("About Notes", Cmd::App("about")).icon(Icon::Info)]),
        ]
    }

    fn new_tab(&self) -> Option<Launch> {
        Some(Launch::Notepad(None))
    }

    /// What a picker picked: a folder for the list down the left, or a file to open.
    fn open_more(&mut self, files: &[PathBuf]) -> Action {
        let Some(f) = files.first() else { return Action::None };
        if f.is_dir() {
            self.tree.borrow_mut().set_root(f.clone());
            self.clamp();
            return Action::None;
        }
        self.open_file(f)
    }

    fn showing(&self) -> Option<PathBuf> {
        self.path.clone()
    }

    fn tree(&self) -> Option<SharedTree> {
        Some(self.tree.clone())
    }

    fn set_tree(&mut self, t: SharedTree) {
        self.tree = t;
        self.clamp();
    }

    /// Pick up files made, moved or deleted elsewhere, and changes another tab made to the list.
    fn poll(&mut self) -> (bool, Action) {
        let mut t = self.tree.borrow_mut();
        if t.root.is_some() && t.read.is_none_or(|r| r.elapsed().as_secs() >= 2) {
            t.refresh();
        }
        (t.version != self.drawn, Action::None)
    }

    fn command(&mut self, cmd: &str) -> Action {
        match cmd {
            "new" => Action::Launch(Launch::Notepad(None)),
            "open" => Action::Pick { dir: self.here(), folder: false },
            "openfolder" => {
                let dir = self.tree.borrow().root.clone().and_then(|r| r.parent().map(Path::to_path_buf)).unwrap_or_else(|| self.here());
                Action::Pick { dir, folder: true }
            }
            "closefolder" => {
                self.tree.borrow_mut().close();
                self.in_tree = false;
                self.clamp();
                Action::None
            }
            "tree" => {
                self.in_tree = self.side() > 0 && !self.in_tree;
                Action::None
            }
            "save" => self.save(),
            "saveas" => {
                self.prompt = Some(self.path.as_ref().map(|p| p.display().to_string()).unwrap_or("~/untitled.txt".into()));
                Action::None
            }
            "date" => self.key(KeyEvent::new(KeyCode::F(5), KeyModifiers::NONE)),
            "cut" | "copy" => {
                self.copy(cmd == "cut");
                self.clamp();
                Action::None
            }
            "paste" => {
                match clip::paste_text() {
                    Some(t) => self.paste(&t),
                    None => self.status = Some("Can't read the clipboard here. Use your terminal's paste key.".into()),
                }
                Action::None
            }
            "del" => {
                self.delete_sel();
                self.clamp();
                Action::None
            }
            "all" => {
                self.select_all();
                self.clamp();
                Action::None
            }
            "about" => Action::Launch(Launch::Msg { title: "About Notes".into(), text: "Notes\nA plain text editor.\n\nCtrl+S save, F5 inserts the time and date.\nDrag or Shift+arrows to select, Ctrl+C/X/V to copy, cut and paste.\nFile > Open Folder shows a folder down the left; Ctrl+E moves to it and back.".into() }),
            _ => Action::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pad(txt: &str) -> Notepad {
        let mut n = Notepad::new(None);
        n.paste(txt);
        n
    }

    fn text(n: &Notepad) -> String {
        n.lines.iter().map(|l| l.iter().collect::<String>()).collect::<Vec<_>>().join("\n")
    }

    fn key(n: &mut Notepad, code: KeyCode, m: KeyModifiers) {
        n.key(KeyEvent::new(code, m));
    }

    #[test]
    fn folder_list_opens_files() {
        let root = std::env::temp_dir().join(format!("win95-tree-{}", std::process::id()));
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
        fs::write(root.join("README"), "hello\n").unwrap();
        fs::write(root.join(".hidden"), "").unwrap();
        let mut n = Notepad::new(None);
        n.resize(60, 18);
        n.open_more(std::slice::from_ref(&root));
        let names = |n: &Notepad| n.tree.borrow().rows.iter().map(|r| r.name.clone()).collect::<Vec<_>>();
        let root_name = root.file_name().unwrap().to_string_lossy().to_string();
        assert_eq!(names(&n), [root_name.as_str(), "src", "README"]);
        let left = MouseEventKind::Down(MouseButton::Left);
        // click a folder to open it out, then a file: it opens here, as this tab is empty
        n.mouse(left, 2, 1, KeyModifiers::NONE);
        assert_eq!(names(&n)[2], "main.rs");
        n.mouse(left, 4, 2, KeyModifiers::NONE);
        assert_eq!(n.showing(), Some(root.join("src/main.rs")));
        assert_eq!(text(&n), "fn main() {}");
        // another file now wants a tab of its own
        assert!(matches!(n.mouse(left, 2, 3, KeyModifiers::NONE), Action::OpenTab(Launch::Notepad(Some(p))) if p == root.join("README")));
        // the text moves over for the list, and back when the folder closes
        assert_eq!(n.cursor(), Some((n.side() as u16, 0)));
        n.command("closefolder");
        assert_eq!((n.side(), n.cursor()), (0, Some((0, 0))));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn shift_arrows_select_across_lines() {
        let mut n = pad("hello\nworld");
        (n.cy, n.cx) = (0, 3);
        key(&mut n, KeyCode::Down, KeyModifiers::SHIFT);
        key(&mut n, KeyCode::Right, KeyModifiers::SHIFT);
        assert_eq!(n.selected().as_deref(), Some("lo\nworl"));
        key(&mut n, KeyCode::Char('X'), KeyModifiers::SHIFT);
        assert_eq!(text(&n), "helXd");
        assert_eq!((n.cy, n.cx, n.sel()), (0, 4, None));
    }

    #[test]
    fn backwards_selection_and_paste_replace() {
        let mut n = pad("one two three");
        n.cx = 7;
        key(&mut n, KeyCode::Home, KeyModifiers::SHIFT);
        assert_eq!(n.selected().as_deref(), Some("one two"));
        n.paste("a\nb");
        assert_eq!(text(&n), "a\nb three");
        key(&mut n, KeyCode::Char('a'), KeyModifiers::CONTROL);
        key(&mut n, KeyCode::Backspace, KeyModifiers::NONE);
        assert_eq!(text(&n), "");
    }

    #[test]
    fn click_drag_and_double_click() {
        let mut n = pad("alpha beta_2 gamma");
        n.mouse(MouseEventKind::Down(MouseButton::Left), 2, 0, KeyModifiers::NONE);
        n.mouse(MouseEventKind::Drag(MouseButton::Left), 9, 0, KeyModifiers::NONE);
        assert_eq!(n.selected().as_deref(), Some("pha bet"));
        // a plain click drops the selection
        n.mouse(MouseEventKind::Down(MouseButton::Left), 15, 0, KeyModifiers::NONE);
        assert_eq!(n.selected(), None);
        n.last_click = None;
        n.mouse(MouseEventKind::Down(MouseButton::Left), 8, 0, KeyModifiers::NONE);
        n.mouse(MouseEventKind::Down(MouseButton::Left), 8, 0, KeyModifiers::NONE);
        assert_eq!(n.selected().as_deref(), Some("beta_2"));
        // shift+click stretches it
        n.mouse(MouseEventKind::Down(MouseButton::Left), 18, 0, KeyModifiers::SHIFT);
        assert_eq!(n.selected().as_deref(), Some("beta_2 gamma"));
        key(&mut n, KeyCode::Delete, KeyModifiers::NONE);
        assert_eq!(text(&n), "alpha ");
    }
}
