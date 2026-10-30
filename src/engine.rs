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
const OVERLAP_SECS: f32 = 1.0;
/// Drafts while talking only while the words not yet in a piece are this short: a
/// draft the engine is still on when the take ends is waited for.
const DRAFT_MAX_SECS: f32 = 8.0;
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
    /// Where each piece ends in the take.
    ends: Vec<usize>,
    /// The words not yet in a piece, written roughly to show while talking: the cut
    /// they start from, how far they go, and the engine at work or the text.
    draft: Option<(usize, usize, Piece)>,
    /// The latest finished draft, shown until the next one is ready.
    drafted: Option<(usize, usize, String)>,
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
        Self { engine, sample_rate, cut: 0, looked: 0, pieces: Vec::new(), ends: Vec::new(), draft: None, drafted: None }
    }

    /// How much audio before a cut the next piece hears again. The engine often skips
    /// the first words of audio that starts straight into speech; a second of what came
    /// before brings them back, and the words heard twice are dropped (see `join`).
    fn overlap(&self) -> usize {
        (OVERLAP_SECS * self.sample_rate as f32) as usize
    }

    /// The audio a piece hears, given the take from up to a second before the cut: after
    /// the first piece, from the gap between words in that second, so it doesn't begin
    /// halfway through one. Also where that is in the take.
    fn with_lead(&self, mut audio: Vec<f32>) -> (Vec<f32>, usize) {
        let from = self.cut.saturating_sub(self.overlap());
        let before = self.cut - from;
        let latest = before.saturating_sub(self.sample_rate as usize / 2);
        if latest == 0 {
            return (audio, self.cut);
        }
        let gap = quietest(&audio, 0, latest.min(audio.len()), self.sample_rate);
        audio.drain(..gap);
        (audio, from + gap)
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
        if len < self.looked + step || self.busy() {
            return;
        }
        self.looked = len;
        let (mut chunk, from) = self.with_lead(peek(self.cut.saturating_sub(self.overlap())));
        let lead = self.cut - from;
        if chunk.len() < lead + min {
            return self.redraft(len, chunk, from);
        }
        let audio = &chunk[lead..];
        let end = match last_pause(audio, min - (3.0 * rate) as usize, self.sample_rate) {
            Some(end) => end,
            None if audio.len() >= max => quietest(audio, max - (5.0 * rate) as usize, max, self.sample_rate),
            None => return self.redraft(len, chunk, from),
        };
        chunk.truncate(lead + end);
        self.cut += end;
        self.ends.push(self.cut);
        let (engine, rate) = (self.engine.clone(), self.sample_rate);
        self.pieces.push(Piece::Writing(std::thread::spawn(move || lock(&engine).transcribe(rate, &chunk))));
    }

    /// The engine is writing a piece or a draft.
    fn busy(&self) -> bool {
        let working = |p: &Piece| matches!(p, Piece::Writing(h) if !h.is_finished());
        self.pieces.last().is_some_and(working) || self.draft.as_ref().is_some_and(|(_, _, p)| working(p))
    }

    /// Write the words not yet in a piece again, when there's a second more of them and
    /// they're short enough not to keep the engine from the end of the take for long.
    fn redraft(&mut self, len: usize, audio: Vec<f32>, from: usize) {
        let rate = self.sample_rate as f32;
        let second = rate as usize;
        let fresh = len >= self.cut + second && self.draft.as_ref().is_none_or(|(_, to, _)| len >= to + second);
        if self.busy() || !fresh || len - self.cut > (DRAFT_MAX_SECS * rate) as usize {
            return;
        }
        let (engine, rate) = (self.engine.clone(), self.sample_rate);
        self.draft = Some((from, len, Piece::Writing(std::thread::spawn(move || lock(&engine).transcribe(rate, &audio)))));
    }

    /// Everything written so far, the draft of the latest words included: for showing
    /// while talking. Takes in what the engine has finished.
    pub fn so_far(&mut self) -> String {
        self.written();
        if let Some((from, to, piece)) = &mut self.draft
            && let Piece::Writing(h) = piece
            && h.is_finished()
        {
            let Piece::Writing(handle) = std::mem::replace(piece, Piece::Written(String::new())) else { unreachable!() };
            self.drafted = Some((*from, *to, handle.join().unwrap_or_default()));
        }
        let mut texts: Vec<&str> = Vec::new();
        let mut written_to = 0;
        for (piece, end) in self.pieces.iter().zip(&self.ends) {
            let Piece::Written(text) = piece else { break };
            texts.push(text);
            written_to = *end;
        }
        // The draft starts a second before a cut; it's shown until a piece covers it.
        if let Some((from, to, text)) = &self.drafted
            && *to > written_to
            && from + self.overlap() >= written_to
        {
            texts.push(text);
        }
        let mut out: Vec<String> = Vec::new();
        for (i, text) in texts.iter().enumerate() {
            out.push(if i == 0 { text.to_string() } else { join(texts[i - 1], text) });
        }
        out.retain(|t| !t.is_empty());
        out.join(" ")
    }

    /// The text of pieces finished since the last call, in order: the take so far, for
    /// showing as it's written.
    pub fn written(&mut self) -> Vec<String> {
        let mut new = Vec::new();
        let mut before = String::new();
        for piece in &mut self.pieces {
            if matches!(piece, Piece::Writing(h) if h.is_finished()) {
                let Piece::Writing(handle) = std::mem::replace(piece, Piece::Written(String::new())) else { unreachable!() };
                let text = handle.join().unwrap_or_default();
                new.push(join(&before, &text));
                *piece = Piece::Written(text);
            }
            if let Piece::Written(text) = piece {
                before.clone_from(text);
            }
        }
        new
    }

    /// The take's text, given the whole clip: the pieces done so far, then the rest.
    pub fn finish(self, samples: &[f32]) -> String {
        let (rest, _) = self.with_lead(samples.get(self.cut.saturating_sub(self.overlap())..).unwrap_or_default().to_vec());
        let rest = transcribe_long(lock(&self.engine).as_mut(), self.sample_rate, &rest);
        let mut texts: Vec<String> = self.pieces.into_iter().map(Piece::text).collect();
        texts.push(rest);
        let mut parts = Vec::new();
        for (i, text) in texts.iter().enumerate() {
            parts.push(if i == 0 { text.clone() } else { join(&texts[i - 1], text) });
        }
        parts.retain(|p| !p.is_empty());
        parts.join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::join;

    #[test]
    fn drops_words_heard_twice() {
        assert_eq!(join("Turn them around.", "Turn them around at a rapid pace."), "at a rapid pace.");
        assert_eq!(join("a dim phosphor bulb glowed.", "Bob glowed. When darkness fell."), "When darkness fell.");
        assert_eq!(join("We went home", "Home we stayed."), "we stayed.");
        assert_eq!(join("Nothing alike.", "Fresh words here."), "Fresh words here.");
        assert_eq!(join("", "First piece."), "First piece.");
    }
}

/// `next` without the words at its start that repeat the end of `before`: the overlap
/// between two pieces, heard twice. Compared without case or punctuation.
fn join(before: &str, next: &str) -> String {
    let word = |w: &str| w.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect::<String>();
    let tail: Vec<String> = before.split_whitespace().map(word).collect();
    let head: Vec<&str> = next.split_whitespace().collect();
    // The repeat can begin with a word or two clipped by the overlap's start ("bulb"
    // heard as "Bob"): those go too, as long as whole words after them match.
    let heard_twice = (0..=2)
        .find_map(|clipped| {
            (1..=head.len().saturating_sub(clipped).min(tail.len()).min(8))
                .rev()
                .find(|&k| tail[tail.len() - k..].iter().zip(&head[clipped..clipped + k]).all(|(a, b)| *a == word(b)))
                .map(|k| clipped + k)
        })
        .unwrap_or(0);
    let kept = head[heard_twice..].join(" ");
    // A piece that began mid-sentence, now that its first words are gone: no capital.
    match (heard_twice > 0, kept.chars().next()) {
        (true, Some(c)) if c.is_uppercase() && !before.trim_end().ends_with(['.', '?', '!']) => {
            c.to_lowercase().chain(kept.chars().skip(1)).collect()
        }
        _ => kept,
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
