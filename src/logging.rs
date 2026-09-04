//! Append-only log in the state dir.
//!
//! Restore runs unattended from a boot hook, where stderr goes nowhere. A
//! silent failure there is indistinguishable from "nothing was saved", so
//! every interesting step is recorded to a file the user can read afterwards.
//! Ported from the Omarchy fork of hypr-session-restore.

use std::io::Write;
use std::path::PathBuf;

/// Rotate once the log passes this size, keeping a single `.old` generation.
const MAX_LOG_BYTES: u64 = 512 * 1024;

pub fn state_dir() -> PathBuf {
    dirs::state_dir()
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".local/state")
        })
        .join("hyprwake")
}

pub fn log_path() -> PathBuf {
    state_dir().join("hyprwake.log")
}

/// Append one line to the log. Never fails: logging must not break restore.
pub fn log(msg: impl AsRef<str>) {
    let _ = try_log(msg.as_ref());
}

fn try_log(msg: &str) -> std::io::Result<()> {
    let dir = state_dir();
    std::fs::create_dir_all(&dir)?;
    let path = log_path();

    if let Ok(meta) = std::fs::metadata(&path) {
        if meta.len() > MAX_LOG_BYTES {
            let _ = std::fs::rename(&path, path.with_extension("log.old"));
        }
    }

    let stamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    writeln!(file, "{stamp} {msg}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_dir_is_namespaced() {
        assert!(state_dir().ends_with("hyprwake"));
    }

    #[test]
    fn log_path_lives_in_state_dir() {
        assert_eq!(log_path().parent(), Some(state_dir().as_path()));
    }
}
