//! The speech models: Heyra's own list, or a folder of the person's choosing.
//! A listed model is found on disk, or downloaded once, checked against a known
//! SHA-256 and unpacked. These downloads are the only time Heyra touches the network.

use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
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
    note: "25 languages",
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
    note: "English, no um or uh",
    dir: "underdog-bark-3.0-clean-int8",
    mb: 399,
    sha256: "2ef1a099b74a52cf28189b6f4bee407e8232065d4d615f6ecabcd687d66c2eb3",
    sources: &["https://github.com/alcun/heyra-desktop/releases/download/model-bark-3.0-clean/underdog-bark-3.0-clean-int8.tar.bz2"],
};

pub const FLUID: Model = Model {
    id: "fluid-speech-mini",
    name: "Fluid Speech Mini",
    note: "English, every word",
    dir: "fluidvoice-fluid-speech-mini",
    mb: 312,
    sha256: "20caa640d57487093d6c4941a2f5261608d28f77f07c4ea90952b5efb9293a0c",
    sources: &["https://github.com/alcun/heyra-desktop/releases/download/model-fluid-speech-mini/fluidvoice-fluid-speech-mini.tar.bz2"],
};

pub const ALL: [&Model; 3] = [&BARK_CLEAN, &FLUID, &PARAKEET];

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

/// Whether a folder's model runs: Heyra loads it in a separate process first, so a model
/// using something the engine can't run fails there instead of taking Heyra down.
pub fn check_folder(dir: &Path) -> Result<(), String> {
    files(dir)?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut child = std::process::Command::new(exe)
        .arg("--check-model")
        .arg(dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    // A model that hangs while loading is as unusable as one that crashes.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err("That model took too long to load.".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    };
    if status.success() {
        return Ok(());
    }
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        let _ = pipe.read_to_string(&mut stderr);
    }
    let said = stderr.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").to_string();
    crate::store::log(&format!("model folder {} failed its check: {said}", dir.display()));
    Err(if status.code() == Some(2) {
        format!("That model didn't load: {said}")
    } else {
        "That model uses something Heyra's engine can't run.".into()
    })
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

    /// Download, verify and unpack, resuming and retrying by itself. `on` hears how it's
    /// going; setting `cancel` stops it.
    pub fn download(&self, cancel: &AtomicBool, on: impl Fn(Progress)) -> Result<PathBuf, Failure> {
        self.download_from(self.sources, &models_dir(), cancel, on, &RETRY_WAITS)
    }

    fn download_from(
        &self,
        sources: &[&str],
        dir: &Path,
        cancel: &AtomicBool,
        on: impl Fn(Progress),
        waits: &[u64],
    ) -> Result<PathBuf, Failure> {
        fs::create_dir_all(dir).map_err(|e| Failure::Other(e.to_string()))?;
        clear_leftovers(dir);
        // The archive, then the unpacked model beside it: about two and a half times the download.
        let need = self.mb as u64 * 5 / 2 + 50;
        if free_mb(dir).is_some_and(|free| free < need) {
            return Err(Failure::NoSpace(need));
        }
        // One partial file per model and checksum, kept between attempts and runs so a
        // download resumes where it stopped. Locked while in use: if a replaced download
        // of the same model still holds it, this one starts a file of its own.
        let stable = dir.join(format!("{}.{}.partial", self.dir, &self.sha256[..12]));
        let (partial, file) = match open_locked(&stable) {
            Some(file) => (stable, file),
            None => {
                let own = dir.join(format!("{}.{}-{}.partial", self.dir, std::process::id(), DOWNLOADS.fetch_add(1, Ordering::Relaxed)));
                let file = fs::OpenOptions::new().create(true).truncate(true).read(true).write(true).open(&own)
                    .map_err(|e| Failure::Other(e.to_string()))?;
                (own, file)
            }
        };
        let mut damaged_once = false;
        let mut attempt = 0;
        let result = loop {
            // The first source that delivers the whole file wins; a source without it is
            // skipped, and a network failure moves on to the next source too.
            // None once a source has delivered the whole file.
            let mut last = Some(Failure::Unavailable);
            for url in sources {
                match fetch(url, &file, cancel, &on) {
                    Ok(()) => {
                        last = if file_matches(&file, self.sha256) { None } else { Some(Failure::Damaged) };
                        break;
                    }
                    Err(Failure::Unavailable) => crate::store::log(&format!("model {} not at {url}", self.id)),
                    Err(e) => {
                        crate::store::log(&format!("model download from {url}: {e:?}"));
                        let retry_elsewhere = matches!(e, Failure::Network(_));
                        last = Some(e);
                        if !retry_elsewhere {
                            break;
                        }
                    }
                }
            }
            let Some(last) = last else { break Ok(()) };
            match last {
                Failure::Damaged if !damaged_once => {
                    damaged_once = true;
                    let _ = file.set_len(0);
                }
                Failure::Network(reason) if attempt < waits.len() => {
                    let wait = waits[attempt];
                    attempt += 1;
                    on(Progress::Retrying { reason: reason.clone(), at: std::time::Instant::now() + std::time::Duration::from_secs(wait) });
                    for _ in 0..wait * 10 {
                        if cancel.load(Ordering::Relaxed) {
                            break;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(100));
                    }
                    if cancel.load(Ordering::Relaxed) {
                        break Err(Failure::Cancelled);
                    }
                }
                other => break Err(other),
            }
        };
        if let Err(e) = result {
            // Kept for next time, unless it's damaged or this download's own file.
            if matches!(e, Failure::Damaged) || !partial.ends_with(format!("{}.{}.partial", self.dir, &self.sha256[..12])) {
                let _ = fs::remove_file(&partial);
            }
            return Err(e);
        }
        let aside = dir.join(format!(".unpacking-{}-{}", std::process::id(), DOWNLOADS.fetch_add(1, Ordering::Relaxed)));
        let placed = unpack(&partial, &aside).and_then(|()| self.place(&aside.join(self.dir), dir)).map_err(Failure::Other);
        let _ = fs::remove_dir_all(&aside);
        if placed.is_ok() {
            let _ = fs::remove_file(&partial);
        }
        placed
    }
}

/// How a download is going.
pub enum Progress {
    Bytes(u64, u64),
    /// It stopped for `reason`, and tries again by itself at `at`.
    Retrying { reason: String, at: std::time::Instant },
}

/// Why a download stopped, said plainly.
#[derive(Debug, Clone, PartialEq)]
pub enum Failure {
    Cancelled,
    /// The connection or the server: worth trying again.
    Network(String),
    Unavailable,
    /// Megabytes of free space it needs.
    NoSpace(u64),
    Damaged,
    Other(String),
}

impl Failure {
    pub fn message(&self) -> String {
        match self {
            Failure::Cancelled => "Cancelled.".into(),
            Failure::Network(reason) => format!("{reason} Try again when you're back online."),
            Failure::Unavailable => "This model isn't available to download right now.".into(),
            Failure::NoSpace(0) => "Not enough space on this Mac.".into(),
            Failure::NoSpace(mb) => format!("Not enough space: it needs about {mb} MB free."),
            Failure::Damaged => "The download arrived damaged twice. Try again later.".into(),
            Failure::Other(e) => format!("Couldn't download it: {e}"),
        }
    }
}

/// No data for this long counts as a dropped connection.
const STALL: std::time::Duration = std::time::Duration::from_secs(30);

/// Seconds to wait before each automatic retry.
const RETRY_WAITS: [u64; 5] = [2, 5, 10, 20, 30];

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

/// Partial downloads of models no longer on the list (or of an older archive), and
/// unpacking folders left by a run that quit midway. Current models' partials stay, to
/// resume. Nothing another Heyra still holds is touched: a partial only goes if it can be
/// locked, an unpacking folder only if the process that made it has gone.
fn clear_leftovers(dir: &Path) {
    let keep: Vec<String> = ALL.iter().map(|m| format!("{}.{}.partial", m.dir, &m.sha256[..12])).collect();
    let alive = |name: &str| {
        // `.unpacking-<pid>-<n>` or `<model>.<pid>-<n>.partial`
        let pid = name.trim_start_matches(".unpacking-").rsplit('.').find_map(|part| part.split('-').next()?.parse::<i32>().ok());
        pid.is_some_and(|pid| unsafe { libc::kill(pid, 0) } == 0)
    };
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        if name.starts_with(".unpacking-") {
            if !alive(&name) {
                let _ = fs::remove_dir_all(&path);
            }
        } else if (name.ends_with(".partial") && !keep.contains(&name)) || name.ends_with(".tar.bz2.part") {
            let free = fs::OpenOptions::new().write(true).open(&path).is_ok_and(|f| f.try_lock().is_ok());
            if free {
                let _ = fs::remove_file(&path);
            }
        }
    }
}

/// Numbers downloads and unpacking folders within this run.
static DOWNLOADS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn open_locked(path: &Path) -> Option<fs::File> {
    let file = fs::OpenOptions::new().create(true).truncate(false).read(true).write(true).open(path).ok()?;
    file.try_lock().ok()?;
    Some(file)
}

fn free_mb(dir: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(dir.as_os_str().as_bytes()).ok()?;
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    (unsafe { libc::statvfs(path.as_ptr(), &mut stat) } == 0).then(|| stat.f_bavail as u64 * stat.f_frsize as u64 / 1_000_000)
}

fn file_matches(file: &fs::File, sha256: &str) -> bool {
    file.metadata().is_ok_and(|m| m.len() > 0) && hash_of(file).is_some_and(|d| d == sha256)
}

fn hash_of(file: &fs::File) -> Option<String> {
    let mut reader = file.try_clone().ok()?;
    reader.seek(SeekFrom::Start(0)).ok()?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = reader.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Some(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

fn agent() -> ureq::Agent {
    use std::time::Duration;
    ureq::Agent::config_builder()
        .timeout_resolve(Some(Duration::from_secs(15)))
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        // A whole attempt; a slow line resumes in the next one.
        .timeout_recv_body(Some(Duration::from_secs(20 * 60)))
        .build()
        .into()
}

fn network(e: &ureq::Error) -> Failure {
    match e {
        ureq::Error::StatusCode(404 | 403 | 410) => Failure::Unavailable,
        ureq::Error::StatusCode(_) => Failure::Network("The download server isn't answering.".into()),
        ureq::Error::Io(io) if io.kind() == std::io::ErrorKind::StorageFull => Failure::NoSpace(0),
        _ => Failure::Network("No internet connection.".into()),
    }
}

/// Download `url` into `file`, resuming from what's already there. The caller checks it.
fn fetch(url: &str, file: &fs::File, cancel: &AtomicBool, on: &impl Fn(Progress)) -> Result<(), Failure> {
    let have = file.metadata().map(|m| m.len()).unwrap_or(0);
    let mut request = agent().get(url);
    if have > 0 {
        request = request.header("Range", format!("bytes={have}-"));
    }
    let response = match request.call() {
        // Asked for bytes past the end: what's here may already be all of it.
        Err(ureq::Error::StatusCode(416)) => return Ok(()),
        other => other.map_err(|e| network(&e))?,
    };
    let resumed = response.status() == 206;
    let mut out = file.try_clone().map_err(|e| Failure::Other(e.to_string()))?;
    let mut done = if resumed { have } else { 0 };
    if resumed {
        out.seek(SeekFrom::End(0)).map_err(|e| Failure::Other(e.to_string()))?;
    } else {
        out.set_len(0).and_then(|()| out.seek(SeekFrom::Start(0)).map(|_| ())).map_err(|e| Failure::Other(e.to_string()))?;
    }
    let total = done + response.body().content_length().unwrap_or(0);
    // Reads happen on a helper thread, so a cancel is heard at once and a connection that
    // goes quiet for STALL is given up on, instead of waiting on a read that may never end.
    let mut reader = response.into_body().into_reader();
    let (tx, rx) = std::sync::mpsc::sync_channel::<std::io::Result<Vec<u8>>>(8);
    std::thread::spawn(move || {
        let mut buf = vec![0u8; 1 << 16];
        loop {
            let read = reader.read(&mut buf).map(|n| buf[..n].to_vec());
            let end = !matches!(&read, Ok(chunk) if !chunk.is_empty());
            if tx.send(read).is_err() || end {
                break;
            }
        }
    });
    let mut heard = std::time::Instant::now();
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(Failure::Cancelled);
        }
        let chunk = match rx.recv_timeout(std::time::Duration::from_millis(100)) {
            Ok(Ok(chunk)) => chunk,
            Ok(Err(e)) if e.kind() == std::io::ErrorKind::StorageFull => return Err(Failure::NoSpace(0)),
            Ok(Err(_)) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                return Err(Failure::Network("The connection dropped.".into()));
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) if heard.elapsed() > STALL => {
                return Err(Failure::Network("The connection stalled.".into()));
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
        };
        heard = std::time::Instant::now();
        let n = chunk.len();
        if n == 0 {
            break;
        }
        let buf = chunk;
        out.write_all(&buf[..n]).map_err(|e| {
            if e.kind() == std::io::ErrorKind::StorageFull { Failure::NoSpace(0) } else { Failure::Other(e.to_string()) }
        })?;
        done += n as u64;
        on(Progress::Bytes(done, total));
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

    /// Against a local server that misbehaves on purpose (HEYRA_TEST_FLAKY, the base URL
    /// of flaky_server.py serving the Bark archive): a download that drops twice resumes
    /// and finishes, a missing file and a damaged one fail plainly, and a cancelled one stops.
    #[test]
    fn downloads_survive_a_flaky_server() {
        let Ok(base) = std::env::var("HEYRA_TEST_FLAKY") else { return };
        let dir = std::env::temp_dir().join("heyra-model-test");
        let _ = std::fs::remove_dir_all(&dir);
        let quick = [0u64, 0, 0, 0, 0];
        let model = &super::BARK_CLEAN;
        let url = |p: &str| format!("{base}/{p}");
        let retries = std::sync::Mutex::new(Vec::new());
        let on = |p: super::Progress| {
            if let super::Progress::Retrying { reason, .. } = p {
                retries.lock().unwrap().push(reason);
            }
        };

        // Drops twice, resumes, and lands whole: every byte fetched once, near enough.
        let got = model.download_from(&[&url("drop")], &dir, &AtomicBool::new(false), on, &quick).expect("resumed download");
        assert!(super::files(&got).is_ok());
        crate::engine::Transducer::load(&got).expect("load");
        assert_eq!(retries.lock().unwrap().len(), 2, "two drops, two retries");
        let log = ureq::get(&url("log")).call().unwrap().into_body().read_to_string().unwrap();
        let sent: u64 = log.lines().filter_map(|l| l.rsplit(' ').next()?.parse::<u64>().ok()).sum();
        let size = std::fs::metadata(std::env::var("HEYRA_TEST_ARCHIVE").unwrap()).unwrap().len();
        assert!(sent < size + size / 10, "resumed, not restarted: sent {sent} for {size}");
        let left: Vec<String> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(left, [model.dir], "nothing left behind: {left:?}");
        std::fs::remove_dir_all(&got).unwrap();

        // Not there: a plain failure, after trying the next source.
        let err = model.download_from(&[&url("missing")], &dir, &AtomicBool::new(false), |_| {}, &quick).unwrap_err();
        assert_eq!(err, super::Failure::Unavailable);
        let got = model.download_from(&[&url("missing"), &url("ok")], &dir, &AtomicBool::new(false), |_| {}, &quick).expect("second source");
        std::fs::remove_dir_all(&got).unwrap();

        // Damaged every time: tried once more from scratch, then a plain failure, no partial kept.
        let err = model.download_from(&[&url("corrupt")], &dir, &AtomicBool::new(false), |_| {}, &quick).unwrap_err();
        assert_eq!(err, super::Failure::Damaged);
        assert!(std::fs::read_dir(&dir).unwrap().next().is_none(), "a damaged partial is removed");

        // Offline: the retries run out, and a partial is kept to resume later.
        let err = model.download_from(&["http://127.0.0.1:9/x"], &dir, &AtomicBool::new(false), |_| {}, &[0, 0]).unwrap_err();
        assert!(matches!(err, super::Failure::Network(_)), "{err:?}");
        println!("offline says: {}", err.message());

        // Cancelled partway: stops, and says so.
        let cancel = AtomicBool::new(false);
        let err = model
            .download_from(&[&url("ok")], &dir, &cancel, |p| {
                if let super::Progress::Bytes(done, _) = p {
                    if done > 10_000_000 {
                        cancel.store(true, Ordering::Relaxed);
                    }
                }
            }, &quick)
            .unwrap_err();
        assert_eq!(err, super::Failure::Cancelled);

        // A server that goes quiet: a cancel is heard at once, not after a read times out.
        let cancel = AtomicBool::new(false);
        let started = std::time::Instant::now();
        let err = std::thread::scope(|scope| {
            scope.spawn(|| {
                std::thread::sleep(std::time::Duration::from_secs(2));
                cancel.store(true, Ordering::Relaxed);
            });
            model.download_from(&[&url("stall")], &dir, &cancel, |_| {}, &quick).unwrap_err()
        });
        assert_eq!(err, super::Failure::Cancelled);
        assert!(started.elapsed() < std::time::Duration::from_secs(4), "took {:?}", started.elapsed());

        // Cleanup leaves a partial alone while something holds it.
        let held = dir.join("old-model.abcdef123456.partial");
        let file = std::fs::File::create(&held).unwrap();
        file.lock().unwrap();
        super::clear_leftovers(&dir);
        assert!(held.exists(), "a held partial was removed");
        drop(file);
        super::clear_leftovers(&dir);
        assert!(!held.exists(), "a free stale partial was kept");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reads_the_macs_language() {
        let language = super::first_language().expect("a preferred language");
        assert!(language.len() >= 2, "{language}");
        println!("first language: {language}, default: {}", super::default().name);
    }
}
