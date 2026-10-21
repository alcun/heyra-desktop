//! Speech engines. A sherpa-onnx transducer (Parakeet, Bark) runs anywhere
//! (macOS, Windows, Linux); an Apple engine can slot in later behind the same trait.

use std::path::Path;

use sherpa_rs::transducer::{TransducerConfig, TransducerRecognizer};

pub trait Engine: Send {
    fn transcribe(&mut self, sample_rate: u32, samples: &[f32]) -> String;
}

/// A NeMo transducer through sherpa-onnx: Parakeet TDT 0.6B v3 and the models built on it.
pub struct Transducer(TransducerRecognizer);

impl Transducer {
    pub fn load(dir: &Path) -> Result<Self, String> {
        let files = crate::model::files(dir)?;
        let file = |path: &Path| path.to_string_lossy().into_owned();
        let config = TransducerConfig {
            encoder: file(&files.encoder),
            decoder: file(&files.decoder),
            joiner: file(&files.joiner),
            tokens: file(&files.tokens),
            model_type: "nemo_transducer".into(),
            decoding_method: "greedy_search".into(),
            sample_rate: 16_000,
            feature_dim: 80,
            num_threads: 4,
            ..Default::default()
        };
        TransducerRecognizer::new(config)
            .map(Self)
            .map_err(|e| format!("could not load model from {}: {e}", dir.display()))
    }
}

impl Engine for Transducer {
    fn transcribe(&mut self, sample_rate: u32, samples: &[f32]) -> String {
        self.0.transcribe(sample_rate, samples).trim().to_string()
    }
}

// TransducerRecognizer wraps a C pointer that sherpa-onnx lets one thread use at a time.
unsafe impl Send for Transducer {}

/// Parakeet aborts the whole process past about six and a half minutes of
/// audio, and its memory grows fast well before that, so long takes go in in
/// pieces of about a minute, each cut at the quietest moment near its end.
const PIECE_SECS: f32 = 60.0;
const LOOK_BACK_SECS: f32 = 10.0;

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
