// Internet radio from radio-browser.info, a free open directory of stations.
use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Station {
    pub name: String,
    pub url: String,
    #[serde(default)]
    pub bitrate: u32,
    #[serde(default)]
    pub codec: String,
    #[serde(default)]
    pub country: String,
}

#[derive(Deserialize)]
struct Raw {
    name: String,
    url_resolved: String,
    #[serde(default)]
    bitrate: u32,
    #[serde(default)]
    codec: String,
    #[serde(default)]
    countrycode: String,
}

pub const GENRES: [&str; 14] = ["any", "pop", "rock", "indie", "jazz", "classical", "dance", "electronic", "ambient", "chillout", "80s", "news", "talk", "soul"];

/// The most liked working stations matching a name and genre.
pub fn search(name: &str, genre: &str) -> Result<Vec<Station>> {
    let mut url = String::from("https://all.api.radio-browser.info/json/stations/search?limit=200&hidebroken=true&order=votes&reverse=true");
    if !name.trim().is_empty() {
        url += &format!("&name={}", enc(name.trim()));
    }
    if genre != "any" {
        url += &format!("&tag={}&tagExact=true", enc(genre));
    }
    let body = ureq::get(&url).header("User-Agent", "win95-tui/0.1").call()?.body_mut().read_to_string()?;
    let raw: Vec<Raw> = serde_json::from_str(&body)?;
    let mut seen = std::collections::HashSet::new();
    Ok(raw
        .into_iter()
        .filter(|r| !r.url_resolved.is_empty())
        .map(|r| Station { name: r.name.trim().to_string(), url: r.url_resolved, bitrate: r.bitrate, codec: r.codec, country: r.countrycode })
        // the directory lists some stations many times over
        .filter(|s| seen.insert(s.name.to_lowercase()))
        .collect())
}

fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}
