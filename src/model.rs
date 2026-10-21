//! The speech models: Heyra's own list, or a folder of the person's choosing.
//! A listed model is found on disk, or downloaded once, checked against a known
//! SHA-256 and unpacked. These downloads are the only time Heyra touches the network.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use sha2::{Digest, Sha256};

pub struct Model {
    /// Kept in settings.json.
    pub id: &'static str,
    pub name: &'static str,
    /// One short line on what it's good at.
    pub note: &'static str,
    /// The folder inside the archive, and under models/.
    dir: &'static str,
    /// Download size, for the list.
    pub mb: u32,
    sha256: &'static str,
    /// Tried in order.
    sources: &'static [&'static str],
}

pub const PARAKEET: Model = Model {
    id: "parakeet-v3",
    name: "Parakeet v3",
    note: "English and 24 European languages",
    dir: "sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8",
    mb: 465,
    sha256: "5793d0fd397c5778d2cf2126994d58e9d56b1be7c04d13c7a15bb1b4eafb16bf",
    sources: &[
        "https://github.com/alcun/heyra-desktop/releases/download/model-parakeet-v3/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8.tar.bz2",
        "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8.tar.bz2",
    ],
};

pub const ALL: [&Model; 1] = [&PARAKEET];

/// The model a new install starts with.
pub fn default() -> &'static Model {
    &PARAKEET
}

pub fn by_id(id: &str) -> Option<&'static Model> {
    ALL.into_iter().find(|m| m.id == id)
}

/// What Heyra listens with: a listed model, or a folder the person picked.
#[derive(Clone, PartialEq, Debug)]
pub enum Choice {
    Listed(&'static str),
    Folder(PathBuf),
}

impl Choice {
    /// From settings.json; a model no longer listed falls back to the default.
    pub fn saved() -> Choice {
        let settings = crate::store::load_settings();
        match (settings.model.as_deref(), settings.model_folder) {
            (Some("folder"), Some(dir)) => Choice::Folder(dir),
            (Some(id), _) if by_id(id).is_some() => Choice::Listed(by_id(id).unwrap().id),
            _ => Choice::Listed(default().id),
        }
    }

    pub fn save(&self) {
        let settings = crate::store::load_settings();
        crate::store::save_settings(&match self {
            Choice::Listed(id) => crate::store::Settings { model: Some(id.to_string()), ..settings },
            Choice::Folder(dir) => {
                crate::store::Settings { model: Some("folder".into()), model_folder: Some(dir.clone()), ..settings }
            }
        });
    }

    pub fn name(&self) -> String {
        match self {
            Choice::Listed(id) => by_id(id).map_or(id.to_string(), |m| m.name.to_string()),
            Choice::Folder(dir) => dir.file_name().map_or("Your model".into(), |n| n.to_string_lossy().into_owned()),
        }
    }
}

/// The four files sherpa-onnx needs for a transducer such as Parakeet.
pub struct Files {
    pub encoder: PathBuf,
    pub decoder: PathBuf,
    pub joiner: PathBuf,
    pub tokens: PathBuf,
}

/// Find the model files in a folder: encoder*.onnx, decoder*.onnx, joiner*.onnx
/// and tokens.txt. Where there's a choice, the int8 one.
pub fn files(dir: &Path) -> Result<Files, String> {
    let names: Vec<String> = fs::read_dir(dir)
        .map_err(|e| format!("couldn't open {}: {e}", dir.display()))?
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .collect();
    let pick = |part: &str| -> Result<PathBuf, String> {
        let mut found: Vec<&String> =
            names.iter().filter(|n| n.starts_with(part) && n.ends_with(".onnx")).collect();
        found.sort_by_key(|n| !n.contains("int8"));
        found.first().map(|n| dir.join(n)).ok_or_else(|| format!("no {part}.onnx in {}", dir.display()))
    };
    let tokens = dir.join("tokens.txt");
    if !tokens.is_file() {
        return Err(format!("no tokens.txt in {}", dir.display()));
    }
    Ok(Files { encoder: pick("encoder")?, decoder: pick("decoder")?, joiner: pick("joiner")?, tokens })
}

fn models_dir() -> PathBuf {
    crate::store::dir().join("models")
}

impl Model {
    /// Where it is, if it's already here.
    pub fn find(&self) -> Option<PathBuf> {
        let home = PathBuf::from(std::env::var("HOME").unwrap_or_default());
        [models_dir().join(self.dir), home.join(".local/share/heyra-local").join(self.dir)]
            .into_iter()
            .find(|d| files(d).is_ok())
    }

    /// Delete the downloaded copy.
    pub fn remove(&self) -> Result<(), String> {
        let dir = models_dir().join(self.dir);
        if dir.exists() { fs::remove_dir_all(dir).map_err(|e| e.to_string()) } else { Ok(()) }
    }

    /// Download, verify and unpack. `progress(done, total)` is called as bytes
    /// arrive; setting `cancel` stops it.
    pub fn download(&self, cancel: &AtomicBool, progress: impl Fn(u64, u64)) -> Result<PathBuf, String> {
        let dir = models_dir();
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let archive = dir.join(format!("{}.tar.bz2.part", self.dir));
        let mut last_error = String::new();
        for url in self.sources {
            match fetch(url, &archive, self.sha256, cancel, &progress) {
                Ok(()) => {
                    let unpacked = unpack(&archive, &dir);
                    let _ = fs::remove_file(&archive);
                    unpacked?;
                    let target = dir.join(self.dir);
                    files(&target).map_err(|_| "the model archive was missing files".to_string())?;
                    return Ok(target);
                }
                Err(e) if cancel.load(Ordering::Relaxed) => {
                    let _ = fs::remove_file(&archive);
                    return Err(e);
                }
                Err(e) => {
                    crate::store::log(&format!("model download from {url} failed: {e}"));
                    last_error = e;
                }
            }
        }
        let _ = fs::remove_file(&archive);
        Err(format!("couldn't download {} ({last_error}). Check your connection and try again.", self.name))
    }
}

/// The folder to load for a choice, if it's on disk.
pub fn find(choice: &Choice) -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("HEYRA_MODEL") {
        return Some(dir.into());
    }
    match choice {
        Choice::Listed(id) => by_id(id)?.find(),
        Choice::Folder(dir) => Some(dir.clone()),
    }
}

pub const CANCELLED: &str = "cancelled";

fn fetch(url: &str, to: &Path, sha256: &str, cancel: &AtomicBool, progress: &impl Fn(u64, u64)) -> Result<(), String> {
    let response = ureq::get(url).call().map_err(|e| e.to_string())?;
    let total = response.body().content_length().unwrap_or(0);
    let mut reader = response.into_body().into_reader();
    let mut file = fs::File::create(to).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    let mut done = 0u64;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(CANCELLED.into());
        }
        let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        done += n as u64;
        progress(done, total);
    }
    let digest: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if digest != sha256 {
        return Err(format!("checksum mismatch ({digest})"));
    }
    Ok(())
}

fn unpack(archive: &Path, into: &Path) -> Result<(), String> {
    let file = fs::File::open(archive).map_err(|e| e.to_string())?;
    tar::Archive::new(bzip2::read::BzDecoder::new(file))
        .unpack(into)
        .map_err(|e| format!("couldn't unpack the model: {e}"))
}
