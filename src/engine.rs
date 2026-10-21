//! Speech engines. Parakeet runs anywhere (macOS, Windows, Linux); an Apple
//! engine can slot in later behind the same trait.

use std::path::Path;

use sherpa_onnx::{OfflineRecognizer, OfflineRecognizerConfig, OfflineTransducerModelConfig};

pub trait Engine: Send {
    fn transcribe(&mut self, sample_rate: u32, samples: &[f32]) -> String;
}

/// NVIDIA Parakeet TDT 0.6B v3 (int8) through sherpa-onnx: the model Heyra uses.
pub struct Parakeet(OfflineRecognizer);

impl Parakeet {
    pub fn load(dir: &Path) -> Result<Self, String> {
        let file = |name: &str| Some(dir.join(name).to_string_lossy().into_owned());
        let mut config = OfflineRecognizerConfig::default();
        config.model_config.transducer = OfflineTransducerModelConfig {
            encoder: file("encoder.int8.onnx"),
            decoder: file("decoder.int8.onnx"),
            joiner: file("joiner.int8.onnx"),
        };
        config.model_config.tokens = file("tokens.txt");
        config.model_config.model_type = Some("nemo_transducer".into());
        config.model_config.num_threads = 4;
        config.decoding_method = Some("greedy_search".into());
        OfflineRecognizer::create(&config)
            .map(Self)
            .ok_or_else(|| format!("could not load model from {}", dir.display()))
    }
}

impl Engine for Parakeet {
    fn transcribe(&mut self, sample_rate: u32, samples: &[f32]) -> String {
        let stream = self.0.create_stream();
        stream.accept_waveform(sample_rate as i32, samples);
        self.0.decode(&stream);
        stream.get_result().map(|r| r.text.trim().to_string()).unwrap_or_default()
    }
}

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
