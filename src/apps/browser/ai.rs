// AI-assisted mode: Claude looks at the page as a real browser draws it (a
// screenshot, and a map of every box on it with its place and colours) and
// writes it out again in a little layout language: coloured bands, menus,
// columns and boxes, pointing back at the page's own text so nothing is
// retyped. That's drawn here in the page's own colours. `claude -p` runs on a
// background thread and bills the subscription.
use super::{
    chrome::{self, Look},
    fetch::Msg,
    page::{self, Span, Src},
    readable, Doc, Lay, Pic, Seg, What, K,
};
use crate::theme::{contrast, mix, Rgb};
use serde_json::{json, Value};
use std::{
    io::Write,
    process::{Command, Stdio},
    sync::mpsc::Sender,
};
use unicode_width::UnicodeWidthStr;
use url::Url;

const MODEL: &str = "sonnet";
/// how much of each box's text Claude is shown; it pulls in the rest by number
const PEEK: usize = 110;

const PROMPT: &str = r#"You redraw web pages for a text-mode browser. You get a screenshot of the top of a page and a numbered map of everything on it. Each map line is one box on the page: its number, tag, position (@x,y in pixels, on a page 1280px wide), size, font size, colours (text on background) and text. Links show as [text](L12).

Write the page out in the layout language below so that in a terminal about 100 to 140 columns wide it looks like a text-mode version of the original: the same header strip and colours, the same menus, the same columns and sidebars, the same boxes and cards, in the same order and places. Use the page's real colours from the map for bands, boxes and headings. Pull the page's own content in with @ references rather than retyping it; type only short labels and menus yourself. Use ranges like @40-180 for long runs of content; lines like "120-480 … more like the line above" are boxes left out of the map to save space, and they are real content to include. Leave out cookie banners, adverts, "skip to content" links and screen-reader-only text, but keep everything a reader would want. Above all, the main content goes in whole: cover it with one or a few big ranges (say @108-383) rather than picking out bits, never skip boxes in the middle of it, and don't retype headings the map already has. Equally, a range mustn't take in boxes you have already typed out yourself, such as the menus. Its pictures and captions come with it. A range is drawn in the page's own arrangement, so boxes side by side on the page (a grid of cards, a number and its title) come out side by side by themselves: one range covers a whole grid. At most 3 columns, and only where the page really has them side by side.

Layout language, one command per line:
page bg=#hex fg=#hex link=#hex      colours of the whole page; always the first line
band bg=#hex fg=#hex                a full-width coloured strip, up to its "end"
box "Title" border=#hex bg=#hex fg=#hex   a box with a border (title and colours optional), up to its "end"
cols 25 75                          columns side by side, widths in percent; "next" starts the next column; "end" closes
h1 text   h2 text   h3 text         headings
p text                              a paragraph
li text                             a bullet point
center text   right text            a line centred, or to the right
menu [Home](L1) [News](L2)          a row of links
@12   @12-40                        the page's own boxes, as they are
logo                                the site's logo
search                              the site's search box
rule   rule #hex                    a line across
gap                                 a blank line
end

In text, [label](L5) is link 5, **bold** is bold and {#hex some words} colours them.
Reply with the layout only: no explanations and no code fences."#;

/// One box on the page, from look.js.
#[derive(Clone, Debug)]
pub struct Item {
    tag: String,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    fg: Rgb,
    size: i32,
    bold: bool,
    spans: Vec<Span>,
    img: Option<usize>,
    alt: String,
}

#[derive(Clone, Debug)]
struct Bit {
    text: String,
    link: Option<usize>,
    bold: bool,
    fg: Option<Rgb>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Line {
    H(u8),
    P,
    Li,
    Center,
    Right,
    Menu,
}

#[derive(Clone, Debug)]
enum Node {
    Band { bg: Option<Rgb>, fg: Option<Rgb>, kids: Vec<Node> },
    Boxed { title: String, border: Option<Rgb>, bg: Option<Rgb>, fg: Option<Rgb>, kids: Vec<Node> },
    Cols { widths: Vec<u32>, cols: Vec<Vec<Node>> },
    Text(Line, Vec<Bit>),
    Refs(usize, usize),
    Logo,
    Search,
    Rule(Option<Rgb>),
    Gap,
}

/// The page as Claude redrew it.
#[derive(Clone, Debug)]
pub struct View {
    pub bg: Rgb,
    fg: Rgb,
    link: Rgb,
    items: Vec<Item>,
    nodes: Vec<Node>,
    /// the page's links and the ones only the map found, in that order
    pub links: Vec<Url>,
}

#[derive(Clone, Debug)]
pub enum Rebuild {
    Asking,
    Done(Box<View>),
    Failed(String),
}

/// What the background thread needs to know about the page.
pub struct Job {
    pub doc: u64,
    pub url: Url,
    pub title: String,
    pub links: Vec<Url>,
    pub imgs: Vec<Src>,
    pub look: Option<Box<Look>>,
}

/// Asks Claude to redraw a page; the answer comes back as `Msg::Rebuilt`.
pub fn rebuild(job: Job, tx: Sender<Msg>) {
    std::thread::spawn(move || {
        let id = job.doc;
        let r = match run(job) {
            Ok(v) => Rebuild::Done(Box::new(v)),
            Err(e) => Rebuild::Failed(e),
        };
        let _ = tx.send(Msg::Rebuilt(id, r));
    });
}

fn run(job: Job) -> Result<View, String> {
    let look = match job.look {
        Some(l) => *l,
        // a second go, in case Chromium had wedged and needed starting afresh
        None => chrome::get(&job.url, true).and_then(|g| g.2).or_else(|| chrome::get(&job.url, true).and_then(|g| g.2)).ok_or("couldn't open the page in Chromium")?,
    };
    let map: Value = serde_json::from_str(&look.map).map_err(|e| format!("the page map didn't read ({e})"))?;
    let mut links = job.links;
    let items = items(&map, &mut links, &job.imgs);
    if items.is_empty() {
        return Err("the page has nothing on it to redraw".into());
    }
    let mut text = format!("Page: {}\nAddress: {}\nPage colours: {} on {}\n\nMap:\n", job.title, job.url, s(&map["fg"]), s(&map["bg"]));
    text.push_str(&outline(&map, &items));
    let said = ask(&look.shot, &text)?;
    // WIN95_AI_LOG=dir keeps what Claude was shown and what it wrote
    if let Some(dir) = std::env::var_os("WIN95_AI_LOG") {
        let dir = std::path::PathBuf::from(dir);
        let _ = std::fs::write(dir.join("map.txt"), &text);
        let _ = std::fs::write(dir.join("layout.txt"), &said);
    }
    let (head, nodes) = parse(&said, links.len(), items.len());
    if nodes.is_empty() {
        return Err("Claude's layout was empty".into());
    }
    let bg = head.0.or_else(|| page::colour(s(&map["bg"]))).unwrap_or((255, 255, 255));
    let fg = ink(head.1.or_else(|| page::colour(s(&map["fg"]))).unwrap_or((0, 0, 0)), bg);
    let link = ink(head.2.unwrap_or((0, 0, 238)), bg);
    Ok(View { bg, fg, link, items, nodes, links })
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or("")
}

/// A colour that reads on `bg`.
fn ink(c: Rgb, bg: Rgb) -> Rgb {
    let far = if contrast(bg, (0, 0, 0)) > contrast(bg, (255, 255, 255)) { (0, 0, 0) } else { (255, 255, 255) };
    readable(c, bg, far)
}

fn items(map: &Value, links: &mut Vec<Url>, imgs: &[Src]) -> Vec<Item> {
    let mut link = |href: &str| -> Option<usize> {
        let u = Url::parse(href).ok()?;
        Some(links.iter().position(|l| *l == u).unwrap_or_else(|| {
            links.push(u);
            links.len() - 1
        }))
    };
    let Some(list) = map["items"].as_array() else { return vec![] };
    list.iter()
        .map(|it| {
            let spans = it["s"]
                .as_array()
                .map(|v| {
                    v.iter()
                        .map(|sp| Span { text: s(&sp[0]).to_string(), link: sp[1].as_str().and_then(&mut link), bold: sp[2].as_bool().unwrap_or(false), ..Default::default() })
                        .collect()
                })
                .unwrap_or_default();
            let src = s(&it["src"]);
            let img = (!src.is_empty()).then(|| imgs.iter().position(|i| matches!(i, Src::Url(u) if u.as_str() == src))).flatten();
            Item {
                tag: s(&it["t"]).to_string(),
                x: it["x"].as_i64().unwrap_or(0) as i32,
                y: it["y"].as_i64().unwrap_or(0) as i32,
                w: it["w"].as_i64().unwrap_or(0) as i32,
                h: it["h"].as_i64().unwrap_or(0) as i32,
                fg: page::colour(s(&it["fg"])).unwrap_or((0, 0, 0)),
                size: it["fs"].as_i64().unwrap_or(16) as i32,
                bold: it["b"].as_bool().unwrap_or(false),
                spans,
                img,
                alt: s(&it["alt"]).to_string(),
            }
        })
        .collect()
}

/// The map as Claude reads it: one line a box, except that a long run of
/// boxes alike (an article's paragraphs) shows only its first few.
fn outline(map: &Value, items: &[Item]) -> String {
    let raw = map["items"].as_array().cloned().unwrap_or_default();
    let mut out = String::new();
    let alike = |a: &Item, b: &Item| (a.x - b.x).abs() < 30 && a.fg == b.fg && a.size == b.size && a.size < 20 && !a.tag.starts_with('h') && !b.tag.starts_with('h') && a.tag != "img" && b.tag != "img";
    let mut run = 0;
    let mut skipped: Option<(usize, usize)> = None;
    for (i, (it, r)) in items.iter().zip(raw.iter()).enumerate() {
        run = if i > 0 && alike(&items[i - 1], it) { run + 1 } else { 0 };
        if run >= 3 {
            skipped = Some((skipped.map_or(i, |s| s.0), i));
            continue;
        }
        if let Some((a, b)) = skipped.take() {
            out.push_str(&format!("{a}-{b} … {} more like the line above\n", b - a + 1));
        }
        let n = |k: &str| r[k].as_i64().unwrap_or(0);
        out.push_str(&format!("{i} {} @{},{} {}x{} {}px{} {} on {}", it.tag, n("x"), n("y"), n("w"), n("h"), it.size, if it.bold { " bold" } else { "" }, s(&r["fg"]), s(&r["bg"])));
        match it.tag.as_str() {
            "img" => out.push_str(&format!(" picture{}{}", if it.alt.is_empty() { String::new() } else { format!(" \"{}\"", it.alt) }, if it.img.is_some() { "" } else { " (not loaded)" })),
            "input" => out.push_str(&format!(" text box \"{}\"", it.alt)),
            _ => {
                out.push_str(": ");
                let mut used = 0;
                for sp in &it.spans {
                    let left = PEEK.saturating_sub(used);
                    if left == 0 {
                        out.push('…');
                        break;
                    }
                    let t: String = sp.text.chars().take(left).collect();
                    used += t.chars().count();
                    match sp.link {
                        Some(l) => out.push_str(&format!("[{t}](L{l})")),
                        None => out.push_str(&t),
                    }
                }
            }
        }
        out.push('\n');
    }
    if let Some((a, b)) = skipped {
        out.push_str(&format!("{a}-{b} … {} more like the line above\n", b - a + 1));
    }
    out
}

fn ask(shot: &str, text: &str) -> Result<String, String> {
    let msg = json!({ "type": "user", "message": { "role": "user", "content": [
        { "type": "image", "source": { "type": "base64", "media_type": "image/jpeg", "data": shot } },
        { "type": "text", "text": text },
    ] } });
    let mut child = Command::new("claude")
        .args(["-p", "--model", MODEL, "--tools", "", "--strict-mcp-config", "--no-session-persistence"])
        .args(["--input-format", "stream-json", "--output-format", "stream-json", "--verbose", "--system-prompt", PROMPT])
        .current_dir(std::env::temp_dir())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("couldn't start claude ({e})"))?;
    if let Some(mut i) = child.stdin.take() {
        let _ = writeln!(i, "{msg}");
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    let said = String::from_utf8_lossy(&out.stdout);
    let result = said.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).find(|v| v["type"] == "result");
    match result {
        Some(r) if r["subtype"] == "success" && r["is_error"] != true => Ok(s(&r["result"]).to_string()),
        Some(r) => Err(s(&r["result"]).lines().next().unwrap_or("claude stopped with an error").to_string()),
        None => {
            let err = String::from_utf8_lossy(&out.stderr);
            Err(err.lines().find(|l| !l.trim().is_empty()).unwrap_or("claude said nothing").trim().to_string())
        }
    }
}

// ------------------------------------------------------------ the layout language

type Head = (Option<Rgb>, Option<Rgb>, Option<Rgb>);

fn attr(words: &[&str], key: &str) -> Option<Rgb> {
    words.iter().find_map(|w| page::colour(w.strip_prefix(key)?.strip_prefix('=')?))
}

enum Open {
    Band(Option<Rgb>, Option<Rgb>),
    Boxed(String, Option<Rgb>, Option<Rgb>, Option<Rgb>),
    Cols(Vec<u32>),
}

fn close(o: Open, mut lists: Vec<Vec<Node>>) -> Node {
    match o {
        Open::Band(bg, fg) => Node::Band { bg, fg, kids: lists.concat() },
        Open::Boxed(title, border, bg, fg) => Node::Boxed { title, border, bg, fg, kids: lists.concat() },
        Open::Cols(mut widths) => {
            lists.retain(|l| !l.is_empty());
            widths.resize(lists.len(), 0);
            if widths.iter().any(|w| *w == 0) {
                widths = vec![1; lists.len()];
            }
            Node::Cols { widths, cols: lists }
        }
    }
}

fn parse(text: &str, nlinks: usize, nitems: usize) -> (Head, Vec<Node>) {
    let mut head: Head = (None, None, None);
    let mut stack: Vec<(Open, Vec<Vec<Node>>)> = vec![];
    let mut top: Vec<Node> = vec![];
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with("```") {
            continue;
        }
        let (cmd, rest) = line.split_once(' ').unwrap_or((line, ""));
        let rest = rest.trim();
        let words: Vec<&str> = rest.split_whitespace().collect();
        let mut node = None;
        let only_refs = !rest.is_empty() && rest.split_whitespace().all(|w| w.starts_with('@') && w[1..].split('-').all(|n| n.parse::<usize>().is_ok()));
        let cmd = if only_refs && matches!(cmd, "p" | "li" | "center" | "right" | "h1" | "h2" | "h3" | "h4") { "@" } else { cmd };
        let line = if cmd == "@" { rest } else { line };
        match cmd {
            "page" => head = (attr(&words, "bg"), attr(&words, "fg"), attr(&words, "link")),
            "band" => stack.push((Open::Band(attr(&words, "bg"), attr(&words, "fg")), vec![vec![]])),
            "box" => {
                let quoted = rest.strip_prefix("title=").unwrap_or(rest);
                let title = quoted.strip_prefix('"').and_then(|r| r.split_once('"')).map(|(t, _)| t.to_string()).unwrap_or_default();
                stack.push((Open::Boxed(title, attr(&words, "border"), attr(&words, "bg"), attr(&words, "fg")), vec![vec![]]));
            }
            "cols" => stack.push((Open::Cols(words.iter().filter_map(|w| w.trim_end_matches('%').parse().ok()).collect()), vec![vec![]])),
            "next" => {
                if let Some((_, lists)) = stack.last_mut() {
                    lists.push(vec![]);
                }
            }
            "end" => {
                if let Some((o, lists)) = stack.pop() {
                    node = Some(close(o, lists));
                }
            }
            "h1" | "h2" | "h3" | "h4" => node = Some(Node::Text(Line::H(cmd.as_bytes()[1] - b'0'), bits(rest, nlinks))),
            "p" => node = Some(Node::Text(Line::P, bits(rest, nlinks))),
            "li" => node = Some(Node::Text(Line::Li, bits(rest, nlinks))),
            "center" => node = Some(Node::Text(Line::Center, bits(rest, nlinks))),
            "right" => node = Some(Node::Text(Line::Right, bits(rest, nlinks))),
            "menu" => node = Some(Node::Text(Line::Menu, bits(rest, nlinks))),
            "logo" => node = Some(Node::Logo),
            "search" => node = Some(Node::Search),
            "rule" => node = Some(Node::Rule(words.first().and_then(|w| page::colour(w)))),
            "gap" => node = Some(Node::Gap),
            _ if cmd.starts_with('@') => {
                // "@12", "@12-40", or several: "@3 @5-9"
                for w in line.split_whitespace() {
                    let w = w.trim_start_matches('@');
                    let (a, b) = w.split_once('-').unwrap_or((w, w));
                    if let (Ok(a), Ok(b)) = (a.parse::<usize>(), b.parse::<usize>()) {
                        if a < nitems && a <= b {
                            let n = Node::Refs(a, b.min(nitems - 1));
                            match stack.last_mut() {
                                Some((_, lists)) => lists.last_mut().unwrap().push(n),
                                None => top.push(n),
                            }
                        }
                    }
                }
            }
            _ => {}
        }
        if let Some(n) = node {
            match stack.last_mut() {
                Some((_, lists)) => lists.last_mut().unwrap().push(n),
                None => top.push(n),
            }
        }
    }
    while let Some((o, lists)) = stack.pop() {
        let n = close(o, lists);
        match stack.last_mut() {
            Some((_, l)) => l.last_mut().unwrap().push(n),
            None => top.push(n),
        }
    }
    (head, top)
}

/// Inline text: [label](L5) links, **bold**, {#hex coloured words}.
fn bits(s: &str, nlinks: usize) -> Vec<Bit> {
    let mut out: Vec<Bit> = vec![];
    let (mut bold, mut fg) = (false, None);
    let mut cur = String::new();
    let flush = |out: &mut Vec<Bit>, cur: &mut String, bold: bool, fg: Option<Rgb>| {
        if !cur.is_empty() {
            out.push(Bit { text: std::mem::take(cur), link: None, bold, fg });
        }
    };
    let mut rest = s;
    while let Some(ch) = rest.chars().next() {
        if rest.starts_with("**") {
            flush(&mut out, &mut cur, bold, fg);
            bold = !bold;
            rest = &rest[2..];
            continue;
        }
        if ch == '[' {
            if let Some((label, after)) = rest[1..].split_once("](") {
                if let Some((target, tail)) = after.split_once(')') {
                    let n = target.trim().trim_start_matches(['L', 'l']).parse::<usize>().ok().filter(|n| *n < nlinks);
                    if !label.contains('\n') && !label.contains('[') {
                        flush(&mut out, &mut cur, bold, fg);
                        out.push(Bit { text: label.replace(' ', "\u{a0}"), link: n, bold, fg });
                        rest = tail;
                        continue;
                    }
                }
            }
        }
        if ch == '{' && rest[1..].starts_with('#') {
            let end = rest.find(' ').unwrap_or(rest.len());
            if let Some(c) = page::colour(&rest[1..end]) {
                flush(&mut out, &mut cur, bold, fg);
                fg = Some(c);
                rest = rest[end..].strip_prefix(' ').unwrap_or(&rest[end..]);
                continue;
            }
        }
        if ch == '}' && fg.is_some() {
            flush(&mut out, &mut cur, bold, fg);
            fg = None;
            rest = &rest[1..];
            continue;
        }
        cur.push(ch);
        rest = &rest[ch.len_utf8()..];
    }
    flush(&mut out, &mut cur, bold, fg);
    out
}

// ------------------------------------------------------------ drawing it

#[derive(Clone, Copy)]
struct Paint {
    fg: Rgb,
    bg: Rgb,
    link: Rgb,
}

impl Paint {
    fn dim(&self) -> K {
        K::Ink(mix(self.fg, self.bg, 0.45), self.bg, false)
    }
    fn on(&self, bg: Option<Rgb>, fg: Option<Rgb>) -> Paint {
        let bg = bg.unwrap_or(self.bg);
        let fg = ink(fg.unwrap_or(self.fg), bg);
        Paint { fg, bg, link: ink(if bg == self.bg { self.link } else { fg }, bg) }
    }
}

pub struct Draw<'a> {
    pub view: &'a View,
    pub doc: &'a Doc,
    /// the page's full width, for bands
    pub full: (i32, i32),
}

impl Draw<'_> {
    pub fn page(&self, l: &mut Lay) {
        let p = Paint { fg: self.view.fg, bg: self.view.bg, link: self.view.link };
        l.bg = Some(p.bg);
        self.nodes(l, &self.view.nodes, p, true);
    }

    fn nodes(&self, l: &mut Lay, ns: &[Node], p: Paint, top: bool) {
        for n in ns {
            self.node(l, n, p, top);
        }
    }

    fn node(&self, l: &mut Lay, n: &Node, p: Paint, top: bool) {
        match n {
            Node::Band { bg, fg, kids } => {
                let q = p.on(*bg, *fg);
                let start = l.lines.len();
                l.lines.push(vec![]);
                l.gap = true;
                let was = l.bg.replace(q.bg);
                self.nodes(l, kids, q, false);
                l.bg = was;
                while l.lines.len() > start + 1 && l.lines.last().is_some_and(|x| x.is_empty()) {
                    l.lines.pop();
                }
                l.lines.push(vec![]);
                let (fx, fw) = if top { self.full } else { (l.x0, l.cw) };
                for line in &mut l.lines[start..] {
                    line.insert(0, Seg { x: fx, w: fw, what: What::Fill(q.bg) });
                }
                l.gap = false;
            }
            Node::Boxed { title, border, bg, fg, kids } => {
                let q = p.on(*bg, *fg);
                let bc = ink(border.unwrap_or(mix(p.fg, p.bg, 0.6)), q.bg);
                let (x0, cw) = (l.x0, l.cw);
                if cw < 8 {
                    return self.nodes(l, kids, q, false);
                }
                let mut inner = Lay { lines: vec![], x0: x0 + 2, cw: cw - 4, gap: true, bg: Some(q.bg) };
                self.nodes(&mut inner, kids, q, false);
                while inner.lines.last().is_some_and(|x| x.is_empty()) {
                    inner.lines.pop();
                }
                let k = K::Ink(bc, q.bg, false);
                let fill = |line: &mut Vec<Seg>| {
                    if bg.is_some() {
                        line.insert(0, Seg { x: x0, w: cw, what: What::Fill(q.bg) });
                    }
                };
                let t = if title.is_empty() { String::new() } else { format!(" {title} ") };
                let tw = (t.width() as i32).min(cw - 4);
                let mut first = vec![Seg { x: x0, w: cw, what: What::Txt { s: format!("┌─{}┐", "─".repeat((cw - 3) as usize)), k, link: None } }];
                if tw > 0 {
                    first.push(Seg { x: x0 + 2, w: tw, what: What::Txt { s: t, k: K::Ink(q.fg, q.bg, true), link: None } });
                }
                fill(&mut first);
                l.blank();
                l.lines.push(first);
                for mut line in inner.lines {
                    line.insert(0, Seg { x: x0, w: 1, what: What::Txt { s: "│".into(), k, link: None } });
                    line.insert(1, Seg { x: x0 + cw - 1, w: 1, what: What::Txt { s: "│".into(), k, link: None } });
                    fill(&mut line);
                    l.lines.push(line);
                }
                let mut last = vec![Seg { x: x0, w: cw, what: What::Txt { s: format!("└{}┘", "─".repeat((cw - 2) as usize)), k, link: None } }];
                fill(&mut last);
                l.lines.push(last);
                l.gap = false;
            }
            Node::Cols { widths, cols } => {
                let gaps = 3 * (cols.len() as i32 - 1);
                let room = l.cw - gaps;
                let sum: u32 = widths.iter().sum::<u32>().max(1);
                // too narrow for columns: one under the other
                if cols.len() < 2 || room / (cols.len() as i32) < 18 {
                    for c in cols {
                        self.nodes(l, c, p, false);
                    }
                    return;
                }
                let mut x = l.x0;
                let mut laid: Vec<Vec<Vec<Seg>>> = vec![];
                for (i, c) in cols.iter().enumerate() {
                    let w = if i + 1 == cols.len() { l.x0 + l.cw - x } else { (room as i64 * widths[i] as i64 / sum as i64) as i32 };
                    let mut inner = Lay { lines: vec![], x0: x, cw: w, gap: true, bg: l.bg };
                    self.nodes(&mut inner, c, p, false);
                    while inner.lines.last().is_some_and(|x| x.is_empty()) {
                        inner.lines.pop();
                    }
                    laid.push(inner.lines);
                    x += w + 3;
                }
                let n = laid.iter().map(Vec::len).max().unwrap_or(0);
                l.blank();
                for i in 0..n {
                    l.lines.push(laid.iter_mut().flat_map(|c| c.get_mut(i).map(std::mem::take).unwrap_or_default()).collect());
                }
                l.gap = false;
            }
            Node::Text(kind, bits) => self.text(l, *kind, bits, p),
            Node::Refs(a, b) => self.arrange(l, &(*a..=*b).collect::<Vec<_>>(), p, 0),
            Node::Logo => {
                if let Some(img) = &self.doc.logo {
                    let aspect = img.width() as f32 / img.height().max(1) as f32;
                    let rows = if aspect > 1.8 { 4 } else { 3 };
                    let cols = ((rows * 2) as f32 * aspect).round().clamp(4.0, l.cw.min(40) as f32) as u16;
                    let rows = ((cols as f32 / aspect) / 2.0).round().max(1.0) as u16;
                    l.pic(Pic::Logo, l.x0, cols, rows);
                }
            }
            Node::Search => {
                if !self.doc.page.forms.is_empty() {
                    l.form(0, &self.doc.page.forms[0], p.dim());
                }
            }
            Node::Rule(c) => l.rule("─", K::Ink(ink(c.unwrap_or(mix(p.fg, p.bg, 0.6)), p.bg), p.bg, false)),
            Node::Gap => {
                l.lines.push(vec![]);
                l.gap = true;
            }
        }
    }

    fn text(&self, l: &mut Lay, kind: Line, bits: &[Bit], p: Paint) {
        let head = matches!(kind, Line::H(_));
        let mut v: Vec<(String, K, Option<usize>)> = bits
            .iter()
            .map(|b| {
                let fg = match (b.link, b.fg) {
                    (_, Some(c)) => ink(c, p.bg),
                    (Some(_), None) => p.link,
                    _ => p.fg,
                };
                (b.text.clone(), K::Ink(fg, p.bg, b.bold || head), b.link)
            })
            .collect();
        if kind == Line::Menu {
            // links run together: space them out
            for piece in v.iter_mut().filter(|x| x.2.is_none() && x.0.trim().is_empty()) {
                piece.0 = "   ".into();
            }
        }
        let w: i32 = v.iter().map(|x| x.0.width() as i32).sum();
        match kind {
            Line::H(n) => {
                l.blank();
                if n == 1 {
                    v = v.into_iter().map(|(s, k, li)| (s.to_uppercase(), k, li)).collect();
                }
                l.spans(&v, 0, 0);
                l.blank();
            }
            Line::P => {
                l.spans(&v, 0, 0);
                l.blank();
            }
            Line::Li => {
                v.insert(0, ("• ".into(), p.dim(), None));
                l.spans(&v, 0, 2);
            }
            Line::Center | Line::Right if w < l.cw => {
                let ind = if kind == Line::Center { (l.cw - w) / 2 } else { l.cw - w };
                l.spans(&v, ind, 0);
            }
            _ => l.spans(&v, 0, 0),
        }
    }

    /// Lays boxes out as the page has them: in strips down the page, and
    /// where strips share columns (a grid of cards), as columns.
    fn arrange(&self, l: &mut Lay, ids: &[usize], p: Paint, depth: u8) {
        let all = &self.view.items;
        // each region is one or more strips
        let mut regions: Vec<(Vec<usize>, usize)> = vec![];
        let mut last_cols = 0;
        for strip in split(all, ids, false) {
            let n = split(all, &strip, true).len();
            match regions.last_mut() {
                Some((r, k)) if last_cols >= 2 && n >= 2 && split(all, &[r.as_slice(), strip.as_slice()].concat(), true).len() >= n.min(last_cols) => {
                    r.extend(strip);
                    *k += 1;
                    last_cols = split(all, r, true).len();
                }
                _ => {
                    regions.push((strip, 1));
                    last_cols = n;
                }
            }
        }
        for (mut region, strips) in regions {
            region.sort();
            let cols = split(all, &region, true);
            let short = |i: usize| {
                let it = &all[i];
                !matches!(it.tag.as_str(), "img" | "input") && it.h <= it.size * 5 / 2
            };
            if cols.len() == 1 || depth >= 3 {
                self.run(l, &region, p);
                continue;
            }
            if cols.iter().all(|c| c.len() == 1 && short(c[0])) {
                // a row of single lines: a number and its title, a menu
                let v: Vec<&Item> = cols.iter().map(|c| &all[c[0]]).collect();
                self.row(l, &v, p);
                continue;
            }
            let span = |c: &Vec<usize>| {
                let x0 = c.iter().map(|&i| all[i].x).min().unwrap_or(0);
                let x1 = c.iter().map(|&i| all[i].x + all[i].w).max().unwrap_or(0);
                (x1 - x0).max(1)
            };
            let total: i32 = cols.iter().map(span).sum();
            let room = l.cw - 2 * (cols.len() as i32 - 1);
            let widths: Vec<i32> = cols.iter().map(|c| (room as i64 * span(c) as i64 / total.max(1) as i64) as i32).collect();
            if cols.len() > 4 || widths.iter().any(|w| *w < 12) {
                if strips > 1 {
                    // rows of a table, say: each row by itself
                    for strip in split(all, &region, false) {
                        self.arrange(l, &strip, p, depth + 1);
                    }
                } else {
                    // too narrow to sit side by side: one under the other
                    for c in cols {
                        self.arrange(l, &c, p, depth + 1);
                    }
                }
                continue;
            }
            let mut x = l.x0;
            let mut laid: Vec<Vec<Vec<Seg>>> = vec![];
            for (n, c) in cols.into_iter().enumerate() {
                let w = if n + 1 == widths.len() { l.x0 + l.cw - x } else { widths[n] };
                let mut inner = Lay { lines: vec![], x0: x, cw: w, gap: true, bg: l.bg };
                self.arrange(&mut inner, &c, p, depth + 1);
                while inner.lines.last().is_some_and(|x| x.is_empty()) {
                    inner.lines.pop();
                }
                laid.push(inner.lines);
                x += w + 2;
            }
            let n = laid.iter().map(Vec::len).max().unwrap_or(0);
            l.blank();
            for i in 0..n {
                l.lines.push(laid.iter_mut().flat_map(|c| c.get_mut(i).map(std::mem::take).unwrap_or_default()).collect());
            }
            l.gap = false;
            l.blank();
        }
    }

    /// Boxes one after another, joining up ones side by side on a line.
    fn run(&self, l: &mut Lay, ids: &[usize], p: Paint) {
        let all = &self.view.items;
        let mut i = 0;
        while i < ids.len() {
            let mut j = i + 1;
            while j < ids.len() && beside(&all[ids[j - 1]], &all[ids[j]]) {
                j += 1;
            }
            if j - i > 1 {
                let v: Vec<&Item> = ids[i..j].iter().map(|&k| &all[k]).collect();
                self.row(l, &v, p);
            } else {
                self.item(l, &all[ids[i]], p);
            }
            i = j;
        }
    }

    fn pieces(&self, it: &Item, p: Paint, head: bool) -> Vec<(String, K, Option<usize>)> {
        let fg = ink(it.fg, p.bg);
        it.spans
            .iter()
            .map(|s| {
                let c = if s.link.is_some() && it.fg == p.fg { p.link } else { fg };
                (s.text.clone(), K::Ink(c, p.bg, s.bold || it.bold || head), s.link)
            })
            .collect()
    }

    fn row(&self, l: &mut Lay, items: &[&Item], p: Paint) {
        let mut v = vec![];
        for (n, it) in items.iter().enumerate() {
            if n > 0 {
                v.push(("  ".to_string(), p.dim(), None));
            }
            v.extend(self.pieces(it, p, false));
        }
        l.spans(&v, 0, 0);
    }

    fn item(&self, l: &mut Lay, it: &Item, p: Paint) {
        match it.tag.as_str() {
            "img" => match it.img.and_then(|i| Some((i, self.doc.imgs.get(i)?.as_ref()?))) {
                Some((i, im)) => {
                    // pictures are small here: text is what a terminal is for
                    let cols = (it.w / 14).clamp(8.min(l.cw), l.cw.min(48)) as u16;
                    let rows = ((cols as f32 * im.height() as f32 / im.width().max(1) as f32) / 2.0).round().clamp(1.0, 14.0) as u16;
                    let cols = if rows == 14 { ((28.0 * im.width() as f32 / im.height().max(1) as f32).round() as u16).clamp(4, cols) } else { cols };
                    l.pic(Pic::Img(i), l.x0 + (l.cw - cols as i32) / 2, cols, rows);
                    l.blank();
                }
                None if !it.alt.is_empty() => l.spans(&[(format!("▨ {}", it.alt), p.dim(), None)], 0, 2),
                None => {}
            },
            "input" => {
                if let Some(f) = self.doc.page.forms.first() {
                    l.form(0, f, p.dim());
                } else {
                    l.spans(&[(format!("[ {} ]", it.alt), p.dim(), None)], 0, 0);
                }
            }
            tag => {
                let len: usize = it.spans.iter().map(|s| s.text.len()).sum();
                let head = tag.len() == 2 && tag.starts_with('h') && tag.as_bytes()[1].is_ascii_digit() || it.size >= 22 || it.size >= 18 && it.bold && len < 80;
                let mut v = self.pieces(it, p, head);
                if head {
                    l.blank();
                }
                let hang = if tag == "li" {
                    v.insert(0, ("• ".into(), p.dim(), None));
                    2
                } else {
                    0
                };
                l.spans(&v, 0, hang);
                if head || len > 140 || matches!(tag, "p" | "blockquote" | "pre" | "figure" | "table" | "dl") {
                    l.blank();
                }
            }
        }
    }
}

/// Cuts boxes into strips down the page (or, `across`, columns) wherever a
/// gap runs right through; each part keeps the page's own order.
fn split(all: &[Item], ids: &[usize], across: bool) -> Vec<Vec<usize>> {
    let span = |i: usize| if across { (all[i].x, all[i].x + all[i].w) } else { (all[i].y, all[i].y + all[i].h) };
    let mut v = ids.to_vec();
    v.sort_by_key(|&i| span(i).0);
    let mut parts: Vec<Vec<usize>> = vec![];
    let mut end = i32::MIN;
    for i in v {
        let (a, b) = span(i);
        match parts.last_mut() {
            Some(p) if a < end - 2 => {
                p.push(i);
                end = end.max(b);
            }
            _ => {
                parts.push(vec![i]);
                end = b;
            }
        }
    }
    for p in &mut parts {
        p.sort();
    }
    parts
}

/// Whether a box sits just to the right of the one before, on the same line.
fn beside(a: &Item, b: &Item) -> bool {
    let text = |i: &Item| !matches!(i.tag.as_str(), "img" | "input") && !i.spans.is_empty() && i.size < 22 && i.h < 60;
    text(a) && text(b) && b.x >= a.x + a.w - 4 && (b.y - a.y).abs() <= 8 && b.x - (a.x + a.w) < 200
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_layout() {
        let (head, nodes) = parse("page bg=#ffffff fg=#202122 link=#3366cc\nband bg=#cc0000\nmenu [Home](L0) [News](L1)\nend\ncols 30 70\n@0-1\nnext\nh1 Hello **world**\nend\n", 2, 3);
        assert_eq!(head.0, Some((255, 255, 255)));
        assert!(matches!(&nodes[0], Node::Band { kids, .. } if kids.len() == 1));
        assert!(matches!(&nodes[1], Node::Cols { cols, .. } if cols.len() == 2));
        let b = bits("go [Home page](L1) now {#ff0000 red} **b**", 2);
        assert_eq!(b[1].link, Some(1));
        assert_eq!(b[3].fg, Some((255, 0, 0)));
        assert!(b.last().unwrap().bold);
    }
}
