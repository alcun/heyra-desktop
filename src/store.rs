//! Everything Heyra keeps lives in one folder on this machine:
//! history.jsonl, dictionary.txt and settings.json.

use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

pub fn dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    let dir = PathBuf::from(home).join("Library/Application Support/Heyra");
    let _ = fs::create_dir_all(&dir);
    dir
}

/// Append a line to heyra.log (in the data folder). Never logs what was said.
pub fn log(line: &str) {
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(dir().join("heyra.log")) {
        let _ = writeln!(file, "{} {line}", now());
    }
}

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

// ---- history ----

#[derive(Clone, Serialize, Deserialize)]
pub struct Entry {
    pub at: u64,
    pub text: String,
    /// Seconds of speech.
    pub secs: f32,
    /// Seconds from release to text.
    #[serde(default)]
    pub took: f32,
}

impl Entry {
    pub fn words(&self) -> usize {
        self.text.split_whitespace().count()
    }
}

pub fn load_history() -> Vec<Entry> {
    let Ok(file) = fs::File::open(dir().join("history.jsonl")) else {
        return Vec::new();
    };
    BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter_map(|line| serde_json::from_str(&line).ok())
        .collect()
}

pub fn append_history(entry: &Entry) {
    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir().join("history.jsonl"))
    {
        if let Ok(line) = serde_json::to_string(entry) {
            let _ = writeln!(file, "{line}");
        }
    }
}

// ---- dictionary: one "heard => meant" rule per line ----

pub fn dictionary_path() -> PathBuf {
    let path = dir().join("dictionary.txt");
    if !path.exists() {
        let _ = fs::write(
            &path,
            "# Heyra dictionary: one rule per line, \"what it hears => what you meant\".\n\
             # Matching ignores case. Lines starting with # are ignored.\n\
             # Example:\n\
             # a cappy bar => capybara\n",
        );
    }
    path
}

pub fn load_dictionary() -> Vec<(String, String)> {
    let text = fs::read_to_string(dictionary_path()).unwrap_or_default();
    text.lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .filter_map(|l| l.split_once("=>"))
        .map(|(a, b)| (a.trim().to_string(), b.trim().to_string()))
        .filter(|(a, _)| !a.is_empty())
        .collect()
}

/// Case-insensitive replace of whole phrases.
pub fn apply_dictionary(text: &str, rules: &[(String, String)]) -> String {
    let mut out = text.to_string();
    for (heard, meant) in rules {
        let lower = out.to_lowercase();
        let needle = heard.to_lowercase();
        if lower.len() != out.len() {
            continue; // lowercasing changed byte offsets (rare non-ASCII); skip safely
        }
        let mut result = String::with_capacity(out.len());
        let mut last = 0;
        for (i, _) in lower.match_indices(&needle) {
            let before_ok = i == 0 || !lower[..i].ends_with(|c: char| c.is_alphanumeric());
            let end = i + needle.len();
            let after_ok = end == lower.len() || !lower[end..].starts_with(|c: char| c.is_alphanumeric());
            if before_ok && after_ok && i >= last {
                result.push_str(&out[last..i]);
                result.push_str(meant);
                last = end;
            }
        }
        result.push_str(&out[last..]);
        out = result;
    }
    out
}

// ---- settings ----

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Settings {
    /// Microphone name; None = the system default.
    pub mic: Option<String>,
    /// Testing only: keep the most recent clip as last-clip.wav (overwritten each
    /// time). Off by default, so no audio is ever stored.
    #[serde(default)]
    pub keep_last_clip: bool,
}

pub fn save_clip(sample_rate: u32, samples: &[f32]) {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    if let Ok(mut writer) = hound::WavWriter::create(dir().join("last-clip.wav"), spec) {
        for s in samples {
            let _ = writer.write_sample((s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16);
        }
        let _ = writer.finalize();
    }
}

pub fn load_settings() -> Settings {
    fs::read_to_string(dir().join("settings.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_settings(settings: &Settings) {
    if let Ok(json) = serde_json::to_string_pretty(settings) {
        let _ = fs::write(dir().join("settings.json"), json);
    }
}

#[cfg(test)]
mod tests {
    use super::apply_dictionary;

    #[test]
    fn replaces_whole_phrases_ignoring_case() {
        let rules = vec![("a cappy bar".to_string(), "capybara".to_string())];
        assert_eq!(apply_dictionary("I saw A Cappy Bar today", &rules), "I saw capybara today");
        assert_eq!(apply_dictionary("a cappy bars", &rules), "a cappy bars");
    }
}
