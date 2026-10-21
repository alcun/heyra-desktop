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
    note: "Every word, in English and 24 European languages",
    dir: "sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8",
    mb: 487,
    sha256: "5793d0fd397c5778d2cf2126994d58e9d56b1be7c04d13c7a15bb1b4eafb16bf",
    sources: &[
        "https://github.com/alcun/heyra-desktop/releases/download/model-parakeet-v3/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8.tar.bz2",
        "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8.tar.bz2",
    ],
};

pub const BARK_CLEAN: Model = Model {
    id: "bark-3.0-clean",
    name: "Bark 3.0 Clean",
    note: "English, without the um and uh",
    dir: "underdog-bark-3.0-clean-int8",
    mb: 399,
    sha256: "2ef1a099b74a52cf28189b6f4bee407e8232065d4d615f6ecabcd687d66c2eb3",
    sources: &["https://github.com/alcun/heyra-desktop/releases/download/model-bark-3.0-clean/underdog-bark-3.0-clean-int8.tar.bz2"],
};

pub const BARK_PLUS: Model = Model {
    id: "bark-3.0-plus",
    name: "Bark 3.0 Plus",
    note: "English, every word",
    dir: "underdog-bark-3.0-plus-int8",
    mb: 398,
    sha256: "1f22bb0799230dc621385f3e140ca98365deb507c9320967226738e352226079",
    sources: &["https://github.com/alcun/heyra-desktop/releases/download/model-bark-3.0-plus/underdog-bark-3.0-plus-int8.tar.bz2"],
};

pub const ALL: [&Model; 3] = [&BARK_CLEAN, &BARK_PLUS, &PARAKEET];

/// The model a new install starts with: Bark Clean on a Mac set to English,
/// Parakeet (which finds the language itself) on any other.
pub fn default() -> &'static Model {
    if first_language().is_none_or(|l| l.starts_with("en")) { &BARK_CLEAN } else { &PARAKEET }
}

/// The Mac's first preferred language, such as "en-GB".
fn first_language() -> Option<String> {
    use core_foundation::array::{CFArray, CFArrayRef};
    use core_foundation::base::TCFType;
    use core_foundation::string::CFString;
    unsafe extern "C" {
        fn CFLocaleCopyPreferredLanguages() -> CFArrayRef;
    }
    let languages: CFArray<CFString> = unsafe {
        let array = CFLocaleCopyPreferredLanguages();
        if array.is_null() {
            return None;
        }
        CFArray::wrap_under_create_rule(array)
    };
    languages.iter().next().map(|l| l.to_string())
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
    /// Installs from before the choice existed keep Parakeet, which they already have.
    pub fn saved() -> Choice {
        let settings = crate::store::load_settings();
        match (settings.model.as_deref(), settings.model_folder) {
            (Some("folder"), Some(dir)) => Choice::Folder(dir),
            (Some(id), _) if by_id(id).is_some() => Choice::Listed(by_id(id).unwrap().id),
            (None, _) if settings.onboarded || PARAKEET.find().is_some() => Choice::Listed(PARAKEET.id),
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

    /// Move a freshly unpacked folder into place, replacing any broken copy.
    fn place(&self, unpacked: &Path, dir: &Path) -> Result<PathBuf, String> {
        files(unpacked).map_err(|_| "the model archive was missing files".to_string())?;
        let target = dir.join(self.dir);
        if target.exists() {
            // Another download of the same model finished first: keep whichever is whole.
            if files(&target).is_ok() {
                return Ok(target);
            }
            fs::remove_dir_all(&target).map_err(|e| e.to_string())?;
        }
        fs::rename(unpacked, &target).map_err(|e| format!("couldn't put the model in place: {e}"))?;
        Ok(target)
    }

    /// Delete the downloaded copy.
    pub fn remove(&self) -> Result<(), String> {
        let dir = models_dir().join(self.dir);
        if dir.exists() { fs::remove_dir_all(dir).map_err(|e| e.to_string()) } else { Ok(()) }
    }

    /// Download, verify and unpack. `progress(done, total)` is called as bytes
    /// arrive; setting `cancel` stops it.
    pub fn download(&self, cancel: &AtomicBool, progress: impl Fn(u64, u64)) -> Result<PathBuf, String> {
        self.download_from(self.sources, &models_dir(), cancel, progress)
    }

    fn download_from(
        &self,
        sources: &[&str],
        dir: &Path,
        cancel: &AtomicBool,
        progress: impl Fn(u64, u64),
    ) -> Result<PathBuf, String> {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        // Every download has its own partial file and unpacking folder, so a replaced
        // download still finishing can't touch this one, and a model only appears under
        // its own name once it has unpacked completely.
        let pid = std::process::id();
        clear_leftovers(dir, pid);
        let unique = format!("{pid}-{}", DOWNLOADS.fetch_add(1, Ordering::Relaxed));
        let archive = dir.join(format!("{}.{unique}.tar.bz2.part", self.dir));
        let mut last_error = String::new();
        for url in sources {
            match fetch(url, &archive, self.sha256, cancel, &progress) {
                Ok(()) => {
                    let aside = dir.join(format!(".unpacking-{unique}"));
                    let placed = unpack(&archive, &aside).and_then(|()| self.place(&aside.join(self.dir), dir));
                    let _ = fs::remove_file(&archive);
                    let _ = fs::remove_dir_all(&aside);
                    return placed;
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

/// Partial files and unpacking folders left by an earlier run that quit mid-download.
/// This run's own, which may still be in use, are left alone.
fn clear_leftovers(dir: &Path, pid: u32) {
    let mine = format!(".{pid}-");
    let mine_aside = format!(".unpacking-{pid}-");
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let leftover = (name.ends_with(".tar.bz2.part") && !name.contains(&mine))
            || (name.starts_with(".unpacking-") && !name.starts_with(&mine_aside));
        if leftover {
            let path = entry.path();
            let _ = if path.is_dir() { fs::remove_dir_all(&path) } else { fs::remove_file(&path) };
        }
    }
}

/// Numbers each download, for its own partial file.
static DOWNLOADS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

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

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    /// Serve the Bark archive locally (HEYRA_TEST_ARCHIVE_URL), then download,
    /// verify, unpack and load it; and check a cancelled download stops.
    #[test]
    fn downloads_bark_from_a_mirror() {
        let Ok(url) = std::env::var("HEYRA_TEST_ARCHIVE_URL") else { return };
        let dir = std::env::temp_dir().join("heyra-model-test");
        let _ = std::fs::remove_dir_all(&dir);
        let bad = "http://127.0.0.1:9/nothing-here.tar.bz2";
        let cancel = AtomicBool::new(false);
        let got = super::BARK_CLEAN.download_from(&[bad, &url], &dir, &cancel, |_, _| {}).expect("download");
        assert!(super::files(&got).is_ok());
        crate::engine::Transducer::load(&got).expect("load");
        let cancel = AtomicBool::new(false);
        let err = super::BARK_CLEAN
            .download_from(&[&url], &dir.join("again"), &cancel, |done, _| {
                if done > 10_000_000 {
                    cancel.store(true, Ordering::Relaxed);
                }
            })
            .unwrap_err();
        assert_eq!(err, super::CANCELLED);
        let left: Vec<_> = std::fs::read_dir(dir.join("again")).unwrap().flatten().collect();
        assert!(left.is_empty(), "a cancelled download left {left:?}");

        // Two downloads of the same model at once (chosen, replaced, chosen again) both
        // finish, and a partial file from a run that died is cleared.
        let both = dir.join("both");
        std::fs::create_dir_all(&both).unwrap();
        std::fs::write(both.join("underdog-bark-3.0-clean-int8.99999-0.tar.bz2.part"), b"old").unwrap();
        let results: Vec<_> = std::thread::scope(|scope| {
            let runs: Vec<_> = (0..2)
                .map(|_| scope.spawn(|| super::BARK_CLEAN.download_from(&[&url], &both, &AtomicBool::new(false), |_, _| {})))
                .collect();
            runs.into_iter().map(|r| r.join().unwrap()).collect()
        });
        for r in &results {
            assert!(super::files(r.as_ref().expect("both downloads finish")).is_ok());
        }
        let names: Vec<String> =
            std::fs::read_dir(&both).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["underdog-bark-3.0-clean-int8"], "leftovers: {names:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reads_the_macs_language() {
        let language = super::first_language().expect("a preferred language");
        assert!(language.len() >= 2, "{language}");
        println!("first language: {language}, default: {}", super::default().name);
    }
}
