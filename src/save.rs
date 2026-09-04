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
    /// Most of the session had just disappeared, so the old one was kept.
    RefusedDrop { kept: usize, captured: usize },
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
    force: bool,
) -> Result<SaveOutcome, SaveError> {
    let session = capture_session(name, hyprctl, process, config)?;
    write_guarded(session, sessions_dir, config, force)
}

/// Storage half of [`perform_save`], separated so the guard can be tested
/// without a compositor. `force` writes whatever was captured.
pub fn write_guarded(
    session: Session,
    sessions_dir: &Path,
    config: &Config,
    force: bool,
) -> Result<SaveOutcome, SaveError> {
    if !force {
        if let Ok(previous) = load_session(&session.name, sessions_dir) {
            let kept = previous.clients.len();
            let captured = session.clients.len();
            if kept > 0 {
                if captured == 0 {
                    log(format!(
                        "save skipped: kept previous session ({kept} windows), refusing empty save"
                    ));
                    return Ok(SaveOutcome::RefusedEmpty { kept });
                }
                // A desktop that lost most of its windows moments after a
                // full snapshot is being torn down, not rearranged. Saving
                // then would replace a good session with a half-empty one --
                // which is what a restore or a logout looks like in progress.
                let age = chrono::Utc::now() - previous.created_at;
                let collapsed = (captured as f64) < kept as f64 * config.general.save_drop_fraction;
                if collapsed && age.num_seconds() < config.general.save_drop_window_secs {
                    log(format!(
                        "save skipped: {captured} of {kept} windows left {}s after the last \
                         save, refusing to record a collapsing session",
                        age.num_seconds()
                    ));
                    return Ok(SaveOutcome::RefusedDrop { kept, captured });
                }
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

    fn config() -> Config {
        toml::from_str("").unwrap()
    }

    /// write_guarded with the default config and no force.
    fn guarded(s: Session, dir: &std::path::Path) -> SaveOutcome {
        write_guarded(s, dir, &config(), false).unwrap()
    }

    /// A session that claims to have been saved `secs` ago.
    fn aged(name: &str, windows: usize, secs: i64) -> Session {
        let mut s = session(name, windows);
        s.created_at = chrono::Utc::now() - chrono::Duration::seconds(secs);
        s
    }

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
            guarded(session("latest", 3), dir.path()),
            SaveOutcome::Saved(3)
        );
        assert_eq!(load_session("latest", dir.path()).unwrap().clients.len(), 3);
    }

    #[test]
    fn an_empty_save_never_replaces_a_populated_session() {
        let dir = TempDir::new().unwrap();
        guarded(session("latest", 4), dir.path());

        let outcome = guarded(session("latest", 0), dir.path());
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
            guarded(session("latest", 0), dir.path()),
            SaveOutcome::Saved(0)
        );
    }

    #[test]
    fn an_empty_save_may_replace_an_empty_session() {
        let dir = TempDir::new().unwrap();
        guarded(session("latest", 0), dir.path());
        assert_eq!(
            guarded(session("latest", 0), dir.path()),
            SaveOutcome::Saved(0)
        );
    }

    #[test]
    fn a_collapsing_session_is_not_recorded() {
        // What a logout or a restore in progress looks like: most of the
        // windows gone, moments after a full snapshot.
        let dir = TempDir::new().unwrap();
        write_guarded(aged("latest", 6, 5), dir.path(), &config(), false).unwrap();
        assert_eq!(
            guarded(session("latest", 2), dir.path()),
            SaveOutcome::RefusedDrop {
                kept: 6,
                captured: 2
            }
        );
        assert_eq!(load_session("latest", dir.path()).unwrap().clients.len(), 6);
    }

    #[test]
    fn a_gradual_loss_is_a_real_change() {
        // The same drop, but the previous save is old: the user closed
        // things and meant it.
        let dir = TempDir::new().unwrap();
        write_guarded(aged("latest", 6, 600), dir.path(), &config(), false).unwrap();
        assert_eq!(
            guarded(session("latest", 2), dir.path()),
            SaveOutcome::Saved(2)
        );
    }

    #[test]
    fn a_modest_loss_is_recorded() {
        let dir = TempDir::new().unwrap();
        write_guarded(aged("latest", 6, 5), dir.path(), &config(), false).unwrap();
        assert_eq!(
            guarded(session("latest", 4), dir.path()),
            SaveOutcome::Saved(4),
            "closing two of six windows is ordinary use"
        );
    }

    #[test]
    fn force_writes_a_collapsing_session_anyway() {
        let dir = TempDir::new().unwrap();
        write_guarded(aged("latest", 6, 5), dir.path(), &config(), false).unwrap();
        assert_eq!(
            write_guarded(session("latest", 1), dir.path(), &config(), true).unwrap(),
            SaveOutcome::Saved(1)
        );
    }

    #[test]
    fn the_guard_can_be_turned_off() {
        let dir = TempDir::new().unwrap();
        let mut cfg = config();
        cfg.general.save_drop_fraction = 0.0;
        write_guarded(aged("latest", 6, 5), dir.path(), &cfg, false).unwrap();
        assert_eq!(
            write_guarded(session("latest", 1), dir.path(), &cfg, false).unwrap(),
            SaveOutcome::Saved(1)
        );
    }

    #[test]
    fn the_guard_is_per_session_name() {
        let dir = TempDir::new().unwrap();
        guarded(session("work", 5), dir.path());
        // An empty "latest" must not be blocked by a populated "work".
        assert_eq!(
            guarded(session("latest", 0), dir.path()),
            SaveOutcome::Saved(0)
        );
    }
}
