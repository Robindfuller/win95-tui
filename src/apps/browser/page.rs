// Turns a web page's HTML into a few plain blocks the browser can draw: the
// site's name, colour, logo and menu, then headings, text, lists, pictures and
// simple search forms. Scripts, ads and anything hidden are left out.
use crate::theme::{hex, Rgb};
use scraper::{node::Node, ElementRef, Html, Selector};
use std::collections::HashSet;
use url::Url;

#[derive(Clone, Debug)]
pub enum Src {
    Url(Url),
    /// an <svg> drawn in the page itself
    Svg(Vec<u8>),
}

#[derive(Clone, Debug, Default)]
pub struct Span {
    pub text: String,
    pub link: Option<usize>,
    pub bold: bool,
    pub em: bool,
    pub code: bool,
}

#[derive(Clone, Debug)]
pub struct Form {
    pub action: Url,
    pub post: bool,
    pub label: String,
    pub field: String,
    pub value: String,
    pub hidden: Vec<(String, String)>,
    /// submit buttons: label, and the name=value they send
    pub buttons: Vec<(String, Option<(String, String)>)>,
}

#[derive(Clone, Debug)]
pub enum Block {
    Heading(u8, Vec<Span>),
    Para(Vec<Span>),
    /// a list item; an empty bullet carries on the item above
    Item { depth: u8, bullet: String, spans: Vec<Span> },
    Quote(Vec<Span>),
    Pre(String),
    Image { img: usize, alt: String, link: Option<usize> },
    Rule,
    Form(usize),
}

#[derive(Clone, Debug)]
pub struct Page {
    pub url: Url,
    pub title: String,
    pub site: String,
    pub brand: Option<Rgb>,
    pub logo: Option<Src>,
    pub nav: Vec<(String, usize)>,
    pub foot: Vec<(String, usize)>,
    pub blocks: Vec<Block>,
    pub links: Vec<Url>,
    /// pictures: where from, and the size the page asks for
    pub imgs: Vec<(Src, Option<u32>, Option<u32>)>,
    pub forms: Vec<Form>,
}

impl Page {
    /// A page of plain text, for errors and files that aren't HTML.
    pub fn note(url: Url, title: &str, text: &str) -> Page {
        let mut p = Page::blank(url, title);
        p.blocks.push(Block::Heading(1, vec![Span { text: title.into(), ..Default::default() }]));
        for para in text.split("\n\n") {
            p.blocks.push(Block::Para(vec![Span { text: para.into(), ..Default::default() }]));
        }
        p
    }

    pub fn text(url: Url, text: &str) -> Page {
        let mut p = Page::blank(url.clone(), url.path_segments().and_then(|mut s| s.next_back()).unwrap_or("Text"));
        p.blocks.push(Block::Pre(text.into()));
        p
    }

    pub fn picture(url: Url) -> Page {
        let mut p = Page::blank(url.clone(), url.path_segments().and_then(|mut s| s.next_back()).unwrap_or("Picture"));
        p.imgs.push((Src::Url(url), Some(10_000), None));
        p.blocks.push(Block::Image { img: 0, alt: String::new(), link: None });
        p
    }

    fn blank(url: Url, title: &str) -> Page {
        let site = host(&url);
        Page { url, title: title.into(), site, brand: None, logo: None, nav: vec![], foot: vec![], blocks: vec![], links: vec![], imgs: vec![], forms: vec![] }
    }
}

pub fn host(u: &Url) -> String {
    u.host_str().unwrap_or("").trim_start_matches("www.").to_string()
}

fn sel(s: &str) -> Selector {
    Selector::parse(s).unwrap()
}

fn squash(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn hidden(e: &ElementRef) -> bool {
    let v = e.value();
    if v.attr("hidden").is_some() || v.attr("aria-hidden") == Some("true") || v.attr("type") == Some("hidden") {
        return true;
    }
    let style = v.attr("style").unwrap_or("").replace(' ', "").to_lowercase();
    if style.contains("display:none") || style.contains("visibility:hidden") {
        return true;
    }
    v.classes().any(|c| {
        let c = c.to_lowercase();
        c.contains("sr-only") || c.contains("visually-hidden") || c.contains("screen-reader") || c == "hidden" || c.contains("skip-link") || c.contains("cookie")
    })
}

/// A colour from CSS: #rgb, #rrggbb or rgb(r, g, b).
pub fn colour(s: &str) -> Option<Rgb> {
    let s = s.trim();
    if let Some(h) = s.strip_prefix('#') {
        if h.len() == 3 {
            let d: Vec<char> = h.chars().collect();
            return hex(&format!("{0}{0}{1}{1}{2}{2}", d[0], d[1], d[2]));
        }
        return hex(h);
    }
    let inner = s.strip_prefix("rgb(").or_else(|| s.strip_prefix("rgba("))?.trim_end_matches(')');
    let n: Vec<u8> = inner.split([',', ' ', '/']).filter(|p| !p.is_empty()).take(3).filter_map(|p| p.trim().parse().ok()).collect();
    (n.len() == 3).then(|| (n[0], n[1], n[2]))
}

/// Whether a colour is lively enough to stand for a brand: not white, black or grey.
pub fn lively(c: Rgb) -> bool {
    let (mx, mn) = (c.0.max(c.1).max(c.2) as i32, c.0.min(c.1).min(c.2) as i32);
    mx - mn > 50 && mx > 60
}

pub fn parse(html: &str, url: Url) -> Page {
    let doc = Html::parse_document(html);
    let base = doc
        .select(&sel("base[href]"))
        .next()
        .and_then(|b| url.join(b.value().attr("href")?).ok())
        .unwrap_or_else(|| url.clone());
    let meta = |sels: &str| doc.select(&sel(sels)).find_map(|m| m.value().attr("content").map(squash).filter(|s| !s.is_empty()));

    let mut p = Page::blank(url.clone(), "");
    p.title = doc.select(&sel("title")).next().map(|t| squash(&t.text().collect::<String>())).unwrap_or_default();
    if let Some(s) = meta("meta[property='og:site_name'], meta[name='application-name']") {
        p.site = s;
    } else {
        // "Windows 95 - Wikipedia" is Wikipedia's page; a short title is the site itself
        let parts: Vec<&str> = p.title.split(" | ").flat_map(|x| x.split(" - ")).flat_map(|x| x.split(" – ")).flat_map(|x| x.split(" — ")).collect();
        if let Some(last) = parts.last().map(|x| x.trim()).filter(|x| (2..=30).contains(&x.chars().count())) {
            p.site = last.into();
        }
    }
    if p.title.is_empty() {
        p.title = p.site.clone();
    }
    p.brand = doc
        .select(&sel("meta[name='theme-color']"))
        .filter(|m| !m.value().attr("media").is_some_and(|m| m.contains("dark")))
        .find_map(|m| colour(m.value().attr("content")?))
        .filter(|&c| lively(c));

    let body = doc.select(&sel("body")).next().unwrap_or_else(|| doc.root_element());
    let main = ["main", "[role=main]", "#main-content", "#content", "#main", ".main-content"]
        .iter()
        .find_map(|s| doc.select(&sel(s)).find(|e| !hidden(e) && e.text().map(str::len).sum::<usize>() > 200))
        .or_else(|| {
            let art = sel("article");
            let mut arts = doc.select(&art);
            let a = arts.next()?;
            arts.next().is_none().then_some(a)
        })
        .unwrap_or(body);
    let inside = |e: &ElementRef| e.ancestors().any(|a| a.id() == main.id());
    let mut w = Walk { base: base.clone(), page: &mut p, cur: vec![], link: None, bold: 0, em: 0, code: 0, heading: None, quote: 0, lists: vec![], item: None, skip: HashSet::new(), sep: None };

    // the site's own header: logo and menu, when they sit outside the content
    let header = doc
        .select(&sel("header, [role=banner], #header, .header, #masthead, .masthead"))
        .find(|h| !inside(h) && h.id() != main.id() && !main.ancestors().any(|a| a.id() == h.id()));
    if let Some(h) = header {
        let logo = h
            .select(&sel("img, svg"))
            .find(|i| {
                let v = i.value();
                let tag = [v.attr("class"), v.attr("id"), v.attr("alt"), v.attr("src"), v.attr("aria-label")].iter().flatten().any(|a| a.to_lowercase().contains("logo"));
                let home = i.ancestors().filter_map(ElementRef::wrap).find(|a| a.value().name() == "a").and_then(|a| a.value().attr("href")).and_then(|h| base.join(h).ok()).is_some_and(|u| u.path() == "/");
                !hidden(i) && (tag || home)
            });
        p_logo(&mut w, logo);
    }
    let nav_sel = sel("nav, [role=navigation]");
    let navs: Vec<ElementRef> = doc.select(&nav_sel).filter(|n| !inside(n) && !hidden(n)).collect();
    if let Some(n) = navs.iter().find(|n| header.is_some_and(|h| n.ancestors().any(|a| a.id() == h.id()))).or(navs.first()) {
        w.page.nav = w.links_in(n, 12);
    }
    if let Some(f) = doc.select(&sel("footer, [role=contentinfo]")).filter(|f| !inside(f)).last() {
        w.page.foot = w.links_in(&f, 14);
    }
    if w.page.logo.is_none() {
        let icon = doc
            .select(&sel("link[rel~='apple-touch-icon'], link[rel~='icon']"))
            .filter_map(|l| {
                let size = l.value().attr("sizes").and_then(|s| s.split('x').next()?.parse::<u32>().ok()).unwrap_or(16);
                let href = l.value().attr("href")?;
                (!href.ends_with(".ico")).then_some((size, href))
            })
            .max_by_key(|(s, _)| *s)
            .and_then(|(_, h)| base.join(h).ok());
        w.page.logo = icon.map(Src::Url);
    }

    w.el(main);
    w.flush();
    // rules: never two together, none at the top
    p.blocks.dedup_by(|a, b| matches!((a, b), (Block::Rule, Block::Rule)));
    while matches!(p.blocks.first(), Some(Block::Rule)) {
        p.blocks.remove(0);
    }
    p
}

fn p_logo(w: &mut Walk, logo: Option<ElementRef>) {
    let Some(l) = logo else { return };
    w.page.logo = if l.value().name() == "svg" {
        Some(Src::Svg(svg_bytes(&l)))
    } else {
        img_src(&l).and_then(|s| w.base.join(&s).ok()).map(Src::Url)
    };
    w.skip.insert(l.id());
}

fn svg_bytes(e: &ElementRef) -> Vec<u8> {
    let mut s = e.html();
    if !s.contains("xmlns=") {
        s = s.replacen("<svg", "<svg xmlns=\"http://www.w3.org/2000/svg\"", 1);
    }
    s.into_bytes()
}

fn img_src(e: &ElementRef) -> Option<String> {
    let v = e.value();
    let srcset = |a: &str| v.attr(a).and_then(|s| s.split(',').next_back()).and_then(|s| s.split_whitespace().next()).map(String::from);
    [v.attr("data-src"), v.attr("src")]
        .into_iter()
        .flatten()
        .find(|s| !s.starts_with("data:"))
        .map(String::from)
        .or_else(|| srcset("srcset"))
        .or_else(|| srcset("data-srcset"))
}

fn dim(e: &ElementRef, a: &str) -> Option<u32> {
    e.value().attr(a)?.trim_end_matches("px").parse::<f32>().ok().map(|v| v as u32)
}

struct Walk<'a> {
    base: Url,
    page: &'a mut Page,
    cur: Vec<Span>,
    link: Option<usize>,
    bold: u32,
    em: u32,
    code: u32,
    heading: Option<u8>,
    quote: u32,
    /// open lists: numbered, and the count so far
    lists: Vec<(bool, usize)>,
    item: Option<(u8, String)>,
    skip: HashSet<ego_tree::NodeId>,
    /// what goes between this text and the next, where the page's layout
    /// would have put a gap: between table cells, around links
    sep: Option<&'static str>,
}

impl Walk<'_> {
    fn add_link(&mut self, href: &str) -> Option<usize> {
        if href.starts_with("javascript:") || href.starts_with("mailto:") || href.starts_with("tel:") {
            return None;
        }
        let u = self.base.join(href).ok()?;
        if let Some(i) = self.page.links.iter().position(|l| *l == u) {
            return Some(i);
        }
        self.page.links.push(u);
        Some(self.page.links.len() - 1)
    }

    fn links_in(&mut self, e: &ElementRef, max: usize) -> Vec<(String, usize)> {
        let mut out: Vec<(String, usize)> = vec![];
        for a in e.select(&sel("a[href]")) {
            if hidden(&a) || a.ancestors().filter_map(ElementRef::wrap).any(|p| hidden(&p)) {
                continue;
            }
            let t = squash(&a.text().collect::<String>());
            let lower = t.to_lowercase();
            if t.is_empty() || t.chars().count() > 28 || lower.starts_with("skip") || lower.starts_with("accessibility") {
                continue;
            }
            if let Some(i) = self.add_link(a.value().attr("href").unwrap_or("")) {
                if !out.iter().any(|(l, j)| *j == i || *l == t) {
                    out.push((t, i));
                }
            }
            if out.len() >= max {
                break;
            }
        }
        out
    }

    fn text(&mut self, t: &str) {
        let mut s = String::with_capacity(t.len());
        let mut space = self.cur.last().is_none_or(|l| l.text.ends_with(' '));
        for ch in t.chars() {
            if ch.is_whitespace() {
                if !space {
                    s.push(' ');
                    space = true;
                }
            } else {
                s.push(ch);
                space = false;
            }
        }
        if s.is_empty() {
            return;
        }
        if let Some(sep) = self.sep.take() {
            let after = self.cur.last().and_then(|l| l.text.chars().last()).is_some_and(|c| c.is_alphanumeric() || ".,:;!?)]\"'”’".contains(c));
            let before = s.chars().next().is_some_and(|c| c.is_alphanumeric() || "([\"'“‘".contains(c));
            if after && before {
                if let Some(l) = self.cur.last_mut() {
                    l.text.push_str(sep);
                }
            }
        }
        let (bold, em, code, link) = (self.bold > 0 || self.heading.is_some(), self.em > 0, self.code > 0, self.link);
        if let Some(l) = self.cur.last_mut() {
            if l.bold == bold && l.em == em && l.code == code && l.link == link {
                l.text.push_str(&s);
                return;
            }
        }
        self.cur.push(Span { text: s, link, bold, em, code });
    }

    fn flush(&mut self) {
        self.sep = None;
        let mut spans = std::mem::take(&mut self.cur);
        if let Some(f) = spans.first_mut() {
            f.text = f.text.trim_start().to_string();
        }
        if let Some(l) = spans.last_mut() {
            l.text = l.text.trim_end().to_string();
        }
        spans.retain(|s| !s.text.is_empty());
        if spans.is_empty() {
            return;
        }
        let b = if let Some(h) = self.heading {
            Block::Heading(h, spans)
        } else if let Some((depth, bullet)) = self.item.as_mut() {
            let b = Block::Item { depth: *depth, bullet: std::mem::take(bullet), spans };
            b
        } else if self.quote > 0 {
            Block::Quote(spans)
        } else {
            Block::Para(spans)
        };
        self.page.blocks.push(b);
    }

    fn kids(&mut self, e: ElementRef) {
        for n in e.children() {
            match n.value() {
                Node::Text(t) => self.text(t),
                Node::Element(_) => {
                    if let Some(c) = ElementRef::wrap(n) {
                        self.el(c);
                    }
                }
                _ => {}
            }
        }
    }

    fn image(&mut self, src: Src, alt: String, w: Option<u32>, h: Option<u32>) {
        if w.is_some_and(|w| w < 40) || h.is_some_and(|h| h < 24) {
            return;
        }
        self.flush();
        self.page.imgs.push((src, w, h));
        let link = self.link;
        self.page.blocks.push(Block::Image { img: self.page.imgs.len() - 1, alt, link });
    }

    fn form(&mut self, e: ElementRef) -> bool {
        let field = e.select(&sel("input, textarea")).find(|i| {
            let t = i.value().attr("type").unwrap_or("text").to_lowercase();
            i.value().attr("name").is_some() && (i.value().name() == "textarea" || t == "text" || t == "search") && !hidden(i)
        });
        let Some(field) = field else { return false };
        let post = e.value().attr("method").is_some_and(|m| m.eq_ignore_ascii_case("post"));
        let Ok(action) = self.base.join(e.value().attr("action").unwrap_or("")) else { return false };
        let hidden_inputs = e
            .select(&sel("input[type=hidden]"))
            .filter_map(|i| Some((i.value().attr("name")?.to_string(), i.value().attr("value").unwrap_or("").to_string())))
            .collect();
        let mut buttons = vec![];
        for b in e.select(&sel("input[type=submit], button")) {
            if hidden(&b) || b.value().attr("type").is_some_and(|t| t != "submit") {
                continue;
            }
            let label = squash(&b.value().attr("value").map(String::from).unwrap_or_else(|| b.text().collect()));
            let label = if label.is_empty() { b.value().attr("aria-label").unwrap_or("").to_string() } else { label };
            if label.is_empty() || label.chars().count() > 24 {
                continue;
            }
            if buttons.iter().any(|(l, _)| *l == label) {
                continue;
            }
            let nv = b.value().attr("name").map(|n| (n.to_string(), b.value().attr("value").unwrap_or("").to_string()));
            buttons.push((label, nv));
            if buttons.len() == 3 {
                break;
            }
        }
        if buttons.is_empty() {
            buttons.push(("Search".into(), None));
        }
        let fv = field.value();
        let label = fv.attr("aria-label").or(fv.attr("placeholder")).or(fv.attr("title")).map(squash).filter(|l| !l.is_empty() && l.chars().count() < 30).unwrap_or_else(|| "Search".into());
        self.flush();
        self.page.forms.push(Form {
            action,
            post,
            label,
            field: fv.attr("name").unwrap_or("q").to_string(),
            value: fv.attr("value").unwrap_or("").to_string(),
            hidden: hidden_inputs,
            buttons,
        });
        self.page.blocks.push(Block::Form(self.page.forms.len() - 1));
        true
    }

    fn el(&mut self, e: ElementRef) {
        if self.skip.contains(&e.id()) || hidden(&e) {
            return;
        }
        let name = e.value().name().to_ascii_lowercase();
        match name.as_str() {
            "script" | "style" | "noscript" | "template" | "iframe" | "object" | "embed" | "canvas" | "select" | "button" | "input" | "textarea" | "head" | "video" | "audio" | "map" | "dialog" | "nav" | "aside" | "footer" | "header" | "label" => {}
            "svg" => {
                let v = e.value();
                let named = v.attr("role") == Some("img") || v.attr("aria-label").is_some();
                if named {
                    let alt = v.attr("aria-label").unwrap_or("").to_string();
                    self.image(Src::Svg(svg_bytes(&e)), alt, dim(&e, "width"), dim(&e, "height"));
                }
            }
            "img" => {
                if let Some(s) = img_src(&e).and_then(|s| self.base.join(&s).ok()) {
                    let alt = squash(e.value().attr("alt").unwrap_or(""));
                    self.image(Src::Url(s), alt, dim(&e, "width"), dim(&e, "height"));
                }
            }
            "br" => self.flush(),
            "hr" => {
                self.flush();
                self.page.blocks.push(Block::Rule);
            }
            "pre" => {
                self.flush();
                let t: String = e.text().collect();
                let t = t.trim_matches('\n').to_string();
                if !t.trim().is_empty() {
                    self.page.blocks.push(Block::Pre(t));
                }
            }
            "a" => {
                let prev = self.link;
                if let Some(h) = e.value().attr("href") {
                    self.link = self.add_link(h).or(prev);
                }
                self.sep = self.sep.or(Some(" "));
                self.kids(e);
                self.link = prev;
                self.sep = self.sep.or(Some(" "));
            }
            "b" | "strong" => {
                self.bold += 1;
                self.kids(e);
                self.bold -= 1;
            }
            "i" | "em" | "cite" | "dfn" => {
                self.em += 1;
                self.kids(e);
                self.em -= 1;
            }
            "code" | "kbd" | "samp" | "tt" => {
                self.code += 1;
                self.kids(e);
                self.code -= 1;
            }
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                self.flush();
                let prev = self.heading.replace(name.as_bytes()[1] - b'0');
                self.kids(e);
                self.flush();
                self.heading = prev;
            }
            "ul" | "ol" | "menu" => {
                self.flush();
                let start = e.value().attr("start").and_then(|s| s.parse::<usize>().ok()).unwrap_or(1);
                self.lists.push((name == "ol", start.saturating_sub(1)));
                let prev = self.item.take();
                self.kids(e);
                self.flush();
                self.item = prev;
                self.lists.pop();
            }
            "li" => {
                self.flush();
                let depth = self.lists.len().max(1) as u8 - 1;
                let bullet = match self.lists.last_mut() {
                    Some((true, n)) => {
                        *n += 1;
                        format!("{n}.")
                    }
                    _ => ["•", "◦", "▪"][depth as usize % 3].into(),
                };
                let prev = self.item.replace((depth, bullet));
                self.kids(e);
                self.flush();
                self.item = prev.map(|(d, _)| (d, String::new()));
            }
            "dd" => {
                self.flush();
                let prev = self.item.replace((self.lists.len() as u8, String::new()));
                self.kids(e);
                self.flush();
                self.item = prev;
            }
            "dt" => {
                self.flush();
                self.bold += 1;
                self.kids(e);
                self.bold -= 1;
                self.flush();
            }
            "blockquote" => {
                self.flush();
                self.quote += 1;
                self.kids(e);
                self.flush();
                self.quote -= 1;
            }
            "td" | "th" => {
                self.sep = Some("  ");
                if name == "th" {
                    self.bold += 1;
                }
                self.kids(e);
                if name == "th" {
                    self.bold -= 1;
                }
                self.sep = Some("  ");
            }
            "form" => {
                if !self.form(e) {
                    self.kids(e);
                }
            }
            "p" | "div" | "section" | "article" | "main" | "tr" | "table" | "tbody" | "thead" | "figure" | "figcaption" | "address" | "details" | "summary" | "center" | "dl" | "fieldset" | "body" | "html" => {
                // an empty box (a vote arrow, a spacer) doesn't break the line
                let empty = e.text().all(|t| t.trim().is_empty()) && e.select(&sel("img, svg, form, hr")).next().is_none();
                if empty {
                    return self.kids(e);
                }
                self.flush();
                self.kids(e);
                self.flush();
            }
            _ => self.kids(e),
        }
    }
}
