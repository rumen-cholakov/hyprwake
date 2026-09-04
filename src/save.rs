//! The save operation, with the guard that makes unattended saving safe.

use crate::capture::{capture_session, CaptureError};
use crate::config::Config;
use crate::hyprctl::HyprctlClient;
use crate::logging::log;
use crate::process::ProcessInfoProvider;
use crate::session::{load_session, save_session, Session, SessionError};
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum SaveError {
    #[error("capture failed: {0}")]
    Capture(#[from] CaptureError),
    #[error("storage failed: {0}")]
    Storage(#[from] SessionError),
}

#[derive(Debug, PartialEq, Eq)]
pub enum SaveOutcome {
    /// Session written, with the number of windows it holds.
    Saved(usize),
    /// Nothing was open and a populated session already existed, so the old
    /// one was kept.
    RefusedEmpty { kept: usize },
}

/// Capture the desktop and write it to `name`.
///
/// A save that finds no windows never overwrites a populated session. Logout
/// and reboot flows close every window before the session ends, and a
/// periodic save landing in that gap would otherwise replace the last good
/// snapshot with an empty one — exactly when it is needed most.
pub fn perform_save(
    name: &str,
    sessions_dir: &Path,
    config: &Config,
    hyprctl: &dyn HyprctlClient,
    process: &dyn ProcessInfoProvider,
) -> Result<SaveOutcome, SaveError> {
    let session = capture_session(name, hyprctl, process, config)?;
    write_guarded(session, sessions_dir)
}

/// Storage half of [`perform_save`], separated so the guard can be tested
/// without a compositor.
pub fn write_guarded(session: Session, sessions_dir: &Path) -> Result<SaveOutcome, SaveError> {
    if session.clients.is_empty() {
        if let Ok(previous) = load_session(&session.name, sessions_dir) {
            if !previous.clients.is_empty() {
                let kept = previous.clients.len();
                log(format!(
                    "save skipped: kept previous session ({kept} windows), refusing empty save"
                ));
                return Ok(SaveOutcome::RefusedEmpty { kept });
            }
        }
    }
    let count = session.clients.len();
    save_session(&session, sessions_dir)?;
    log(format!("saved {count} windows as '{}'", session.name));
    Ok(SaveOutcome::Saved(count))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{LaunchInfo, SessionClient};
    use crate::workspace::WorkspaceRef;
    use chrono::Utc;
    use tempfile::TempDir;

    fn session(name: &str, windows: usize) -> Session {
        Session {
            name: name.to_string(),
            created_at: Utc::now(),
            hyprland_version: "0.56.2".to_string(),
            monitors: vec![],
            clients: (0..windows)
                .map(|i| SessionClient {
                    class: format!("app{i}"),
                    title: String::new(),
                    workspace: WorkspaceRef::new(1, "1"),
                    monitor: String::new(),
                    at: [0, 0],
                    size: [100, 100],
                    floating: false,
                    pinned: false,
                    fullscreen: 0,
                    focus_history_id: 0,
                    group: None,
                    launch: LaunchInfo {
                        argv: vec![format!("app{i}")],
                        spawn: true,
                    },
                })
                .collect(),
            browser_profiles: vec![],
        }
    }

    #[test]
    fn a_populated_save_is_written() {
        let dir = TempDir::new().unwrap();
        assert_eq!(
            write_guarded(session("latest", 3), dir.path()).unwrap(),
            SaveOutcome::Saved(3)
        );
        assert_eq!(load_session("latest", dir.path()).unwrap().clients.len(), 3);
    }

    #[test]
    fn an_empty_save_never_replaces_a_populated_session() {
        let dir = TempDir::new().unwrap();
        write_guarded(session("latest", 4), dir.path()).unwrap();

        let outcome = write_guarded(session("latest", 0), dir.path()).unwrap();
        assert_eq!(outcome, SaveOutcome::RefusedEmpty { kept: 4 });
        assert_eq!(
            load_session("latest", dir.path()).unwrap().clients.len(),
            4,
            "the last good session must survive a shutdown-time empty save"
        );
    }

    #[test]
    fn an_empty_save_is_allowed_when_nothing_was_saved_before() {
        let dir = TempDir::new().unwrap();
        assert_eq!(
            write_guarded(session("latest", 0), dir.path()).unwrap(),
            SaveOutcome::Saved(0)
        );
    }

    #[test]
    fn an_empty_save_may_replace_an_empty_session() {
        let dir = TempDir::new().unwrap();
        write_guarded(session("latest", 0), dir.path()).unwrap();
        assert_eq!(
            write_guarded(session("latest", 0), dir.path()).unwrap(),
            SaveOutcome::Saved(0)
        );
    }

    #[test]
    fn the_guard_is_per_session_name() {
        let dir = TempDir::new().unwrap();
        write_guarded(session("work", 5), dir.path()).unwrap();
        // An empty "latest" must not be blocked by a populated "work".
        assert_eq!(
            write_guarded(session("latest", 0), dir.path()).unwrap(),
            SaveOutcome::Saved(0)
        );
    }
}
