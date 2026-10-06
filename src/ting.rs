//! A Teenage Engineering TING (EP-2350 FX MIC), heard in the microphone stream.
//!
//! With the ting-wispr script on its disk, the TING plays short chirps down its
//! audio cable (16.5-19.5 kHz, above speech, and gone after the 16 kHz
//! resample): a squeeze, a release, its buttons. So when the TING is Heyra's
//! microphone, the squeeze is push-to-talk with nothing else installed.
//!
//! The decoder is a Rust port of tingle's SymbolDetector (MIT, Copyright (c)
//! 2026 Tutor Intelligence, Inc.): per band, mix down to baseband, low-pass and
//! decimate 48k -> 3k, then correlate against the band's two chirp templates.
//! tingle's three-beacon acquisition lock is left out: Heyra only listens to
//! the one microphone it was given.

use std::f64::consts::PI;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Event {
    Squeeze,
    Release,
    /// The bottom (white) button.
    Bottom,
    /// The middle (green) button, which changes mode.
    Middle,
}

/// The chirps are defined at 48 kHz; other rates aren't decoded.
pub const RATE: u32 = 48_000;
const FRAMES: usize = 1_200; // 25 ms per symbol
const EDGE: usize = 144; // 3 ms raised-cosine edges
const DECIMATION: usize = 16;
const TAPS: usize = FRAMES / DECIMATION;
const FIR_TAPS: usize = 64;

/// (start Hz, end Hz): low band up, low down, high up, high down.
const SWEEPS: [(f64, f64); 4] = [(16_500., 17_900.), (17_900., 16_500.), (18_100., 19_500.), (19_500., 18_100.)];

/// RS[4,2] over GF(4): 16 words, distance 3, so any one wrong symbol is corrected.
const CODEBOOK: [[usize; 4]; 16] = [
    [0, 0, 0, 0], [0, 1, 1, 2], [0, 2, 2, 3], [0, 3, 3, 1],
    [1, 0, 1, 1], [1, 1, 0, 3], [1, 2, 3, 2], [1, 3, 2, 0],
    [2, 0, 2, 2], [2, 1, 3, 0], [2, 2, 0, 1], [2, 3, 1, 3],
    [3, 0, 3, 3], [3, 1, 2, 1], [3, 2, 1, 0], [3, 3, 0, 2],
];

const CORR_THRESHOLD: f64 = 0.45;
const DOMINANCE: f64 = 1.6;
const REFRACTORY: f64 = 0.015;
const WORD_WINDOW: f64 = 0.5;
const MIN_SYMBOL_GAP: f64 = 0.015;
const MAX_SYMBOL_GAP: f64 = 0.070;
const MIN_LEVEL_DB: f64 = -60.0;

type C = (f64, f64);

fn chirp(symbol: usize) -> Vec<f64> {
    let (f0, f1) = SWEEPS[symbol];
    let mut phase = 0.0;
    (0..FRAMES)
        .map(|i| {
            phase += 2. * PI * (f0 + (f1 - f0) * i as f64 / FRAMES as f64) / RATE as f64;
            let fade = |k: usize| if k < EDGE { 0.5 * (1. - (PI * k as f64 / EDGE as f64).cos()) } else { 1. };
            phase.sin() * fade(i) * fade(FRAMES - 1 - i)
        })
        .collect()
}

fn fir() -> [f64; FIR_TAPS] {
    // Hamming-windowed sinc low-pass, cutoff 1.3 kHz at 48k.
    let fc = 1_300. / RATE as f64;
    let mut h = [0.; FIR_TAPS];
    for (i, tap) in h.iter_mut().enumerate() {
        let m = i as f64 - (FIR_TAPS - 1) as f64 / 2.;
        let sinc = if m == 0. { 2. * fc } else { (2. * PI * fc * m).sin() / (PI * m) };
        *tap = sinc * (0.54 - 0.46 * (2. * PI * i as f64 / (FIR_TAPS - 1) as f64).cos());
    }
    let sum: f64 = h.iter().sum();
    h.map(|x| x / sum)
}

/// One sample of baseband: low-passed `x * carrier` ending at `at`.
fn mix(fir: &[f64; FIR_TAPS], carrier: &[C; 64], at: usize, x: impl Fn(usize) -> f64) -> C {
    let (mut re, mut im) = (0., 0.);
    for (k, h) in fir.iter().enumerate() {
        let Some(idx) = at.checked_sub(k) else { break };
        let v = x(idx);
        let c = carrier[idx & 63];
        re += h * v * c.0;
        im += h * v * c.1;
    }
    (re, im)
}

struct Band {
    carrier: [C; 64],
    symbols: [usize; 2],
    templates: [Vec<C>; 2],
    template_energy: [f64; 2],
    /// The last TAPS baseband samples, oldest first, and their total energy.
    window: std::collections::VecDeque<C>,
    window_energy: f64,
    last_peak: f64,
    tracking: Option<(f64, usize, f64, f64)>, // (corr, symbol, at, level dB)
}

impl Band {
    fn new(fir: &[f64; FIR_TAPS], cycles_per_64: usize, symbols: [usize; 2]) -> Self {
        let carrier = std::array::from_fn(|m| {
            let a = -2. * PI * cycles_per_64 as f64 * m as f64 / 64.;
            (a.cos(), a.sin())
        });
        let template = |symbol: usize| -> Vec<C> {
            let raw = chirp(symbol);
            (DECIMATION - 1..FRAMES).step_by(DECIMATION).map(|i| mix(fir, &carrier, i, |j| raw[j])).collect()
        };
        let templates = [template(symbols[0]), template(symbols[1])];
        let energy = |t: &Vec<C>| t.iter().map(|z| z.0 * z.0 + z.1 * z.1).sum();
        let template_energy = [energy(&templates[0]), energy(&templates[1])];
        Self {
            carrier,
            symbols,
            templates,
            template_energy,
            window: Default::default(),
            window_energy: 0.,
            last_peak: f64::NEG_INFINITY,
            tracking: None,
        }
    }
}

pub struct Decoder {
    fir: [f64; FIR_TAPS],
    bands: [Band; 2],
    raw: [f32; 128],
    count: usize,
    stream: Vec<(usize, f64)>, // (symbol, at)
    held: bool,
}

impl Default for Decoder {
    fn default() -> Self {
        let fir = fir();
        let bands = [Band::new(&fir, 23, [0, 1]), Band::new(&fir, 25, [2, 3])]; // 17.25k, 18.75k
        Self { fir, bands, raw: [0.; 128], count: 0, stream: Vec::new(), held: false }
    }
}

impl Decoder {
    fn now(&self) -> f64 {
        self.count as f64 / RATE as f64
    }

    pub fn process(&mut self, samples: &[f32], events: &mut Vec<Event>) {
        for &x in samples {
            self.raw[self.count & 127] = x;
            self.count += 1;
            if self.count % DECIMATION == 0 && self.count >= FIR_TAPS {
                for b in 0..2 {
                    if let Some((symbol, at)) = self.advance(b) {
                        self.symbol(symbol, at, events);
                    }
                }
            }
        }
        if self.stream.last().is_some_and(|&(_, at)| self.now() - at > WORD_WINDOW) {
            self.stream.clear();
        }
    }

    /// One decimated sample for a band; returns a symbol once its correlation peak has passed.
    fn advance(&mut self, b: usize) -> Option<(usize, f64)> {
        let now = self.now();
        let raw = &self.raw;
        let band = &mut self.bands[b];
        let z = mix(&self.fir, &band.carrier, self.count - 1, |i| raw[i & 127] as f64);
        band.window.push_back(z);
        band.window_energy += z.0 * z.0 + z.1 * z.1;
        if band.window.len() > TAPS {
            let old = band.window.pop_front().unwrap();
            band.window_energy -= old.0 * old.0 + old.1 * old.1;
        }
        if band.window.len() < TAPS || now - band.last_peak < REFRACTORY || band.window_energy <= 1e-18 {
            return None;
        }

        // (corr, symbol, level dB) for the best template, and the other's corr.
        let mut best = (0., usize::MAX, -200.);
        let mut sibling: f64 = 0.;
        for t in 0..2 {
            let (mut cre, mut cim) = (0., 0.);
            for (w, z) in band.templates[t].iter().zip(&band.window) {
                cre += w.0 * z.0 + w.1 * z.1;
                cim += w.0 * z.1 - w.1 * z.0;
            }
            let mag = (cre * cre + cim * cim).sqrt();
            let corr = mag / (band.window_energy * band.template_energy[t]).sqrt();
            if corr > best.0 {
                sibling = sibling.max(best.0);
                best = (corr, band.symbols[t], 20. * (mag / band.template_energy[t].sqrt()).max(1e-10).log10());
            } else {
                sibling = sibling.max(corr);
            }
        }

        // Emit when the tracked peak has fallen away: the next symbol of the
        // same band may come before the correlation drops below threshold.
        let tracked = band.tracking.map_or(0., |t| t.0);
        if best.0 >= CORR_THRESHOLD && best.0 >= sibling * DOMINANCE && (band.tracking.is_none() || best.0 >= tracked * 0.75) {
            if best.0 > tracked {
                band.tracking = Some((best.0, best.1, now, best.2));
            }
            return None;
        }
        let (_, symbol, at, level) = band.tracking.take()?;
        band.last_peak = at;
        (level >= MIN_LEVEL_DB).then_some((symbol, at))
    }

    fn symbol(&mut self, symbol: usize, at: f64, events: &mut Vec<Event>) {
        self.stream.push((symbol, at));
        self.stream.retain(|&(_, t)| at - t <= WORD_WINDOW);
        let n = self.stream.len();
        if n < 4 {
            return;
        }
        let word = &self.stream[n - 4..];
        if word.windows(2).any(|p| !(MIN_SYMBOL_GAP..=MAX_SYMBOL_GAP).contains(&(p[1].1 - p[0].1))) {
            return;
        }
        let received: Vec<usize> = word.iter().map(|w| w.0).collect();
        let Some(message) = (0..16).find(|&m| CODEBOOK[m].iter().zip(&received).filter(|(a, b)| a != b).count() <= 1) else {
            return;
        };
        self.stream.clear();
        let event = match message {
            0 if self.held => Event::Release, // released beacon: heals a missed release
            1 | 2 if !self.held => Event::Squeeze,
            3 if self.held => Event::Release,
            4..=7 => Event::Bottom,
            8..=11 => Event::Middle,
            _ => return,
        };
        match event {
            Event::Squeeze => self.held = true,
            Event::Release => self.held = false,
            _ => {}
        }
        events.push(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Words as the TING plays them: four chirps 29 ms apart, quiet, over speech-band noise.
    fn play(words: &[(f64, usize)], secs: f64) -> Vec<f32> {
        let mut out = vec![0f32; (secs * RATE as f64) as usize];
        let mut seed = 1u32;
        for (i, s) in out.iter_mut().enumerate() {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let t = i as f64 / RATE as f64;
            *s = (0.02 * (seed as f64 / u32::MAX as f64 - 0.5) + 0.2 * (2. * PI * 220. * t).sin()) as f32;
        }
        for &(at, message) in words {
            for (k, &symbol) in CODEBOOK[message].iter().enumerate() {
                let start = ((at + k as f64 * 0.029) * RATE as f64) as usize;
                for (j, v) in chirp(symbol).into_iter().enumerate() {
                    out[start + j] += (v * 0.15) as f32;
                }
            }
        }
        out
    }

    fn decode(audio: &[f32]) -> Vec<Event> {
        let mut decoder = Decoder::default();
        let mut events = Vec::new();
        for block in audio.chunks(512) {
            decoder.process(block, &mut events);
        }
        events
    }

    // As the script sends them: squeeze = beacon(held) + down; release = up + two
    // released beacons; a button = released beacon + the button's word.
    #[test]
    fn squeeze_and_release() {
        let words = [(0.1, 1), (0.25, 2), (1.0, 3), (1.15, 0), (1.3, 0)];
        assert_eq!(decode(&play(&words, 2.0)), [Event::Squeeze, Event::Release]);
    }

    #[test]
    fn buttons() {
        let words = [(0.1, 0), (0.25, 5), (0.8, 0), (0.95, 9)];
        assert_eq!(decode(&play(&words, 1.5)), [Event::Bottom, Event::Middle]);
    }

    #[test]
    fn bottom_button_mid_squeeze_ends_the_take_first() {
        // The beacon before the button says "released", as in tingle: talk, then send.
        let words = [(0.1, 1), (0.25, 2), (1.0, 0), (1.15, 4)];
        assert_eq!(decode(&play(&words, 1.6)), [Event::Squeeze, Event::Release, Event::Bottom]);
    }

    #[test]
    fn speech_alone_is_silent() {
        assert_eq!(decode(&play(&[], 3.0)), []);
    }
}
