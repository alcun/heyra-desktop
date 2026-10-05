//! Speech engines. Parakeet runs anywhere (macOS, Windows, Linux); an Apple
//! engine can slot in later behind the same trait.

use std::path::{Path, PathBuf};

use sherpa_rs::transducer::{TransducerConfig, TransducerRecognizer};

pub trait Engine: Send {
    fn transcribe(&mut self, sample_rate: u32, samples: &[f32]) -> String;
}

/// NVIDIA Parakeet TDT 0.6B v3 (int8) through sherpa-onnx: the model Heyra uses.
pub struct Parakeet(TransducerRecognizer);

impl Parakeet {
    pub fn default_dir() -> PathBuf {
        if let Ok(dir) = std::env::var("HEYRA_MODEL") {
            return dir.into();
        }
        let home = std::env::var("HOME").unwrap_or_default();
        Path::new(&home)
            .join(".local/share/heyra-local/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8")
    }

    pub fn load(dir: &Path) -> Result<Self, String> {
        let file = |name: &str| dir.join(name).to_string_lossy().into_owned();
        let config = TransducerConfig {
            encoder: file("encoder.int8.onnx"),
            decoder: file("decoder.int8.onnx"),
            joiner: file("joiner.int8.onnx"),
            tokens: file("tokens.txt"),
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

impl Engine for Parakeet {
    fn transcribe(&mut self, sample_rate: u32, samples: &[f32]) -> String {
        self.0.transcribe(sample_rate, samples).trim().to_string()
    }
}

// TransducerRecognizer wraps a C pointer that sherpa-onnx lets one thread use at a time.
unsafe impl Send for Parakeet {}
