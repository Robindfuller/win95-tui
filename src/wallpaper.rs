// The desktop background: a plain colour, or a picture that fills, fits or
// stretches to the screen and can be faded out towards that colour. Kept in
// ~/.config/win95-tui/settings alongside the look.
use crate::theme::{home, mix, save_setting, setting, Rgb};
use image::{imageops, ImageReader, RgbImage};
use std::{
    fs,
    path::{Path, PathBuf},
    time::SystemTime,
};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Fit {
    Fill,
    Fit,
    Stretch,
}

pub const FITS: [(Fit, &str); 3] = [(Fit::Fill, "Fill"), (Fit::Fit, "Fit"), (Fit::Stretch, "Stretch")];

#[derive(Clone, PartialEq, Debug)]
pub struct Back {
    pub picture: Option<PathBuf>,
    pub fit: Fit,
    /// how far the picture fades into the colour, in percent
    pub fade: u8,
    /// None: the theme's own desktop colour
    pub colour: Option<Rgb>,
    /// a patch of the theme's background behind each icon, so it shows up on anything
    pub shade: bool,
}

impl Back {
    pub fn load() -> Back {
        Back {
            picture: setting("wallpaper").filter(|p| !p.is_empty()).map(PathBuf::from),
            fit: match setting("wallpaper_fit").as_deref() {
                Some("fit") => Fit::Fit,
                Some("stretch") => Fit::Stretch,
                _ => Fit::Fill,
            },
            fade: setting("wallpaper_fade").and_then(|v| v.parse().ok()).unwrap_or(0).min(MAX_FADE),
            colour: setting("desk_colour").and_then(|v| crate::theme::hex(&v)),
            shade: setting("icon_shade").as_deref() != Some("off"),
        }
    }

    pub fn save(&self) {
        save_setting("wallpaper", &self.picture.as_ref().map(|p| p.to_string_lossy().to_string()).unwrap_or_default());
        save_setting("wallpaper_fit", &FITS.iter().find(|f| f.0 == self.fit).unwrap().1.to_lowercase());
        save_setting("wallpaper_fade", &self.fade.to_string());
        save_setting("desk_colour", &self.colour.map(|(r, g, b)| format!("#{r:02x}{g:02x}{b:02x}")).unwrap_or_else(|| "theme".into()));
        save_setting("icon_shade", if self.shade { "on" } else { "off" });
    }

    /// Whether there's anything behind the icons but the theme's own colour.
    pub fn custom(&self) -> bool {
        self.picture.is_some() || self.colour.is_some()
    }
}

pub const MAX_FADE: u8 = 90;

/// Omarchy's current wallpaper; it follows theme switches.
pub fn omarchy() -> PathBuf {
    home().join(".local/state/omarchy/current/background")
}

/// The pictures beside `p` (or in Omarchy's theme backgrounds), sorted.
pub fn siblings(p: Option<&Path>) -> Vec<PathBuf> {
    let dir = p
        .and_then(|p| fs::canonicalize(p).ok())
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| home().join(".local/state/omarchy/current/theme/backgrounds"));
    let mut v: Vec<PathBuf> = fs::read_dir(dir)
        .map(|d| d.flatten().map(|e| e.path()).filter(|p| is_picture(p)).collect())
        .unwrap_or_default();
    v.sort();
    v
}

pub fn is_picture(p: &Path) -> bool {
    let ext = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    p.is_file() && ["jpg", "jpeg", "png", "webp", "gif", "bmp"].contains(&ext.as_str())
}

/// Keeps the decoded picture and the last screenful of pixels, so redraws
/// only scale it again when something changes.
#[derive(Default)]
pub struct Painter {
    /// the picture's real path and modified time, and the picture shrunk to a workable size
    src: Option<((PathBuf, Option<SystemTime>), Option<RgbImage>)>,
    out: Option<((PathBuf, i32, i32, Fit, u8, Rgb), Vec<Rgb>)>,
}

impl Painter {
    /// The screen as w by 2h pixels (two to a text cell), or None when
    /// there's no picture or it can't be read.
    pub fn pixels(&mut self, back: &Back, w: i32, h: i32, bg: Rgb) -> Option<&[Rgb]> {
        let path = fs::canonicalize(back.picture.as_ref()?).ok()?;
        let id = (path.clone(), fs::metadata(&path).and_then(|m| m.modified()).ok());
        if self.src.as_ref().is_none_or(|(k, _)| *k != id) {
            self.src = Some((id, load(&path)));
            self.out = None;
        }
        let img = self.src.as_ref()?.1.as_ref()?;
        let key = (path, w, h, back.fit, back.fade, bg);
        if self.out.as_ref().is_none_or(|(k, _)| *k != key) {
            self.out = Some((key, scale(img, w.max(1) as u32, (2 * h).max(1) as u32, back.fit, back.fade, bg)));
        }
        self.out.as_ref().map(|(_, v)| v.as_slice())
    }
}

/// Reads a picture, shrunk to no more than 640 by 640 since a terminal never
/// shows more than that.
fn load(p: &Path) -> Option<RgbImage> {
    let img = ImageReader::open(p).ok()?.with_guessed_format().ok()?.decode().ok()?;
    Some(img.thumbnail(640, 640).to_rgb8())
}

fn scale(img: &RgbImage, tw: u32, th: u32, fit: Fit, fade: u8, bg: Rgb) -> Vec<Rgb> {
    let (iw, ih) = (img.width().max(1) as f32, img.height().max(1) as f32);
    let filter = imageops::FilterType::Triangle;
    let mut out = vec![bg; (tw * th) as usize];
    let (pic, ox, oy) = match fit {
        Fit::Stretch => (imageops::resize(img, tw, th, filter), 0, 0),
        Fit::Fill => {
            // cut the middle out at the screen's shape, then scale that
            let s = (tw as f32 / iw).max(th as f32 / ih);
            let (cw, ch) = (((tw as f32 / s).round() as u32).clamp(1, img.width()), ((th as f32 / s).round() as u32).clamp(1, img.height()));
            let crop = imageops::crop_imm(img, (img.width() - cw) / 2, (img.height() - ch) / 2, cw, ch).to_image();
            (imageops::resize(&crop, tw, th, filter), 0, 0)
        }
        Fit::Fit => {
            let s = (tw as f32 / iw).min(th as f32 / ih);
            let (nw, nh) = (((iw * s).round() as u32).clamp(1, tw), ((ih * s).round() as u32).clamp(1, th));
            (imageops::resize(img, nw, nh, filter), (tw - nw) / 2, (th - nh) / 2)
        }
    };
    let t = fade.min(MAX_FADE) as f32 / 100.0;
    for (x, y, p) in pic.enumerate_pixels() {
        let (x, y) = (x + ox, y + oy);
        if x < tw && y < th {
            out[(y * tw + x) as usize] = mix((p[0], p[1], p[2]), bg, t);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_fill_stretch_and_fade() {
        // a 4 by 2 picture, red on the left half and blue on the right
        let img = RgbImage::from_fn(4, 2, |x, _| if x < 2 { image::Rgb([255, 0, 0]) } else { image::Rgb([0, 0, 255]) });
        let bg = (0, 0, 0);
        // fit into a square: bands of background above and below
        let v = scale(&img, 4, 4, Fit::Fit, 0, bg);
        assert_eq!(v[0], bg);
        assert_eq!(v[4 * 1], (255, 0, 0));
        assert_eq!(v[4 * 3], bg);
        // fill a square: no background, the middle of the picture
        let v = scale(&img, 4, 4, Fit::Fill, 0, bg);
        assert!(v.iter().all(|&p| p != bg));
        // fading half way mixes towards the background
        let v = scale(&img, 4, 2, Fit::Stretch, 50, bg);
        assert_eq!(v[0], (128, 0, 0));
    }
}
