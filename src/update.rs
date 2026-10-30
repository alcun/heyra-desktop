//! Staying current without the terminal. A minute after starting, then once a day,
//! Heyra asks GitHub for the latest release. If it's newer and Heyra came from
//! Homebrew, it runs `brew upgrade` itself; Homebrew swaps the app, and Heyra notices
//! and restarts into it at rest (see `replaced_bundle` in main.rs). Installed some
//! other way, it says once that the new version is out.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::store;
use crate::worker::Shared;

const RELEASES: &str = "https://api.github.com/repos/alcun/heyra-desktop/releases/latest";
const CASK: &str = "alcun/tap/heyra";

pub fn start(state: Shared) {
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(60));
        loop {
            check(&state);
            std::thread::sleep(Duration::from_secs(24 * 60 * 60));
        }
    });
}

fn check(state: &Shared) {
    let Some(latest) = latest() else { return };
    if !newer(&latest, env!("CARGO_PKG_VERSION")) {
        return;
    }
    match brew() {
        Some(brew) => {
            store::log(&format!("update: {latest} is out, upgrading with Homebrew"));
            let out = std::process::Command::new(&brew)
                .args(["upgrade", "--cask", CASK])
                .env("HOMEBREW_NO_ENV_HINTS", "1")
                .output();
            match out {
                Ok(out) if out.status.success() => store::log("update: Homebrew upgraded Heyra"),
                Ok(out) => store::log(&format!("update: brew failed: {}", String::from_utf8_lossy(&out.stderr).trim())),
                Err(e) => store::log(&format!("update: couldn't run brew: {e}")),
            }
        }
        // Not from Homebrew: say so once per version.
        None => {
            let settings = store::load_settings();
            if settings.update_offered.as_deref() != Some(latest.as_str()) {
                state.lock().unwrap().toast = Some(("Update".into(), format!("Heyra {latest} is out"), Instant::now()));
                store::save_settings(&store::Settings { update_offered: Some(latest), ..settings });
            }
        }
    }
}

/// The latest release's version, "0.5.2".
fn latest() -> Option<String> {
    let body = ureq::get(RELEASES)
        .header("User-Agent", concat!("Heyra/", env!("CARGO_PKG_VERSION")))
        .header("Accept", "application/vnd.github+json")
        .call()
        .ok()?
        .into_body()
        .read_to_string()
        .ok()?;
    let json: serde_json::Value = serde_json::from_str(&body).ok()?;
    Some(json["tag_name"].as_str()?.trim_start_matches('v').to_string())
}

/// Is `a` a later version than `b`? Numbers compared part by part.
fn newer(a: &str, b: &str) -> bool {
    let parts = |v: &str| v.split('.').map(|p| p.parse::<u32>().unwrap_or(0)).collect::<Vec<_>>();
    parts(a) > parts(b)
}

/// Homebrew, if this copy of Heyra was installed with it.
fn brew() -> Option<PathBuf> {
    ["/opt/homebrew", "/usr/local"]
        .iter()
        .map(PathBuf::from)
        .find(|prefix| prefix.join("Caskroom/heyra").is_dir() && prefix.join("bin/brew").is_file())
        .map(|prefix| prefix.join("bin/brew"))
}

#[cfg(test)]
mod tests {
    use super::newer;

    #[test]
    fn compares_versions_by_number() {
        assert!(newer("0.5.2", "0.5.1"));
        assert!(newer("0.10.0", "0.9.9"));
        assert!(!newer("0.5.1", "0.5.1"));
        assert!(!newer("0.5.0", "0.5.1"));
    }
}
