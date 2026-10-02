// AI-assisted mode: a few lines at the top of each page saying what it's about,
// written by `claude -p` on a background thread (it bills the subscription).
use super::{
    fetch::Msg,
    page::{Block, Page, Span},
};
use std::{
    io::Write,
    process::{Command, Stdio},
    sync::mpsc::Sender,
};

/// pages shorter than this have nothing worth summing up
const MIN_TEXT: usize = 300;
/// what Claude is sent, at most
const MAX_TEXT: usize = 24_000;

const PROMPT: &str = "You are shown the readable text of a web page. Say what it is and the main points in 2 to 4 short lines, each starting with '• '. \
If it's a page of search results, give the answer if the results show it, then the best one or two places to look. \
Plain text only, no markdown, no headings, no preamble. British English, plain words.";

#[derive(Clone, Debug)]
pub enum Summary {
    Asking,
    Done(Vec<String>),
    Failed(String),
}

fn spans(v: &[Span]) -> String {
    v.iter().map(|s| s.text.as_str()).collect::<String>().trim().to_string()
}

/// The page as plain text, the way a reader sees it.
pub fn text(p: &Page) -> String {
    let mut out = format!("Title: {}\nAddress: {}\n\n", p.title, p.url);
    for b in &p.blocks {
        let line = match b {
            Block::Heading(lv, s) => format!("{} {}", "#".repeat(*lv as usize), spans(s)),
            Block::Para(s) | Block::Quote(s) => spans(s),
            Block::Item { bullet, spans: s, .. } => format!("{bullet} {}", spans(s)),
            Block::Pre(t) => t.clone(),
            Block::Image { alt, .. } if !alt.is_empty() => format!("[picture: {alt}]"),
            _ => continue,
        };
        if !line.trim().is_empty() {
            out.push_str(&line);
            out.push('\n');
        }
        if out.len() > MAX_TEXT {
            let mut cut = MAX_TEXT;
            while !out.is_char_boundary(cut) {
                cut -= 1;
            }
            out.truncate(cut);
            break;
        }
    }
    out
}

/// Whether a page has enough words to be worth asking about.
pub fn worth(p: &Page) -> bool {
    p.url.scheme() != "about" && text(p).len() >= MIN_TEXT
}

/// Asks Claude about a page; the answer comes back as `Msg::Summary(doc, ..)`.
pub fn ask(doc: u64, page_text: String, tx: Sender<Msg>) {
    std::thread::spawn(move || {
        let _ = tx.send(Msg::Summary(doc, run(&page_text)));
    });
}

fn run(page_text: &str) -> Summary {
    let child = Command::new("claude")
        .args(["-p", "--model", "haiku", "--tools", "", "--strict-mcp-config", "--no-session-persistence", "--system-prompt", PROMPT])
        .current_dir(std::env::temp_dir())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => return Summary::Failed(format!("couldn't start claude ({e})")),
    };
    if let Some(mut i) = child.stdin.take() {
        let _ = i.write_all(page_text.as_bytes());
    }
    let out = match child.wait_with_output() {
        Ok(o) => o,
        Err(e) => return Summary::Failed(e.to_string()),
    };
    let said = String::from_utf8_lossy(&out.stdout);
    if !out.status.success() {
        let why = String::from_utf8_lossy(&out.stderr);
        let why = why.lines().chain(said.lines()).find(|l| !l.trim().is_empty()).unwrap_or("claude stopped with an error");
        return Summary::Failed(why.trim().to_string());
    }
    let lines: Vec<String> = said.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect();
    if lines.is_empty() {
        return Summary::Failed("claude said nothing".into());
    }
    Summary::Done(lines)
}
