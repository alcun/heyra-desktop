//! The speech model: found on disk, or downloaded once on first run, checked
//! against a known SHA-256 and unpacked. This download is the only time Heyra
//! touches the network.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

pub const NAME: &str = "sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8";
const SHA256: &str = "5793d0fd397c5778d2cf2126994d58e9d56b1be7c04d13c7a15bb1b4eafb16bf";
const FILES: [&str; 4] = ["encoder.int8.onnx", "decoder.int8.onnx", "joiner.int8.onnx", "tokens.txt"];

/// Our mirror first, then the sherpa-onnx release it was copied from.
fn sources() -> [String; 2] {
    [
        format!("https://github.com/alcun/heyra-desktop/releases/download/model-parakeet-v3/{NAME}.tar.bz2"),
        format!("https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/{NAME}.tar.bz2"),
    ]
}

fn models_dir() -> PathBuf {
    crate::store::dir().join("models")
}

fn complete(dir: &Path) -> bool {
    FILES.iter().all(|f| dir.join(f).is_file())
}

/// Where the model is, if it's already here.
pub fn find() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("HEYRA_MODEL") {
        return Some(dir.into());
    }
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_default());
    [models_dir().join(NAME), home.join(".local/share/heyra-local").join(NAME)]
        .into_iter()
        .find(|d| complete(d))
}

/// Download, verify and unpack. `progress(done, total)` is called as bytes arrive.
pub fn download(progress: impl Fn(u64, u64)) -> Result<PathBuf, String> {
    let dir = models_dir();
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let archive = dir.join(format!("{NAME}.tar.bz2.part"));
    let mut last_error = String::new();
    for url in sources() {
        match fetch(&url, &archive, &progress) {
            Ok(()) => {
                let unpacked = unpack(&archive, &dir);
                let _ = fs::remove_file(&archive);
                unpacked?;
                let target = dir.join(NAME);
                return if complete(&target) {
                    Ok(target)
                } else {
                    Err("the model archive was missing files".into())
                };
            }
            Err(e) => {
                crate::store::log(&format!("model download from {url} failed: {e}"));
                last_error = e;
            }
        }
    }
    let _ = fs::remove_file(&archive);
    Err(format!("couldn't download the speech model ({last_error}). Check your connection and reopen Heyra."))
}

fn fetch(url: &str, to: &Path, progress: &impl Fn(u64, u64)) -> Result<(), String> {
    let response = ureq::get(url).call().map_err(|e| e.to_string())?;
    let total = response.body().content_length().unwrap_or(0);
    let mut reader = response.into_body().into_reader();
    let mut file = fs::File::create(to).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    let mut done = 0u64;
    loop {
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
    if digest != SHA256 {
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
