//! Setting up a TING for Heyra, once per TING: plug it in over USB-C, and its
//! disk gets a small script and four chirp tones (from ting-wispr). The TING's
//! firmware isn't touched; deleting main.py from its disk undoes it.

use std::path::{Path, PathBuf};

const FILES: [(&str, &[u8]); 6] = [
    ("main.py", include_bytes!("../assets/ting/main.py")),
    ("1.wav", include_bytes!("../assets/ting/1.wav")),
    ("2.wav", include_bytes!("../assets/ting/2.wav")),
    ("3.wav", include_bytes!("../assets/ting/3.wav")),
    ("4.wav", include_bytes!("../assets/ting/4.wav")),
    ("config.json", include_bytes!("../assets/ting/config.json")),
];

/// The TING's disk, while it's plugged in over USB-C.
pub fn disk() -> Option<PathBuf> {
    ["/Volumes/FX MIC DISK", "/Volumes/TINGDISK"].into_iter().map(PathBuf::from).find(|p| p.is_dir())
}

/// Does this TING already have Heyra's script?
pub fn is_set_up(disk: &Path) -> bool {
    std::fs::read(disk.join("main.py")).is_ok_and(|b| b == FILES[0].1)
}

/// Back up what's on the disk, copy the files and eject it. Returns the backup folder.
pub fn set_up(disk: &Path) -> Result<PathBuf, String> {
    let home = std::env::var("HOME").map_err(|_| "no home folder")?;
    let stamp = chrono::Local::now().format("%Y-%m-%d %H.%M");
    let backup = PathBuf::from(home).join("Documents").join(format!("FX mic backup {stamp}"));
    std::fs::create_dir_all(&backup).map_err(|e| format!("backup folder: {e}"))?;
    for entry in std::fs::read_dir(disk).map_err(|e| format!("read the FX mic: {e}"))?.flatten() {
        let name = entry.file_name();
        if entry.path().is_file() && !name.to_string_lossy().starts_with('.') {
            std::fs::copy(entry.path(), backup.join(&name)).map_err(|e| format!("back up {name:?}: {e}"))?;
        }
    }
    for (name, bytes) in FILES {
        std::fs::write(disk.join(name), bytes).map_err(|e| format!("write {name}: {e}"))?;
    }
    let _ = std::process::Command::new("sync").status();
    let _ = std::process::Command::new("diskutil").arg("eject").arg(disk).status();
    crate::store::log(&format!("TING set up; its old files are in {}", backup.display()));
    Ok(backup)
}

#[cfg(test)]
mod tests {
    #[test]
    fn backs_up_then_copies() {
        let disk = std::env::temp_dir().join(format!("fx-mic-test-{}", std::process::id()));
        std::fs::create_dir_all(&disk).unwrap();
        std::fs::write(disk.join("main.py"), "stock").unwrap();
        assert!(!super::is_set_up(&disk));
        let backup = super::set_up(&disk).unwrap();
        assert!(super::is_set_up(&disk));
        assert_eq!(std::fs::read_to_string(backup.join("main.py")).unwrap(), "stock");
        std::fs::remove_dir_all(&disk).unwrap();
        std::fs::remove_dir_all(&backup).unwrap();
    }
}
