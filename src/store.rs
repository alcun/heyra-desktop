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
    let path = dir().join("heyra.log");
    if fs::metadata(&path).map(|m| m.len() > 1_000_000).unwrap_or(false) {
        let _ = fs::rename(&path, dir().join("heyra.old.log"));
    }
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
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
    let file = OpenOptions::new().create(true).append(true).open(dir().join("history.jsonl"));
    if let (Ok(mut file), Ok(line)) = (file, serde_json::to_string(entry)) {
        let _ = writeln!(file, "{line}");
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
/// The take is just "scratch that": undo the last one.
pub fn is_scratch_that(text: &str) -> bool {
    text.trim_matches(|c: char| c.is_whitespace() || ".,!?".contains(c)).eq_ignore_ascii_case("scratch that")
}

/// "new line" and "new paragraph" said anywhere become line breaks; the next word
/// starts with a capital, and commas or spaces around the words go.
pub fn line_breaks(text: &str) -> String {
    let lower = text.to_ascii_lowercase(); // same byte positions as `text`
    let word_edge = |c: Option<char>| c.is_none_or(|c| !c.is_alphanumeric());
    let mut out = String::new();
    let mut capital = false;
    let mut i = 0;
    while i < text.len() {
        let before = text[..i].chars().last();
        let hit = [("new paragraph", "\n\n"), ("new line", "\n")].into_iter().find(|(said, _)| {
            lower[i..].starts_with(said) && word_edge(before) && word_edge(text[i + said.len()..].chars().next())
        });
        if let Some((said, brk)) = hit {
            while out.ends_with([' ', ',', ';', ':']) {
                out.pop();
            }
            out.push_str(brk);
            i += said.len();
            while text[i..].starts_with([' ', ',', ';', ':', '.']) {
                i += 1;
            }
            capital = true;
            continue;
        }
        let c = text[i..].chars().next().unwrap();
        if capital && c.is_alphabetic() {
            out.extend(c.to_uppercase());
        } else {
            out.push(c);
        }
        capital &= !c.is_alphabetic();
        i += c.len_utf8();
    }
    out
}

/// Commands and prompts don't end in a full stop: drop one (but not "...").
pub fn drop_full_stop(text: &str) -> String {
    match text.strip_suffix('.') {
        Some(rest) if !rest.ends_with('.') => rest.to_string(),
        _ => text.to_string(),
    }
}

/// A take ending in "press enter" is the words before it, then Enter.
pub fn press_enter(text: &str) -> Option<String> {
    let trimmed = text.trim_end_matches(|c: char| c.is_whitespace() || ".,!?;:".contains(c));
    let cut = trimmed.len().checked_sub("press enter".len())?;
    if !trimmed.is_char_boundary(cut) || !trimmed[cut..].eq_ignore_ascii_case("press enter") {
        return None;
    }
    let before = &trimmed[..cut];
    if before.chars().last().is_some_and(|c| c.is_alphanumeric()) {
        return None; // "…express enter": not the command
    }
    Some(before.trim_end_matches(|c: char| c.is_whitespace() || ",;:".contains(c)).to_string())
}

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

#[derive(Clone, Serialize, Deserialize)]
pub struct Settings {
    /// Microphone name; None = the system default.
    pub mic: Option<String>,
    /// Testing only: keep the most recent clip as last-clip.wav (overwritten each
    /// time). Off by default, so no audio is ever stored.
    #[serde(default)]
    pub keep_last_clip: bool,
    /// Open at login was turned on once, on the first run; after that it's the user's switch.
    #[serde(default)]
    pub login_offered: bool,
    /// A push-to-talk button of the user's choosing, alongside fn.
    #[serde(default)]
    pub button: Option<Button>,
    /// The soft click when a take starts and ends.
    #[serde(default = "yes")]
    pub sounds: bool,
    /// Mute the Mac's sound output while a take is recording.
    #[serde(default)]
    pub mute_while_talking: bool,
    /// The welcome window has been shown.
    #[serde(default)]
    pub onboarded: bool,
    /// Keep the microphone open between takes: instant, and the first word is caught.
    /// Off, it opens only while recording, so macOS's mic light goes out.
    #[serde(default = "yes")]
    pub keep_mic_ready: bool,
}

fn yes() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Self { mic: None, keep_last_clip: false, login_offered: false, button: None, sounds: true, mute_while_talking: false, onboarded: false, keep_mic_ready: true }
    }
}

/// A key (with the modifiers held with it) or an extra mouse button.
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub struct Button {
    pub mouse: bool,
    /// Key code, or mouse button number (2 = middle, 3 and up = side buttons).
    pub code: i64,
    /// Command, control, option and shift, as CGEventFlags bits.
    pub mods: u64,
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
    use super::{apply_dictionary, drop_full_stop, is_scratch_that, line_breaks, press_enter};

    #[test]
    fn voice_commands() {
        assert!(is_scratch_that("Scratch that."));
        assert!(!is_scratch_that("Scratch that idea, try another."));
        assert_eq!(line_breaks("Fix the bug new line then run the tests."), "Fix the bug\nThen run the tests.");
        assert_eq!(line_breaks("Dear Sam. New paragraph. Thanks for this."), "Dear Sam.\n\nThanks for this.");
        assert_eq!(line_breaks("A newline character"), "A newline character");
        assert_eq!(drop_full_stop("run the tests."), "run the tests");
        assert_eq!(drop_full_stop("wait..."), "wait...");
    }

    #[test]
    fn press_enter_at_the_end() {
        assert_eq!(press_enter("Fix the bug. Press enter."), Some("Fix the bug.".into()));
        assert_eq!(press_enter("ship it, press enter"), Some("ship it".into()));
        assert_eq!(press_enter("Press Enter."), Some("".into()));
        assert_eq!(press_enter("Don't press enter yet, I'm thinking."), None);
        assert_eq!(press_enter("Express enter"), None);
    }

    #[test]
    fn replaces_whole_phrases_ignoring_case() {
        let rules = vec![("a cappy bar".to_string(), "capybara".to_string())];
        assert_eq!(apply_dictionary("I saw A Cappy Bar today", &rules), "I saw capybara today");
        assert_eq!(apply_dictionary("a cappy bars", &rules), "a cappy bars");
    }
}
