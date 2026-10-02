// The visualiser listens to what the speakers are playing (the default
// output's monitor, through parec) and turns it into spectrum bars and a
// scope trace. It only listens while something is playing.
use std::{
    f32::consts::PI,
    io::Read,
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    thread,
};

pub const BANDS: usize = 48;
const N: usize = 1024;
const RATE: f32 = 44100.0;

#[derive(Default)]
pub struct Levels {
    pub bands: Vec<f32>,
    pub peaks: Vec<f32>,
    pub wave: Vec<f32>,
}

pub struct Vis {
    pub levels: Arc<Mutex<Levels>>,
    child: Option<Child>,
}

impl Vis {
    pub fn new() -> Vis {
        let l = Levels { bands: vec![0.0; BANDS], peaks: vec![0.0; BANDS], wave: vec![0.0; 128] };
        Vis { levels: Arc::new(Mutex::new(l)), child: None }
    }

    pub fn start(&mut self) {
        if self.child.is_some() {
            return;
        }
        let Ok(mut child) = Command::new("parec")
            .args(["-d", "@DEFAULT_MONITOR@", "--format=s16le", "--rate=44100", "--channels=1", "--latency-msec=25", "--client-name=win95-amp-vis"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        else {
            return;
        };
        let mut out = child.stdout.take().unwrap();
        let levels = self.levels.clone();
        thread::spawn(move || {
            let mut win = vec![0f32; N];
            let mut raw = [0u8; N];
            let edges = band_edges();
            while out.read_exact(&mut raw).is_ok() {
                // half a window of new samples each time
                let fresh: Vec<f32> = raw.chunks(2).map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0).collect();
                win.drain(..fresh.len());
                win.extend_from_slice(&fresh);
                let mags = spectrum(&win);
                let mut l = levels.lock().unwrap();
                for b in 0..BANDS {
                    let (lo, hi) = edges[b];
                    let m = mags[lo..hi.max(lo + 1)].iter().cloned().fold(0.0, f32::max);
                    let db = 20.0 * (m / (N as f32 / 4.0) + 1e-9).log10();
                    let v = ((db + 70.0) / 58.0).clamp(0.0, 1.0);
                    l.bands[b] = v.max(l.bands[b] * 0.82);
                    l.peaks[b] = l.bands[b].max(l.peaks[b] - 0.015);
                }
                let step = N / 128;
                l.wave = win.iter().step_by(step).cloned().collect();
            }
        });
        self.child = Some(child);
    }

    pub fn stop(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        let mut l = self.levels.lock().unwrap();
        l.bands.iter_mut().for_each(|v| *v = 0.0);
        l.peaks.iter_mut().for_each(|v| *v = 0.0);
        l.wave.iter_mut().for_each(|v| *v = 0.0);
    }
}

impl Drop for Vis {
    fn drop(&mut self) {
        self.stop();
    }
}

/// FFT bins for each bar, spaced evenly in pitch from 40 Hz to 16 kHz.
fn band_edges() -> Vec<(usize, usize)> {
    let bin = |f: f32| ((f / RATE * N as f32) as usize).clamp(1, N / 2 - 1);
    (0..BANDS)
        .map(|b| {
            let f = |i: usize| 40.0 * (16000.0f32 / 40.0).powf(i as f32 / BANDS as f32);
            (bin(f(b)), bin(f(b + 1)))
        })
        .collect()
}

/// Magnitudes of a Hann-windowed FFT of `x` (length N).
fn spectrum(x: &[f32]) -> Vec<f32> {
    let mut re: Vec<f32> = x.iter().enumerate().map(|(i, v)| v * (0.5 - 0.5 * (2.0 * PI * i as f32 / (N - 1) as f32).cos())).collect();
    let mut im = vec![0f32; N];
    fft(&mut re, &mut im);
    (0..N / 2).map(|i| (re[i] * re[i] + im[i] * im[i]).sqrt()).collect()
}

fn fft(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let ang = -2.0 * PI / len as f32;
        for start in (0..n).step_by(len) {
            for k in 0..len / 2 {
                let (wr, wi) = ((ang * k as f32).cos(), (ang * k as f32).sin());
                let (a, b) = (start + k, start + k + len / 2);
                let (xr, xi) = (re[b] * wr - im[b] * wi, re[b] * wi + im[b] * wr);
                re[b] = re[a] - xr;
                im[b] = im[a] - xi;
                re[a] += xr;
                im[a] += xi;
            }
        }
        len <<= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tone_lands_in_its_band() {
        let f = 1000.0;
        let x: Vec<f32> = (0..N).map(|i| (2.0 * PI * f * i as f32 / RATE).sin()).collect();
        let m = spectrum(&x);
        let peak = m.iter().enumerate().max_by(|a, b| a.1.partial_cmp(b.1).unwrap()).unwrap().0;
        assert_eq!(peak, (f / RATE * N as f32).round() as usize);
    }
}
