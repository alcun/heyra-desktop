//! Speech engines. A sherpa-onnx transducer (Parakeet, Bark) runs anywhere
//! (macOS, Windows, Linux); an Apple engine can slot in later behind the same trait.

use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;

use sherpa_onnx::{OfflineRecognizer, OfflineRecognizerConfig, OfflineTransducerModelConfig};

pub trait Engine: Send {
    fn transcribe(&mut self, sample_rate: u32, samples: &[f32]) -> String;
}

/// The engine in use, shared with the thread that transcribes a take as it's recorded.
pub type Shared = Arc<Mutex<Box<dyn Engine>>>;

fn lock(engine: &Shared) -> MutexGuard<'_, Box<dyn Engine>> {
    engine.lock().unwrap_or_else(|e| e.into_inner())
}

/// A NeMo transducer through sherpa-onnx: Parakeet TDT 0.6B v3 and the models built on it.
pub struct Transducer(OfflineRecognizer);

impl Transducer {
    pub fn load(dir: &Path) -> Result<Self, String> {
        let files = crate::model::files(dir)?;
        let file = |path: &Path| Some(path.to_string_lossy().into_owned());
        let mut config = OfflineRecognizerConfig::default();
        config.model_config.transducer = OfflineTransducerModelConfig {
            encoder: file(&files.encoder),
            decoder: file(&files.decoder),
            joiner: file(&files.joiner),
        };
        config.model_config.tokens = file(&files.tokens);
        config.model_config.model_type = Some("nemo_transducer".into());
        config.model_config.num_threads = 4;
        config.decoding_method = Some("greedy_search".into());
        OfflineRecognizer::create(&config)
            .map(Self)
            .ok_or_else(|| format!("could not load the model in {}", dir.display()))
    }
}

impl Engine for Transducer {
    fn transcribe(&mut self, sample_rate: u32, samples: &[f32]) -> String {
        let stream = self.0.create_stream();
        // A word right at an edge (a piece cut near it) is often dropped without a
        // moment of silence around it.
        let pad = vec![0.0; (sample_rate as f32 * PAD_SECS) as usize];
        stream.accept_waveform(sample_rate as i32, &pad);
        stream.accept_waveform(sample_rate as i32, samples);
        stream.accept_waveform(sample_rate as i32, &pad);
        self.0.decode(&stream);
        stream.get_result().map(|r| r.text.trim().to_string()).unwrap_or_default()
    }
}

const PAD_SECS: f32 = 0.3;

/// Parakeet skips stretches of speech in a long input (a 7-minute take in
/// 60-second pieces lost a quarter of its words), so long takes go in in
/// pieces of about 15 seconds, each cut at the quietest moment near its end.
const PIECE_SECS: f32 = 15.0;
const LOOK_BACK_SECS: f32 = 5.0;

pub fn transcribe_long(engine: &mut dyn Engine, sample_rate: u32, samples: &[f32]) -> String {
    let rate = sample_rate as f32;
    let piece = (PIECE_SECS * rate) as usize;
    let mut parts = Vec::new();
    let mut start = 0;
    while samples.len() - start > piece + piece / 2 {
        let cut = quietest(samples, start + piece - (LOOK_BACK_SECS * rate) as usize, start + piece, sample_rate);
        parts.push(engine.transcribe(sample_rate, &samples[start..cut]));
        start = cut;
    }
    parts.push(engine.transcribe(sample_rate, &samples[start..]));
    parts.retain(|p| !p.is_empty());
    parts.join(" ")
}

/// While talking, the take goes to the engine in the background a piece at a time,
/// each cut at a pause once there's enough of it. Letting go then leaves only the
/// last stretch to transcribe, so the wait no longer grows with the length of the take.
/// A cut in the middle of speech loses words at the edges, so without a pause it waits,
/// and only past `AHEAD_MAX_SECS` cuts at the quietest moment it has.
const AHEAD_MIN_SECS: f32 = 10.0;
const AHEAD_MAX_SECS: f32 = 20.0;
/// A pause: 100 ms at under this share of the take's speaking loudness.
const PAUSE_SHARE: f32 = 0.12;

/// One take being transcribed while it's recorded.
pub struct Ahead {
    engine: Shared,
    sample_rate: u32,
    /// Where the audio not yet sent to the engine begins.
    cut: usize,
    /// The take's length when it was last looked at for a pause.
    looked: usize,
    pieces: Vec<Piece>,
}

/// A piece of the take: still with the engine, or written.
enum Piece {
    Writing(JoinHandle<String>),
    Written(String),
}

impl Piece {
    fn text(self) -> String {
        match self {
            Piece::Writing(handle) => handle.join().unwrap_or_default(),
            Piece::Written(text) => text,
        }
    }
}

impl Ahead {
    pub fn new(engine: Shared, sample_rate: u32) -> Self {
        Self { engine, sample_rate, cut: 0, looked: 0, pieces: Vec::new() }
    }

    /// Samples already sent to the engine.
    pub fn cut(&self) -> usize {
        self.cut
    }

    /// Called through the take with its length so far; `peek` copies the take from a point on.
    pub fn feed(&mut self, len: usize, peek: impl FnOnce(usize) -> Vec<f32>) {
        let rate = self.sample_rate as f32;
        let (min, max) = ((AHEAD_MIN_SECS * rate) as usize, (AHEAD_MAX_SECS * rate) as usize);
        // Looked at again every quarter second at most: each look copies the audio.
        let step = (rate / 4.0) as usize;
        if len < self.cut + min || len < self.looked + step || self.pieces.last().is_some_and(|p| matches!(p, Piece::Writing(h) if !h.is_finished())) {
            return;
        }
        self.looked = len;
        let audio = peek(self.cut);
        if audio.len() < min {
            return;
        }
        let end = match last_pause(&audio, min - (3.0 * rate) as usize, self.sample_rate) {
            Some(end) => end,
            None if audio.len() >= max => quietest(&audio, max - (5.0 * rate) as usize, max, self.sample_rate),
            None => return,
        };
        let mut chunk = audio;
        chunk.truncate(end);
        self.cut += end;
        let (engine, rate) = (self.engine.clone(), self.sample_rate);
        self.pieces.push(Piece::Writing(std::thread::spawn(move || lock(&engine).transcribe(rate, &chunk))));
    }

    /// The text of pieces finished since the last call, in order: the take so far, for
    /// showing as it's written.
    pub fn written(&mut self) -> Vec<String> {
        let mut new = Vec::new();
        for piece in &mut self.pieces {
            if matches!(piece, Piece::Writing(h) if h.is_finished()) {
                let Piece::Writing(handle) = std::mem::replace(piece, Piece::Written(String::new())) else { unreachable!() };
                let text = handle.join().unwrap_or_default();
                new.push(text.clone());
                *piece = Piece::Written(text);
            }
        }
        new
    }

    /// The take's text, given the whole clip: the pieces done so far, then the rest.
    pub fn finish(self, samples: &[f32]) -> String {
        let mut parts: Vec<String> = self.pieces.into_iter().map(Piece::text).collect();
        let rest = samples.get(self.cut..).unwrap_or_default();
        parts.push(transcribe_long(lock(&self.engine).as_mut(), self.sample_rate, rest));
        parts.retain(|p| !p.is_empty());
        parts.join(" ")
    }
}

/// The middle of the longest pause after `from` (the latest, if two are as long): at
/// least 150 ms far quieter than the speech around it, and not in the last 300 ms, which
/// may be a breath inside a phrase. A gap between words isn't a pause.
fn last_pause(samples: &[f32], from: usize, sample_rate: u32) -> Option<usize> {
    let window = (sample_rate / 10) as usize;
    let hop = window / 4;
    let rms: Vec<f32> = (0..samples.len().saturating_sub(window))
        .step_by(hop)
        .map(|i| (samples[i..i + window].iter().map(|x| x * x).sum::<f32>() / window as f32).sqrt())
        .collect();
    let mut loud = rms.clone();
    loud.sort_by(f32::total_cmp);
    let speech = *loud.get(loud.len() * 9 / 10)?;
    let quiet = (speech * PAUSE_SHARE).max(0.002);
    let last = samples.len().saturating_sub(window + (sample_rate as f32 * 0.3) as usize) / hop;
    // Runs of quiet windows: (first, length). Three overlapping windows span 150 ms.
    let mut best: Option<(usize, usize)> = None;
    let mut run = 0;
    for w in from / hop..last.min(rms.len()) {
        run = if rms[w] < quiet { run + 1 } else { 0 };
        if run >= 3 && best.is_none_or(|(_, len)| run >= len) {
            best = Some((w + 1 - run, run));
        }
    }
    best.map(|(first, len)| (first + len / 2) * hop + window / 2)
}

/// The middle of the quietest 100 ms in `from..to`.
fn quietest(samples: &[f32], from: usize, to: usize, sample_rate: u32) -> usize {
    let window = (sample_rate / 10) as usize;
    (from..to.saturating_sub(window))
        .step_by(window / 4)
        .min_by(|&a, &b| {
            let energy = |i: usize| samples[i..i + window].iter().map(|x| x * x).sum::<f32>();
            energy(a).total_cmp(&energy(b))
        })
        .map_or(to, |i| i + window / 2)
}
