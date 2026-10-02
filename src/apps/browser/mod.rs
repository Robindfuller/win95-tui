// A web browser that draws pages the desktop's way: your theme's colours, the
// site's own colour for its name, headings and rules, its logo and pictures in
// chunky pixels, and links you click. No JavaScript, so it suits reading:
// articles, wikis, forums, search results.
mod chrome;
mod fetch;
mod page;

use super::{Action, App, Launch};
use crate::{
    draw::{st, Canvas},
    icons::Icon,
    menu::{Cmd, Item, Sys},
    theme::{contrast, mix, Rgb, Theme},
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind};
use fetch::{Msg, Req};
use image::{imageops::FilterType, RgbaImage};
use page::{Block, Page, Span};
use ratatui::style::{Color, Modifier, Style};
use std::{
    collections::HashMap,
    sync::mpsc::{channel, Receiver, Sender},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
use url::Url;

pub const HOME: &str = "https://search.brave.com/";
const SEARCH: &str = "https://search.brave.com/search?q=";
/// pages read best no wider than this
const MAX_W: i32 = 100;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Pic {
    Logo,
    Img(usize),
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum K {
    Text,
    Bold,
    Em,
    Code,
    Dim,
    Brand,
    Quote,
}

#[derive(Clone, Debug)]
enum What {
    Txt { s: String, k: K, link: Option<usize> },
    Px { pic: Pic, row: u16, cols: u16, rows: u16 },
    Field(usize),
    Btn(usize, usize),
}

#[derive(Clone, Debug)]
struct Seg {
    x: i32,
    w: i32,
    what: What,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Hit {
    Link(usize),
    Field(usize),
    Btn(usize, usize),
}

/// One page in the history, with the pictures that have come in for it.
struct Doc {
    page: Page,
    logo: Option<RgbaImage>,
    imgs: Vec<Option<RgbaImage>>,
    /// all the pictures that are coming have come
    done: bool,
    scroll: i32,
    values: Vec<String>,
    /// the logo's own colour, when the page doesn't name one
    logo_brand: Option<Rgb>,
    scaled: HashMap<(Pic, u16), RgbaImage>,
}

impl Doc {
    fn new(page: Page) -> Doc {
        let values = page.forms.iter().map(|f| f.value.clone()).collect();
        let imgs = vec![None; page.imgs.len()];
        Doc { page, logo: None, imgs, done: false, scroll: 0, values, logo_brand: None, scaled: HashMap::new() }
    }

    fn pic(&self, p: Pic) -> Option<&RgbaImage> {
        match p {
            Pic::Logo => self.logo.as_ref(),
            Pic::Img(i) => self.imgs.get(i)?.as_ref(),
        }
    }
}

#[derive(PartialEq, Clone, Copy)]
enum Focus {
    Page,
    Addr,
    Form(usize),
}

#[derive(PartialEq, Clone, Copy)]
enum Nav {
    /// a new page on the history
    Push,
    /// the same page again
    Replace,
}

pub struct Browser {
    size: (u16, u16),
    ai: bool,
    agent: ureq::Agent,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    ticket: u64,
    loading: Option<(String, Nav)>,
    hist: Vec<Doc>,
    pos: usize,
    addr: String,
    /// typing replaces the address rather than adding to it
    addr_fresh: bool,
    focus: Focus,
    hover: Option<usize>,
    /// the link Tab moved to
    sel: Option<usize>,
    lines: Vec<Vec<Seg>>,
    laid: Option<(i32, usize, usize, bool)>,
    hits: Vec<(i32, i32, i32, Hit)>,
    brand: Rgb,
    drag_bar: bool,
}

fn rgb(c: Color) -> Rgb {
    match c {
        Color::Rgb(r, g, b) => (r, g, b),
        _ => (128, 128, 128),
    }
}

fn col(c: Rgb) -> Color {
    Color::Rgb(c.0, c.1, c.2)
}

/// Pulls a colour towards the text colour until it reads on the background.
fn readable(c: Rgb, bg: Rgb, text: Rgb) -> Rgb {
    let mut t = 0.0;
    let mut out = c;
    while contrast(out, bg) < 3.5 && t < 1.0 {
        t += 0.1;
        out = mix(c, text, t);
    }
    out
}

/// What the address bar means: a web address, a file, or something to search for.
fn resolve(s: &str) -> Option<Url> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if s.contains("://") || s.starts_with("about:") {
        return Url::parse(s).ok();
    }
    if s.starts_with('/') || s.starts_with('~') {
        let p = if let Some(rest) = s.strip_prefix('~') { format!("{}{rest}", crate::theme::home().display()) } else { s.to_string() };
        return Url::from_file_path(p).ok();
    }
    let first = s.split('/').next().unwrap_or("");
    if !s.contains(' ') && (first.contains('.') || first.starts_with("localhost")) {
        let scheme = if first.starts_with("localhost") || first.split(':').next().is_some_and(|h| h.parse::<std::net::Ipv4Addr>().is_ok()) { "http" } else { "https" };
        return Url::parse(&format!("{scheme}://{s}")).ok();
    }
    let mut u = Url::parse(SEARCH).ok()?;
    u.query_pairs_mut().clear().append_pair("q", s);
    Some(u)
}

/// Snaps a small picture to a few flat colours, with hard see-through edges.
fn posterise(img: &RgbaImage, k: usize) -> RgbaImage {
    let mut counts: HashMap<(u8, u8, u8), (u32, [u32; 3])> = HashMap::new();
    for p in img.pixels().filter(|p| p[3] >= 128) {
        let e = counts.entry((p[0] >> 4, p[1] >> 4, p[2] >> 4)).or_default();
        e.0 += 1;
        for i in 0..3 {
            e.1[i] += p[i] as u32;
        }
    }
    let mut seeds: Vec<_> = counts.values().collect();
    seeds.sort_by_key(|(n, _)| std::cmp::Reverse(*n));
    let mut cent: Vec<[f32; 3]> = seeds.iter().take(k).map(|(n, s)| [s[0] as f32 / *n as f32, s[1] as f32 / *n as f32, s[2] as f32 / *n as f32]).collect();
    if cent.is_empty() {
        return img.clone();
    }
    let near = |c: &[f32; 3], p: &image::Rgba<u8>| (0..3).map(|i| (c[i] - p[i] as f32).powi(2)).sum::<f32>();
    let best = |cent: &Vec<[f32; 3]>, p: &image::Rgba<u8>| (0..cent.len()).min_by(|&a, &b| near(&cent[a], p).total_cmp(&near(&cent[b], p))).unwrap();
    for _ in 0..5 {
        let mut sum = vec![([0f32; 3], 0u32); cent.len()];
        for p in img.pixels().filter(|p| p[3] >= 128) {
            let b = best(&cent, p);
            for i in 0..3 {
                sum[b].0[i] += p[i] as f32;
            }
            sum[b].1 += 1;
        }
        for (c, (s, n)) in cent.iter_mut().zip(sum) {
            if n > 0 {
                *c = [s[0] / n as f32, s[1] / n as f32, s[2] / n as f32];
            }
        }
    }
    let mut out = img.clone();
    for p in out.pixels_mut() {
        if p[3] < 128 {
            *p = image::Rgba([0, 0, 0, 0]);
        } else {
            let c = cent[best(&cent, p)];
            *p = image::Rgba([c[0] as u8, c[1] as u8, c[2] as u8, 255]);
        }
    }
    out
}

/// Logos, icons and drawings, as against photos: see-through bits or few colours.
fn drawn(img: &RgbaImage) -> bool {
    let n = (img.width() * img.height()).max(1);
    let clear = img.pixels().filter(|p| p[3] < 250).count() as u32;
    if clear * 20 > n {
        return true;
    }
    let mut seen = std::collections::HashSet::new();
    for p in img.pixels().step_by(3) {
        seen.insert((p[0] >> 4, p[1] >> 4, p[2] >> 4));
        if seen.len() > 40 {
            return false;
        }
    }
    true
}

/// The commonest lively colour in a logo.
fn logo_colour(img: &RgbaImage) -> Option<Rgb> {
    let mut counts: HashMap<(u8, u8, u8), (u32, [u32; 3])> = HashMap::new();
    for p in img.pixels().filter(|p| p[3] >= 200 && page::lively((p[0], p[1], p[2]))) {
        let e = counts.entry((p[0] >> 5, p[1] >> 5, p[2] >> 5)).or_default();
        e.0 += 1;
        for i in 0..3 {
            e.1[i] += p[i] as u32;
        }
    }
    let (n, s) = counts.values().max_by_key(|(n, _)| *n)?;
    Some(((s[0] / n) as u8, (s[1] / n) as u8, (s[2] / n) as u8))
}

impl Browser {
    pub fn new(url: Option<String>) -> Browser {
        let (tx, rx) = channel();
        let mut b = Browser {
            size: (0, 0),
            ai: false,
            agent: fetch::agent(),
            tx,
            rx,
            ticket: 0,
            loading: None,
            hist: vec![],
            pos: 0,
            addr: String::new(),
            addr_fresh: false,
            focus: Focus::Page,
            hover: None,
            sel: None,
            lines: vec![],
            laid: None,
            hits: vec![],
            brand: (128, 128, 128),
            drag_bar: false,
        };
        let start = url.as_deref().and_then(resolve).or_else(|| Url::parse(HOME).ok());
        if let Some(u) = start {
            b.open(Req::Get(u), Nav::Push);
        }
        b
    }

    fn doc(&self) -> Option<&Doc> {
        self.hist.get(self.pos)
    }

    fn doc_mut(&mut self) -> Option<&mut Doc> {
        self.hist.get_mut(self.pos)
    }

    fn open(&mut self, req: Req, into: Nav) {
        self.ticket += 1;
        let u = match &req {
            Req::Get(u) | Req::Post(u, _) => u.clone(),
        };
        self.addr = u.to_string();
        self.loading = Some((page::host(&u), into));
        self.focus = Focus::Page;
        fetch::load(self.agent.clone(), self.ticket, req, self.tx.clone());
    }

    fn go(&mut self, s: &str) {
        if let Some(u) = resolve(s) {
            self.open(Req::Get(u), Nav::Push);
        }
    }

    fn follow(&mut self, link: usize, new_tab: bool) -> Action {
        let Some(u) = self.doc().and_then(|d| d.page.links.get(link)).cloned() else { return Action::None };
        if new_tab {
            return Action::OpenTab(Launch::Browser(Some(u.to_string())));
        }
        // a jump within this page just goes to the top of it
        let here = self.doc().map(|d| d.page.url.clone());
        if here.is_some_and(|mut h| {
            h.set_fragment(None);
            let mut v = u.clone();
            v.set_fragment(None);
            h == v && u.fragment().is_some()
        }) {
            return Action::None;
        }
        self.open(Req::Get(u), Nav::Push);
        Action::None
    }

    fn submit(&mut self, f: usize, button: Option<usize>) {
        let Some(d) = self.doc() else { return };
        let Some(form) = d.page.forms.get(f) else { return };
        let mut pairs = form.hidden.clone();
        pairs.push((form.field.clone(), d.values.get(f).cloned().unwrap_or_default()));
        if let Some((n, v)) = button.and_then(|b| form.buttons.get(b)).and_then(|b| b.1.clone()) {
            pairs.push((n, v));
        }
        let req = if form.post {
            Req::Post(form.action.clone(), pairs)
        } else {
            let mut u = form.action.clone();
            u.query_pairs_mut().clear().extend_pairs(pairs.iter().map(|(k, v)| (k.as_str(), v.as_str())));
            Req::Get(u)
        };
        self.open(req, Nav::Push);
    }

    fn back(&mut self) {
        if self.pos > 0 {
            self.step_to(self.pos - 1);
        }
    }

    fn forward(&mut self) {
        if self.pos + 1 < self.hist.len() {
            self.step_to(self.pos + 1);
        }
    }

    fn step_to(&mut self, i: usize) {
        self.ticket += 1;
        self.loading = None;
        self.pos = i;
        self.addr = self.hist[i].page.url.to_string();
        self.sel = None;
        self.hover = None;
        self.laid = None;
    }

    fn refresh(&mut self) {
        if let Some(u) = self.doc().map(|d| d.page.url.clone()) {
            self.open(Req::Get(u), Nav::Replace);
        }
    }

    fn page_area(&self) -> (i32, i32, i32) {
        // top, height, width (less the scroll bar)
        (2, self.size.1 as i32 - 3, self.size.0 as i32 - 1)
    }

    fn max_scroll(&self) -> i32 {
        (self.lines.len() as i32 - self.page_area().1).max(0)
    }

    fn scroll_by(&mut self, n: i32) {
        let max = self.max_scroll();
        if let Some(d) = self.doc_mut() {
            d.scroll = (d.scroll + n).clamp(0, max);
        }
    }

    fn scroll(&self) -> i32 {
        self.doc().map_or(0, |d| d.scroll)
    }

    // ------------------------------------------------------------ layout

    fn layout(&mut self, th: &Theme) {
        let pw = self.page_area().2;
        let Some(d) = self.hist.get(self.pos) else {
            self.lines.clear();
            return;
        };
        let key = (pw, d.imgs.iter().filter(|i| i.is_some()).count(), self.pos, d.logo.is_some() || d.done);
        if self.laid == Some(key) && !self.lines.is_empty() {
            return;
        }
        self.laid = Some(key);
        let page_brand = d.page.brand.or(d.logo_brand).unwrap_or(rgb(th.accent));
        self.brand = readable(page_brand, rgb(th.client), rgb(th.text));
        let cw = (pw - 4).clamp(10, MAX_W);
        let x0 = (pw - cw) / 2;
        let mut l = Lay { lines: vec![], x0, cw, gap: true };

        // the site's own banner
        let banner = d.logo.is_some() || !d.page.nav.is_empty();
        if banner {
            l.blank();
            let mut name_x = x0;
            if let Some(img) = &d.logo {
                let aspect = img.width() as f32 / img.height().max(1) as f32;
                let rows = if aspect > 1.8 { 5 } else { 3 };
                let cols = ((rows * 2) as f32 * aspect).round().clamp(4.0, cw.min(48) as f32) as u16;
                let rows = ((cols as f32 / aspect) / 2.0).round().max(1.0) as u16;
                l.pic(Pic::Logo, x0, cols, rows);
                if aspect < 1.8 {
                    // a square icon: the site's name goes beside it
                    let n = l.lines.len() - rows as usize + (rows as usize - 1) / 2;
                    l.lines[n].push(Seg { x: x0 + cols as i32 + 2, w: d.page.site.width() as i32, what: What::Txt { s: d.page.site.clone(), k: K::Brand, link: None } });
                }
                name_x = -1;
            }
            if name_x >= 0 {
                l.spans(&[(d.page.site.clone(), K::Brand, None)], 0, 0);
            }
            if !d.page.nav.is_empty() {
                l.list(&d.page.nav, K::Text);
            }
            l.rule("━", K::Brand);
            l.gap = false;
            l.blank();
        }

        for b in &d.page.blocks {
            match b {
                Block::Heading(lv, spans) => {
                    l.blank();
                    let k = if *lv <= 2 { K::Brand } else { K::Bold };
                    l.spans(&styled(spans, Some(k)), 0, 0);
                    l.blank();
                }
                Block::Para(spans) => {
                    l.spans(&styled(spans, None), 0, 0);
                    l.blank();
                }
                Block::Item { depth, bullet, spans } => {
                    let ind = *depth as i32 * 3;
                    let bw = bullet.width().max(1) as i32 + 1;
                    if !bullet.is_empty() {
                        l.gap = false;
                    }
                    let mut v = vec![(format!("{bullet} "), K::Dim, None)];
                    if bullet.is_empty() {
                        v[0].0 = " ".repeat(bw as usize);
                    }
                    v.extend(styled(spans, None));
                    l.spans(&v, ind, ind + bw);
                    l.gap = true;
                }
                Block::Quote(spans) => {
                    let mut v = vec![("│ ".to_string(), K::Dim, None)];
                    v.extend(styled(spans, Some(K::Quote)));
                    l.spans(&v, 0, 2);
                    l.blank();
                }
                Block::Pre(t) => {
                    for line in t.lines() {
                        let s: String = line.replace('\t', "    ");
                        l.lines.push(vec![Seg { x: x0, w: s.width() as i32, what: What::Txt { s, k: K::Code, link: None } }]);
                    }
                    l.gap = false;
                    l.blank();
                }
                Block::Image { img, alt, link } => {
                    let (_, dw, _) = d.page.imgs[*img];
                    match d.imgs.get(*img).and_then(|i| i.as_ref()) {
                        // site icons next to search results and the like
                        Some(im) if dw.is_none() && im.width() < 64 && im.height() < 64 => {}
                        Some(im) => {
                            let want = dw.unwrap_or(im.width()) as i32 / 8;
                            // a picture that's a link is a thumbnail: keep it small
                            let most = if link.is_some() { cw.min(24) } else { cw };
                            let cols = want.clamp(12.min(most), most) as u16;
                            let rows = ((cols as f32 * im.height() as f32 / im.width().max(1) as f32) / 2.0).round().max(1.0) as u16;
                            let rows = rows.min(40);
                            let cols = if rows == 40 { ((80.0 * im.width() as f32 / im.height().max(1) as f32).round() as u16).max(4) } else { cols };
                            let x = x0 + (cw - cols as i32) / 2;
                            let start = l.lines.len();
                            l.pic(Pic::Img(*img), x, cols, rows);
                            if let Some(k) = link {
                                for line in &mut l.lines[start..] {
                                    line.push(Seg { x, w: cols as i32, what: What::Txt { s: String::new(), k: K::Text, link: Some(*k) } });
                                }
                            }
                            l.blank();
                        }
                        None if !d.done || !alt.is_empty() => {
                            let s = if alt.is_empty() { "▨ picture".to_string() } else { format!("▨ {alt}") };
                            l.spans(&[(s, K::Dim, *link)], 0, 2);
                            l.blank();
                        }
                        None => {}
                    }
                }
                Block::Rule => l.rule("─", K::Dim),
                Block::Form(f) => {
                    let form = &d.page.forms[*f];
                    let fw = cw.min(60);
                    let fx = x0 + (cw - fw) / 2;
                    l.blank();
                    let label = format!("┌─ {} ", form.label);
                    let top = format!("{label}{}┐", "─".repeat((fw - label.width() as i32 - 1).max(0) as usize));
                    l.lines.push(vec![Seg { x: fx, w: fw, what: What::Txt { s: top, k: K::Dim, link: None } }]);
                    l.lines.push(vec![
                        Seg { x: fx, w: 1, what: What::Txt { s: "│".into(), k: K::Dim, link: None } },
                        Seg { x: fx + 2, w: fw - 4, what: What::Field(*f) },
                        Seg { x: fx + fw - 1, w: 1, what: What::Txt { s: "│".into(), k: K::Dim, link: None } },
                    ]);
                    l.lines.push(vec![Seg { x: fx, w: fw, what: What::Txt { s: format!("└{}┘", "─".repeat((fw - 2) as usize)), k: K::Dim, link: None } }]);
                    let labels: Vec<String> = form.buttons.iter().map(|b| format!("[ {} ]", b.0)).collect();
                    let total: i32 = labels.iter().map(|s| s.width() as i32).sum::<i32>() + 3 * (labels.len() as i32 - 1);
                    let mut bx = x0 + (cw - total) / 2;
                    let mut row = vec![];
                    for (i, s) in labels.into_iter().enumerate() {
                        let w = s.width() as i32;
                        row.push(Seg { x: bx, w, what: What::Btn(*f, i) });
                        bx += w + 3;
                    }
                    l.lines.push(vec![]);
                    l.lines.push(row);
                    l.gap = false;
                    l.blank();
                }
            }
        }

        if !d.page.foot.is_empty() {
            l.blank();
            l.rule("─", K::Dim);
            l.list(&d.page.foot, K::Dim);
        }
        while l.lines.last().is_some_and(|x| x.is_empty()) {
            l.lines.pop();
        }
        self.lines = l.lines;
    }

    // ------------------------------------------------------------ drawing

    fn style(&self, k: K, th: &Theme) -> Style {
        let bg = th.client;
        match k {
            K::Text => st(th.text, bg),
            K::Bold => st(th.text, bg).add_modifier(Modifier::BOLD),
            K::Em => st(th.text, bg).add_modifier(Modifier::ITALIC),
            K::Code => st(th.yellow, bg),
            K::Dim => st(th.dim, bg),
            K::Brand => st(col(self.brand), bg).add_modifier(Modifier::BOLD),
            K::Quote => st(th.dim, bg).add_modifier(Modifier::ITALIC),
        }
    }

    fn scaled(&mut self, pic: Pic, cols: u16, rows: u16, th: &Theme) -> Option<RgbaImage> {
        let d = self.hist.get_mut(self.pos)?;
        if let Some(i) = d.scaled.get(&(pic, cols)) {
            return Some(i.clone());
        }
        let src = d.pic(pic)?;
        let small = image::imageops::resize(src, cols as u32, rows as u32 * 2, FilterType::Triangle);
        let mut small = if pic == Pic::Logo || drawn(src) { posterise(&small, 6) } else { small };
        // a dark logo on a dark page (or light on light) takes the text colour
        if pic == Pic::Logo || drawn(src) {
            let (bg, text) = (rgb(th.client), rgb(th.text));
            for p in small.pixels_mut().filter(|p| p[3] == 255) {
                if contrast((p[0], p[1], p[2]), bg) < 1.6 {
                    *p = image::Rgba([text.0, text.1, text.2, 255]);
                }
            }
        }
        d.scaled.insert((pic, cols), small.clone());
        Some(small)
    }

    fn draw_page(&mut self, c: &mut Canvas, th: &Theme) {
        let (top, ph, pw) = self.page_area();
        c.fill(0, top, pw, ph, st(th.text, th.client));
        self.hits.clear();
        let scroll = self.scroll();
        let bg = rgb(th.client);
        let blend = |p: &image::Rgba<u8>| {
            let a = p[3] as f32 / 255.0;
            let m = |v: u8, b: u8| (v as f32 * a + b as f32 * (1.0 - a)).round() as u8;
            Color::Rgb(m(p[0], bg.0), m(p[1], bg.1), m(p[2], bg.2))
        };
        for row in 0..ph {
            let Some(line) = self.lines.get((scroll + row) as usize).cloned() else { break };
            let y = top + row;
            for seg in line {
                match seg.what {
                    What::Txt { s, k, link } => {
                        let mut style = self.style(k, th);
                        if let Some(li) = link {
                            if !s.is_empty() {
                                style = style.fg(th.blue);
                                if self.sel == Some(li) {
                                    style = th.sel();
                                } else if self.hover == Some(li) {
                                    style = style.add_modifier(Modifier::UNDERLINED);
                                }
                            }
                            self.hits.push((seg.x, y, seg.w, Hit::Link(li)));
                        }
                        c.text_max(seg.x, y, &s, style, pw);
                    }
                    What::Px { pic, row: r, cols, rows } => {
                        if let Some(img) = self.scaled(pic, cols, rows, th) {
                            for x in 0..cols as u32 {
                                let (t, b) = (img.get_pixel(x, r as u32 * 2), img.get_pixel(x, r as u32 * 2 + 1));
                                c.put(seg.x + x as i32, y, "▀", st(blend(t), blend(b)));
                            }
                        }
                    }
                    What::Field(f) => {
                        let v = self.doc().and_then(|d| d.values.get(f).cloned()).unwrap_or_default();
                        c.field(seg.x - 1, y, seg.w + 2, &v, th);
                        if self.focus != Focus::Form(f) && v.is_empty() {
                            c.put(seg.x, y, " ", st(th.text, th.client));
                        }
                        self.hits.push((seg.x - 1, y, seg.w + 2, Hit::Field(f)));
                    }
                    What::Btn(f, i) => {
                        let label = self.doc().and_then(|d| d.page.forms.get(f)?.buttons.get(i).map(|b| b.0.clone())).unwrap_or_default();
                        let s = st(th.text, th.client).add_modifier(Modifier::BOLD);
                        c.text(seg.x, y, &format!("[ {label} ]"), s);
                        self.hits.push((seg.x, y, seg.w, Hit::Btn(f, i)));
                    }
                }
            }
        }
        // the scroll bar
        let n = self.lines.len() as i32;
        c.fill(pw, top, 1, ph, st(th.text, th.tile_b));
        c.put(pw, top, "▲", st(th.text, th.button));
        c.put(pw, top + ph - 1, "▼", st(th.text, th.button));
        if n > ph && ph > 3 {
            let track = ph - 2;
            let len = (track * ph / n).max(1);
            let at = (track - len) * scroll / self.max_scroll().max(1);
            c.fill(pw, top + 1 + at, 1, len, st(th.text, th.button));
        }
    }

    fn hit(&self, x: i32, y: i32) -> Option<Hit> {
        self.hits.iter().find(|&&(hx, hy, hw, _)| y == hy && x >= hx && x < hx + hw).map(|h| h.3)
    }

    /// Moves the Tab selection to the next (or previous) link, scrolling to it.
    fn tab(&mut self, by: i32) {
        let mut order: Vec<(usize, usize)> = vec![];
        for (i, line) in self.lines.iter().enumerate() {
            for s in line {
                if let What::Txt { link: Some(l), s: t, .. } = &s.what {
                    if !t.is_empty() && order.last().is_none_or(|o| o.1 != *l) {
                        order.push((i, *l));
                    }
                }
            }
        }
        if order.is_empty() {
            return;
        }
        let (scroll, ph) = (self.scroll(), self.page_area().1);
        let cur = self.sel.and_then(|s| order.iter().position(|o| o.1 == s && (o.0 as i32) >= scroll - 1));
        let next = match cur {
            Some(i) => (i as i32 + by).rem_euclid(order.len() as i32) as usize,
            None => order.iter().position(|o| o.0 as i32 >= scroll).unwrap_or(0),
        };
        let (line, l) = order[next];
        self.sel = Some(l);
        let line = line as i32;
        if line < scroll || line >= scroll + ph {
            let max = self.max_scroll();
            if let Some(d) = self.doc_mut() {
                d.scroll = (line - ph / 3).clamp(0, max);
            }
        }
    }

    fn status(&self) -> String {
        if let Some((host, _)) = &self.loading {
            return format!("Opening {host}…");
        }
        if let Some(u) = self.hover.or(self.sel).and_then(|l| self.doc()?.page.links.get(l)) {
            return u.to_string();
        }
        match self.doc() {
            Some(d) if !d.done => "Fetching pictures…".into(),
            _ => "Done".into(),
        }
    }
}

/// Lays text out in lines.
struct Lay {
    lines: Vec<Vec<Seg>>,
    x0: i32,
    cw: i32,
    /// the last line is blank already
    gap: bool,
}

type Piece = (String, K, Option<usize>);

fn styled(spans: &[Span], force: Option<K>) -> Vec<Piece> {
    spans
        .iter()
        .map(|s| {
            let k = force.unwrap_or(if s.code {
                K::Code
            } else if s.bold {
                K::Bold
            } else if s.em {
                K::Em
            } else {
                K::Text
            });
            (s.text.clone(), k, s.link)
        })
        .collect()
}

impl Lay {
    fn blank(&mut self) {
        if !self.gap {
            self.lines.push(vec![]);
            self.gap = true;
        }
    }

    fn rule(&mut self, ch: &str, k: K) {
        self.lines.push(vec![Seg { x: self.x0, w: self.cw, what: What::Txt { s: ch.repeat(self.cw as usize), k, link: None } }]);
        self.gap = false;
    }

    fn pic(&mut self, pic: Pic, x: i32, cols: u16, rows: u16) {
        for row in 0..rows {
            self.lines.push(vec![Seg { x, w: cols as i32, what: What::Px { pic, row, cols, rows } }]);
        }
        self.gap = false;
    }

    /// Links in a row with bars between, wrapping as needed.
    fn list(&mut self, items: &[(String, usize)], k: K) {
        let mut v: Vec<Piece> = vec![];
        for (i, (t, l)) in items.iter().enumerate() {
            if i > 0 {
                v.push((" │ ".into(), K::Dim, None));
            }
            v.push((t.replace(' ', "\u{a0}"), k, Some(*l)));
        }
        self.spans(&v, 0, 0);
    }

    /// Word-wraps styled text: `ind` on the first line, `hang` after.
    fn spans(&mut self, pieces: &[Piece], ind: i32, hang: i32) {
        let right = self.x0 + self.cw;
        let mut line: Vec<Seg> = vec![];
        let mut x = self.x0 + ind;
        let start_of_line = |x: i32, hang: i32, x0: i32| x == x0 + hang;
        let push = |line: &mut Vec<Seg>, x: i32, s: &str, k: K, link: Option<usize>| {
            let w = s.width() as i32;
            if let Some(Seg { what: What::Txt { s: ps, k: pk, link: pl }, w: pw, x: px }) = line.last_mut() {
                if *pk == k && *pl == link && *px + *pw == x {
                    ps.push_str(s);
                    *pw += w;
                    return;
                }
            }
            line.push(Seg { x, w, what: What::Txt { s: s.into(), k, link } });
        };
        for (text, k, link) in pieces {
            for word in text.split_inclusive(' ') {
                let ww = word.trim_end().width() as i32;
                if x + ww > right && !start_of_line(x, hang, self.x0) && !line.is_empty() {
                    self.lines.push(std::mem::take(&mut line));
                    x = self.x0 + hang;
                }
                let word = if start_of_line(x, hang, self.x0) && !line.is_empty() || x == self.x0 + hang && word.trim().is_empty() { word.trim_start() } else { word };
                if word.is_empty() {
                    continue;
                }
                // a word longer than the line is cut
                let mut rest = word;
                while (rest.trim_end().width() as i32) > right - x && right - x > 0 {
                    let mut taken = 0;
                    let mut cut = 0;
                    for (i, ch) in rest.char_indices() {
                        let cw = ch.width().unwrap_or(0) as i32;
                        if taken + cw > right - x {
                            break;
                        }
                        taken += cw;
                        cut = i + ch.len_utf8();
                    }
                    if cut == 0 {
                        break;
                    }
                    push(&mut line, x, &rest[..cut], *k, *link);
                    self.lines.push(std::mem::take(&mut line));
                    x = self.x0 + hang;
                    rest = &rest[cut..];
                }
                let w = rest.width() as i32;
                push(&mut line, x, rest, *k, *link);
                x += w;
            }
        }
        if !line.is_empty() {
            // trailing spaces shouldn't stretch a link's hover area
            if let Some(Seg { what: What::Txt { s, .. }, w, .. }) = line.last_mut() {
                let t = s.trim_end().to_string();
                *w = t.width() as i32;
                *s = t;
            }
            self.lines.push(line);
        }
        self.gap = false;
    }
}

impl App for Browser {
    fn title(&self) -> String {
        match self.doc() {
            Some(d) if !d.page.title.is_empty() => format!("{} - Browser", d.page.title),
            _ => "Browser".into(),
        }
    }
    fn tab_title(&self) -> String {
        self.doc().map(|d| if d.page.title.is_empty() { d.page.site.clone() } else { d.page.title.clone() }).unwrap_or_else(|| "Browser".into())
    }
    fn icon(&self) -> Icon {
        Icon::Browser
    }
    fn size_hint(&self) -> (u16, u16) {
        (104, 34)
    }
    fn resize(&mut self, w: u16, h: u16) {
        self.size = (w, h);
        self.laid = None;
    }
    fn new_tab(&self) -> Option<Launch> {
        Some(Launch::Browser(None))
    }

    fn poll(&mut self) -> (bool, Action) {
        let mut dirty = false;
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::Page(g, page) if g == self.ticket => {
                    let into = self.loading.take().map_or(Nav::Push, |l| l.1);
                    self.addr = page.url.to_string();
                    let doc = Doc::new(*page);
                    if into == Nav::Replace && !self.hist.is_empty() {
                        self.hist[self.pos] = doc;
                    } else {
                        self.hist.truncate(self.pos + 1);
                        self.hist.push(doc);
                        self.pos = self.hist.len() - 1;
                    }
                    self.sel = None;
                    self.hover = None;
                    self.laid = None;
                    // a search page puts you straight in its box
                    if let Some(d) = self.doc() {
                        if d.page.blocks.iter().take(6).any(|b| matches!(b, Block::Form(_))) && d.page.forms.len() == 1 && d.values[0].is_empty() {
                            self.focus = Focus::Form(0);
                        }
                    }
                }
                Msg::Logo(g, img) if g == self.ticket => {
                    if let Some(d) = self.doc_mut() {
                        d.logo_brand = logo_colour(&img);
                        d.logo = Some(img);
                        d.scaled.clear();
                    }
                    self.laid = None;
                }
                Msg::Img(g, i, img) if g == self.ticket => {
                    if let Some(slot) = self.doc_mut().and_then(|d| d.imgs.get_mut(i)) {
                        *slot = Some(img);
                    }
                }
                Msg::Done(g) if g == self.ticket => {
                    if let Some(d) = self.doc_mut() {
                        d.done = true;
                    }
                    self.laid = None;
                }
                _ => continue,
            }
            dirty = true;
        }
        (dirty, Action::None)
    }

    fn render(&mut self, c: &mut Canvas, th: &Theme, _focused: bool) {
        let (w, h) = (self.size.0 as i32, self.size.1 as i32);
        if w < 20 || h < 6 {
            return;
        }
        self.layout(th);
        let max = self.max_scroll();
        if let Some(d) = self.doc_mut() {
            d.scroll = d.scroll.min(max);
        }
        c.fill(0, 0, w, h, st(th.text, th.face));

        // toolbar
        let can_back = self.pos > 0;
        let can_fwd = self.pos + 1 < self.hist.len();
        let reload = if self.loading.is_some() { "✕ Stop" } else { "⟳ Refresh" };
        let mut x = 1;
        for (label, bw, on) in [("◂ Back", 8, can_back), ("Forward ▸", 11, can_fwd), (reload, 11, true), ("⌂ Home", 8, true)] {
            c.button(x, 0, bw, label, th, false, false);
            if !on {
                let lw = label.chars().count() as i32;
                c.text(x + (bw - lw) / 2, 0, label, st(th.dim, th.button));
            }
            x += bw + 1;
        }
        let chip = if self.ai { " ✦ AI-assisted " } else { " ✦ AI off " };
        let cs = if self.ai { th.sel() } else { st(th.dim, th.button) };
        c.text(w - chip.chars().count() as i32 - 1, 0, chip, cs);

        // address bar
        let ax = c.text(1, 1, "Address ", st(th.text, th.face));
        let fw = w - ax - 7;
        c.field(ax, 1, fw, "", th);
        c.text(ax + 1, 1, "◍", st(th.cyan, th.client));
        let shown: String = {
            let room = (fw - 5).max(0) as usize;
            let chars: Vec<char> = self.addr.chars().collect();
            let start = if self.focus == Focus::Addr { chars.len().saturating_sub(room) } else { 0 };
            chars[start..].iter().take(room).collect()
        };
        let astyle = if self.focus == Focus::Addr && self.addr_fresh { th.sel() } else { st(th.text, th.client) };
        c.text(ax + 3, 1, &shown, astyle);
        c.button(w - 6, 1, 5, "Go", th, true, false);

        self.draw_page(c, th);

        // status bar
        let sy = h - 1;
        c.fill(0, sy, w, 1, st(th.text, th.face));
        let host = self.doc().map(|d| d.page.site.clone()).unwrap_or_default();
        let zone = format!("◍ {host}  │  {}", if self.ai { "AI-assisted" } else { "plain mode" });
        let zx = w - zone.chars().count() as i32 - 2;
        c.text_max(1, sy, &self.status(), st(th.text, th.face), zx - 2);
        c.text(zx, sy, &zone, st(th.dim, th.face));
    }

    fn cursor(&self) -> Option<(u16, u16)> {
        match self.focus {
            Focus::Addr if !self.addr_fresh => {
                let ax = 9;
                let room = (self.size.0 as i32 - ax - 7 - 5).max(0);
                Some(((ax + 3 + (self.addr.chars().count() as i32).min(room)) as u16, 1))
            }
            Focus::Form(f) => {
                let (top, ph, _) = self.page_area();
                let scroll = self.scroll();
                let (i, seg) = self.lines.iter().enumerate().find_map(|(i, l)| l.iter().find(|s| matches!(s.what, What::Field(g) if g == f)).map(|s| (i, s)))?;
                let y = top + i as i32 - scroll;
                if y < top || y >= top + ph {
                    return None;
                }
                let v = self.doc()?.values.get(f)?.chars().count() as i32;
                Some(((seg.x + v.min(seg.w - 1)) as u16, y as u16))
            }
            _ => None,
        }
    }

    fn key(&mut self, k: KeyEvent) -> Action {
        let (ctrl, alt) = (k.modifiers.contains(KeyModifiers::CONTROL), k.modifiers.contains(KeyModifiers::ALT));
        match (k.code, ctrl, alt) {
            (KeyCode::Char('l'), true, _) | (KeyCode::F(6), ..) => {
                self.focus = Focus::Addr;
                self.addr_fresh = true;
                return Action::None;
            }
            (KeyCode::Left, _, true) => return self.command("back"),
            (KeyCode::Right, _, true) => return self.command("forward"),
            (KeyCode::F(5), ..) | (KeyCode::Char('r'), true, _) => return self.command("refresh"),
            _ => {}
        }
        match self.focus {
            Focus::Addr => match k.code {
                KeyCode::Enter => {
                    let a = self.addr.clone();
                    self.focus = Focus::Page;
                    self.go(&a);
                }
                KeyCode::Esc => {
                    self.focus = Focus::Page;
                    self.addr = self.doc().map(|d| d.page.url.to_string()).unwrap_or_default();
                }
                KeyCode::Backspace => {
                    if self.addr_fresh {
                        self.addr.clear();
                    } else {
                        self.addr.pop();
                    }
                    self.addr_fresh = false;
                }
                KeyCode::Char('u') if ctrl => self.addr.clear(),
                KeyCode::Left | KeyCode::Right | KeyCode::End | KeyCode::Home => self.addr_fresh = false,
                KeyCode::Char(ch) if !ctrl => {
                    if self.addr_fresh {
                        self.addr.clear();
                        self.addr_fresh = false;
                    }
                    self.addr.push(ch);
                }
                _ => {}
            },
            Focus::Form(f) => match k.code {
                KeyCode::Enter => self.submit(f, Some(0)),
                KeyCode::Esc | KeyCode::Tab => self.focus = Focus::Page,
                KeyCode::Backspace => {
                    if let Some(v) = self.doc_mut().and_then(|d| d.values.get_mut(f)) {
                        v.pop();
                    }
                }
                KeyCode::Char('u') if ctrl => {
                    if let Some(v) = self.doc_mut().and_then(|d| d.values.get_mut(f)) {
                        v.clear();
                    }
                }
                KeyCode::Char(ch) if !ctrl => {
                    if let Some(v) = self.doc_mut().and_then(|d| d.values.get_mut(f)) {
                        v.push(ch);
                    }
                }
                _ => {}
            },
            Focus::Page => {
                let ph = self.page_area().1;
                match k.code {
                    KeyCode::Up | KeyCode::Char('k') => self.scroll_by(-1),
                    KeyCode::Down | KeyCode::Char('j') => self.scroll_by(1),
                    KeyCode::PageUp => self.scroll_by(-(ph - 2)),
                    KeyCode::PageDown | KeyCode::Char(' ') => self.scroll_by(ph - 2),
                    KeyCode::Home | KeyCode::Char('g') => self.scroll_by(-i32::MAX / 2),
                    KeyCode::End | KeyCode::Char('G') => self.scroll_by(i32::MAX / 2),
                    KeyCode::Tab => self.tab(1),
                    KeyCode::BackTab => self.tab(-1),
                    KeyCode::Enter => {
                        if let Some(l) = self.sel {
                            return self.follow(l, false);
                        }
                    }
                    KeyCode::Backspace => self.back(),
                    KeyCode::Esc => {
                        if self.loading.take().is_some() {
                            self.ticket += 1;
                            self.addr = self.doc().map(|d| d.page.url.to_string()).unwrap_or_default();
                        } else {
                            self.sel = None;
                        }
                    }
                    _ => {}
                }
            }
        }
        Action::None
    }

    fn paste(&mut self, s: &str) {
        let s = s.lines().next().unwrap_or("");
        match self.focus {
            Focus::Addr => {
                if self.addr_fresh {
                    self.addr.clear();
                    self.addr_fresh = false;
                }
                self.addr.push_str(s);
            }
            Focus::Form(f) => {
                if let Some(v) = self.doc_mut().and_then(|d| d.values.get_mut(f)) {
                    v.push_str(s);
                }
            }
            Focus::Page => {
                self.focus = Focus::Addr;
                self.addr = s.to_string();
                self.addr_fresh = false;
            }
        }
    }

    fn mouse(&mut self, kind: MouseEventKind, x: i32, y: i32, _mods: KeyModifiers) -> Action {
        let (w, _) = (self.size.0 as i32, self.size.1 as i32);
        let (top, ph, pw) = self.page_area();
        match kind {
            MouseEventKind::ScrollUp => self.scroll_by(-3),
            MouseEventKind::ScrollDown => self.scroll_by(3),
            MouseEventKind::Moved => {
                self.hover = match self.hit(x, y) {
                    Some(Hit::Link(l)) => Some(l),
                    _ => None,
                };
            }
            MouseEventKind::Up(_) => self.drag_bar = false,
            MouseEventKind::Drag(MouseButton::Left) if self.drag_bar => {
                let max = self.max_scroll();
                let at = ((y - top - 1) * max / (ph - 2).max(1)).clamp(0, max);
                if let Some(d) = self.doc_mut() {
                    d.scroll = at;
                }
            }
            MouseEventKind::Down(b) => {
                // toolbar
                if y == 0 && b == MouseButton::Left {
                    let cmd = match x {
                        1..=8 => "back",
                        10..=20 => "forward",
                        22..=32 => "refresh",
                        34..=41 => "home",
                        _ if x >= w - 11 => "ai",
                        _ => "",
                    };
                    return self.command(cmd);
                }
                if y == 1 {
                    if x >= w - 6 {
                        let a = self.addr.clone();
                        self.go(&a);
                    } else if x >= 9 {
                        self.focus = Focus::Addr;
                        self.addr_fresh = true;
                    }
                    return Action::None;
                }
                if x == pw && y >= top && y < top + ph {
                    match y - top {
                        0 => self.scroll_by(-3),
                        r if r == ph - 1 => self.scroll_by(3),
                        _ => {
                            self.drag_bar = true;
                            let max = self.max_scroll();
                            let at = ((y - top - 1) * max / (ph - 2).max(1)).clamp(0, max);
                            if let Some(d) = self.doc_mut() {
                                d.scroll = at;
                            }
                        }
                    }
                    return Action::None;
                }
                match (self.hit(x, y), b) {
                    (Some(Hit::Link(l)), MouseButton::Left) => return self.follow(l, false),
                    (Some(Hit::Link(l)), MouseButton::Middle) => return self.follow(l, true),
                    (Some(Hit::Field(f)), _) => self.focus = Focus::Form(f),
                    (Some(Hit::Btn(f, i)), MouseButton::Left) => self.submit(f, Some(i)),
                    (None, MouseButton::Right) => {
                        let items = vec![
                            Item::new("Back", Cmd::App("back")).enabled(self.pos > 0),
                            Item::new("Forward", Cmd::App("forward")).enabled(self.pos + 1 < self.hist.len()),
                            Item::new("Refresh", Cmd::App("refresh")),
                        ];
                        return Action::Menu(items, x, y);
                    }
                    (Some(Hit::Link(l)), MouseButton::Right) => {
                        self.sel = Some(l);
                        let items = vec![Item::new("Open", Cmd::App("open-sel")), Item::new("Open in New Tab", Cmd::App("open-sel-tab"))];
                        return Action::Menu(items, x, y);
                    }
                    _ => self.focus = Focus::Page,
                }
            }
            _ => {}
        }
        Action::None
    }

    fn menubar(&self) -> Vec<(&'static str, Vec<Item>)> {
        vec![
            ("File", vec![
                Item::new("Open Address...", Cmd::App("address")).key("Ctrl+L"),
                Item::new("New Tab", Cmd::App("tab-new")).key("Ctrl+Shift+T"),
                Item::sep(),
                Item::new("Close", Cmd::Sys(Sys::Close)),
            ]),
            ("View", vec![
                Item::new("AI-assisted mode", Cmd::App("ai")).checked(self.ai),
                Item::sep(),
                Item::new("Refresh", Cmd::App("refresh")).key("F5"),
            ]),
            ("Go", vec![
                Item::new("Back", Cmd::App("back")).key("Alt+◂").enabled(self.pos > 0),
                Item::new("Forward", Cmd::App("forward")).key("Alt+▸").enabled(self.pos + 1 < self.hist.len()),
                Item::new("Home", Cmd::App("home")),
            ]),
            ("Help", vec![Item::new("Keys", Cmd::App("help")).icon(Icon::Help)]),
        ]
    }

    fn command(&mut self, cmd: &str) -> Action {
        match cmd {
            "back" => self.back(),
            "forward" => self.forward(),
            "refresh" => {
                if self.loading.take().is_some() {
                    self.ticket += 1;
                    self.addr = self.doc().map(|d| d.page.url.to_string()).unwrap_or_default();
                } else {
                    self.refresh();
                }
            }
            "home" => self.go(HOME),
            "address" => {
                self.focus = Focus::Addr;
                self.addr_fresh = true;
            }
            "tab-new" => return Action::OpenTab(Launch::Browser(None)),
            "open-sel" => {
                if let Some(l) = self.sel {
                    return self.follow(l, false);
                }
            }
            "open-sel-tab" => {
                if let Some(l) = self.sel {
                    return self.follow(l, true);
                }
            }
            "ai" => {
                self.ai = !self.ai;
            }
            "help" => {
                return Action::Launch(Launch::Msg {
                    title: "Browser keys".into(),
                    text: "Ctrl+L  type an address or a search\nTab     next link, Enter opens it\nAlt+◂ ▸ back and forward (or Backspace)\nF5      refresh, Esc stops\nSpace   page down\n\nMiddle-click a link for a new tab.\nAI-assisted mode isn't built yet.".into(),
                });
            }
            _ => {}
        }
        Action::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Draws a live page as text: BROWSE=<url> cargo test dump -- --ignored --nocapture
    #[test]
    #[ignore]
    fn dump() {
        let url = std::env::var("BROWSE").unwrap_or(HOME.into());
        let mut b = Browser::new(Some(url));
        b.resize(100, 60);
        let t = std::time::Instant::now();
        while t.elapsed().as_secs() < 25 && !b.doc().is_some_and(|d| d.done) {
            b.poll();
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let th = Theme::load();
        let mut buf = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 100, 60));
        b.render(&mut Canvas::new(&mut buf), &th, true);
        for y in 0..60 {
            let line: String = (0..100).map(|x| buf.cell((x, y)).unwrap().symbol().to_string()).collect();
            println!("{}", line.trim_end());
        }
        println!("-- {} lines, {} links, {} pictures, logo {}", b.lines.len(), b.doc().unwrap().page.links.len(), b.doc().unwrap().imgs.iter().filter(|i| i.is_some()).count(), b.doc().unwrap().logo.is_some());
    }
}
