// Fetching pages and pictures, on a background thread so the desktop never
// waits on the network.
use super::page::{self, Page, Src};
use image::{imageops::FilterType, RgbaImage};
use resvg::{tiny_skia, usvg};
use std::{sync::mpsc::Sender, time::Duration};
use ureq::{Agent, ResponseExt};
use url::Url;

const UA: &str = "Mozilla/5.0 (X11; Linux x86_64; rv:140.0) Gecko/20100101 Firefox/140.0";
/// pictures are kept no wider than this; the screen never needs more
const MAX_PX: u32 = 480;
const MAX_IMGS: usize = 40;

pub fn agent() -> Agent {
    Agent::config_builder()
        .user_agent(UA)
        .accept("text/html,application/xhtml+xml,image/*;q=0.9,*/*;q=0.8")
        .timeout_global(Some(Duration::from_secs(20)))
        .http_status_as_error(false)
        .max_redirects(10)
        .build()
        .into()
}

pub enum Req {
    Get(Url),
    Post(Url, Vec<(String, String)>),
}

pub enum Msg {
    Page(u64, Box<Page>),
    Logo(u64, RgbaImage),
    Img(u64, usize, RgbaImage),
    Done(u64),
}

/// Loads a page, sends it, then its logo and pictures one by one.
pub fn load(agent: Agent, ticket: u64, req: Req, tx: Sender<Msg>) {
    std::thread::spawn(move || {
        let page = get_page(&agent, req);
        let logo = page.logo.clone();
        let imgs: Vec<Src> = page.imgs.iter().take(MAX_IMGS).map(|i| i.0.clone()).collect();
        if tx.send(Msg::Page(ticket, Box::new(page))).is_err() {
            return;
        }
        if let Some(img) = logo.and_then(|s| get_image(&agent, &s)) {
            if tx.send(Msg::Logo(ticket, img)).is_err() {
                return;
            }
        }
        for (i, s) in imgs.iter().enumerate() {
            if let Some(img) = get_image(&agent, s) {
                if tx.send(Msg::Img(ticket, i, img)).is_err() {
                    return;
                }
            }
        }
        let _ = tx.send(Msg::Done(ticket));
    });
}

/// Sites whose new pages won't open without a full browser, but have a plain one.
fn plainer(mut u: Url) -> Url {
    if matches!(u.host_str(), Some("reddit.com" | "www.reddit.com")) {
        let _ = u.set_host(Some("old.reddit.com"));
    }
    u
}

/// Files that aren't web pages, which a plain fetch handles better.
fn file_like(u: &Url) -> bool {
    let p = u.path().to_lowercase();
    [".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp", ".svg", ".txt", ".md", ".json", ".pdf", ".zip"].iter().any(|e| p.ends_with(e))
}

fn get_page(agent: &Agent, req: Req) -> Page {
    let req = match req {
        Req::Get(u) => Req::Get(plainer(u)),
        r => r,
    };
    let url = match &req {
        Req::Get(u) | Req::Post(u, _) => u.clone(),
    };
    if let Req::Get(u) = &req {
        if matches!(u.scheme(), "http" | "https") && !file_like(u) {
            if let Some((fin, html)) = super::chrome::get(u) {
                return page::parse(&html, fin);
            }
        }
    }
    if url.scheme() == "file" {
        return match url.to_file_path().ok().and_then(|p| std::fs::read(p).ok()) {
            Some(b) => from_bytes(url, None, b),
            None => Page::note(url, "Can't open this file", "It isn't there, or it can't be read."),
        };
    }
    let res = match req {
        Req::Get(u) => agent.get(u.as_str()).call(),
        Req::Post(u, form) => agent.post(u.as_str()).send_form(form.iter().map(|(k, v)| (k.as_str(), v.as_str()))),
    };
    let mut res = match res {
        Ok(r) => r,
        Err(e) => return Page::note(url, "Can't open this page", &plain_error(&e)),
    };
    let fin = Url::parse(&res.get_uri().to_string()).unwrap_or(url);
    let status = res.status();
    let mime = res.body().mime_type().map(str::to_lowercase);
    let body = match res.body_mut().with_config().limit(8 << 20).read_to_vec() {
        Ok(b) => b,
        Err(e) => return Page::note(fin, "Can't open this page", &plain_error(&e)),
    };
    let mut p = from_bytes(fin, mime, body);
    if status.as_u16() >= 400 && p.blocks.is_empty() {
        p = Page::note(p.url, "Can't open this page", &format!("The site said {status}."));
    }
    p
}

fn from_bytes(url: Url, mime: Option<String>, body: Vec<u8>) -> Page {
    let path = url.path().to_lowercase();
    let mime = mime.unwrap_or_else(|| {
        if path.ends_with(".txt") || path.ends_with(".md") {
            "text/plain".into()
        } else if [".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp", ".svg"].iter().any(|e| path.ends_with(e)) {
            "image/".into()
        } else {
            "text/html".into()
        }
    });
    let text = || String::from_utf8_lossy(&body).to_string();
    if mime.contains("html") || mime.contains("xml") && !mime.contains("svg") {
        page::parse(&text(), url)
    } else if mime.starts_with("image/") {
        Page::picture(url)
    } else if mime.starts_with("text/") || mime.contains("json") {
        Page::text(url, &text())
    } else {
        Page::note(url, "Can't show this here", &format!("It's a {mime} file, which this browser can't draw."))
    }
}

fn plain_error(e: &ureq::Error) -> String {
    match e {
        ureq::Error::HostNotFound => "There's no site at that address. Check the spelling, or that you're online.".into(),
        ureq::Error::Timeout(_) => "The site took too long to answer.".into(),
        ureq::Error::Io(io) => format!("The connection failed: {io}."),
        other => format!("{other}"),
    }
}

pub fn get_image(agent: &Agent, src: &Src) -> Option<RgbaImage> {
    let bytes = match src {
        Src::Svg(b) => b.clone(),
        Src::Url(u) if u.scheme() == "file" => std::fs::read(u.to_file_path().ok()?).ok()?,
        Src::Url(u) => {
            let mut r = agent.get(u.as_str()).call().ok()?;
            if !r.status().is_success() {
                return None;
            }
            r.body_mut().with_config().limit(12 << 20).read_to_vec().ok()?
        }
    };
    let img = match image::load_from_memory(&bytes) {
        Ok(i) => i.to_rgba8(),
        Err(_) => svg(&bytes)?,
    };
    Some(shrink(img))
}

fn shrink(img: RgbaImage) -> RgbaImage {
    if img.width() <= MAX_PX {
        return img;
    }
    let h = (img.height() as u64 * MAX_PX as u64 / img.width().max(1) as u64).max(1) as u32;
    image::imageops::resize(&img, MAX_PX, h, FilterType::Triangle)
}

fn svg(bytes: &[u8]) -> Option<RgbaImage> {
    let tree = usvg::Tree::from_data(bytes, &usvg::Options::default()).ok()?;
    let size = tree.size();
    let scale = (MAX_PX as f32 / size.width()).min(4.0);
    let (w, h) = ((size.width() * scale).ceil() as u32, (size.height() * scale).ceil() as u32);
    let mut pix = tiny_skia::Pixmap::new(w.max(1), h.max(1))?;
    resvg::render(&tree, tiny_skia::Transform::from_scale(scale, scale), &mut pix.as_mut());
    let mut out = RgbaImage::new(pix.width(), pix.height());
    for (o, p) in out.pixels_mut().zip(pix.pixels()) {
        let c = p.demultiply();
        *o = image::Rgba([c.red(), c.green(), c.blue(), c.alpha()]);
    }
    Some(out)
}
