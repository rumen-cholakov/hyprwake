//! The saved session: what a snapshot contains and how it is stored.

use crate::workspace::WorkspaceRef;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub hyprland_version: String,
    #[serde(default)]
    pub monitors: Vec<Monitor>,
    pub clients: Vec<SessionClient>,
    #[serde(default)]
    pub browser_profiles: Vec<BrowserProfile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Monitor {
    pub name: String,
    pub width: u32,
    pub height: u32,
    #[serde(default)]
    pub transform: u32,
    /// The workspace this monitor was showing.
    #[serde(default)]
    pub active_workspace: Option<WorkspaceRef>,
    /// Whether this monitor held the focus.
    #[serde(default)]
    pub focused: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionClient {
    pub class: String,
    /// Kept for the human reading `hyprwake list -v`; titles are set by the
    /// running program and are never used for matching.
    #[serde(default)]
    pub title: String,
    pub workspace: WorkspaceRef,
    #[serde(default)]
    pub monitor: String,
    pub at: [i32; 2],
    pub size: [i32; 2],
    #[serde(default)]
    pub floating: bool,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub fullscreen: u8,
    #[serde(default)]
    pub focus_history_id: i32,
    /// Index of the window group this belonged to, shared by its members.
    ///
    /// Recorded but not reassembled on restore: see `restore::grouped_sets`.
    #[serde(default)]
    pub group: Option<u32>,
    pub launch: LaunchInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LaunchInfo {
    /// argv recovered from /proc, ready to be shell-quoted.
    pub argv: Vec<String>,
    /// Whether restore should launch this window.
    ///
    /// Single-instance programs own several windows from one process; only
    /// the first carries `spawn`, and the rest are placed by the sweep once
    /// the program reopens them itself.
    #[serde(default = "default_true")]
    pub spawn: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserProfile {
    pub class: String,
    pub directory: String,
    pub name: String,
    pub workspace: String,
}

impl Session {
    /// The window that had focus, as an index into `clients`.
    ///
    /// Hyprland numbers focus history from the focused window outward, so
    /// the lowest id was the one in use.
    pub fn focused_client(&self) -> Option<usize> {
        self.clients
            .iter()
            .enumerate()
            .filter(|(_, c)| c.focus_history_id >= 0)
            .min_by_key(|(_, c)| c.focus_history_id)
            .map(|(i, _)| i)
    }

    pub fn spawn_count(&self) -> usize {
        self.clients.iter().filter(|c| c.launch.spawn).count()
    }
}

// ── Storage ────────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("session '{0}' not found")]
    NotFound(String),
}

#[derive(Debug, Clone)]
pub struct SessionSummary {
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub client_count: usize,
}

pub fn session_path(name: &str, sessions_dir: &Path) -> std::path::PathBuf {
    sessions_dir.join(format!("{name}.json"))
}

/// Write atomically: a session file truncated by a crash mid-write would be
/// worse than a slightly stale one.
pub fn save_session(session: &Session, sessions_dir: &Path) -> Result<(), SessionError> {
    std::fs::create_dir_all(sessions_dir)?;
    let path = session_path(&session.name, sessions_dir);
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(session)?)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

pub fn load_session(name: &str, sessions_dir: &Path) -> Result<Session, SessionError> {
    let path = session_path(name, sessions_dir);
    if !path.exists() {
        return Err(SessionError::NotFound(name.to_string()));
    }
    Ok(serde_json::from_str(&std::fs::read_to_string(path)?)?)
}

pub fn list_sessions(sessions_dir: &Path) -> Result<Vec<SessionSummary>, SessionError> {
    if !sessions_dir.exists() {
        return Ok(vec![]);
    }
    let mut summaries = Vec::new();
    for entry in std::fs::read_dir(sessions_dir)?.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        if let Ok(content) = std::fs::read_to_string(&path) {
            if let Ok(session) = serde_json::from_str::<Session>(&content) {
                summaries.push(SessionSummary {
                    name: session.name.clone(),
                    created_at: session.created_at,
                    client_count: session.clients.len(),
                });
            }
        }
    }
    summaries.sort_by_key(|s| std::cmp::Reverse(s.created_at));
    Ok(summaries)
}

pub fn delete_session(name: &str, sessions_dir: &Path) -> Result<(), SessionError> {
    let path = session_path(name, sessions_dir);
    if !path.exists() {
        return Err(SessionError::NotFound(name.to_string()));
    }
    std::fs::remove_file(path)?;
    Ok(())
}

pub fn session_exists(name: &str, sessions_dir: &Path) -> bool {
    session_path(name, sessions_dir).exists()
}

// ── Autosave rotation ──────────────────────────────────────────────────────

pub const AUTOSAVE_PREFIX: &str = "autosave-";

pub fn autosave_name_now() -> String {
    format!("{AUTOSAVE_PREFIX}{}", Utc::now().format("%Y%m%dT%H%M%S"))
}

/// Autosaves only, newest first. The timestamp format sorts lexicographically.
pub fn list_autosave_sessions(sessions_dir: &Path) -> Result<Vec<SessionSummary>, SessionError> {
    let mut all = list_sessions(sessions_dir)?;
    all.retain(|s| s.name.starts_with(AUTOSAVE_PREFIX));
    all.sort_by(|a, b| b.name.cmp(&a.name));
    Ok(all)
}

pub fn rotate_autosaves(sessions_dir: &Path, retain: usize) -> Result<usize, SessionError> {
    if retain == 0 {
        return Ok(0);
    }
    let autosaves = list_autosave_sessions(sessions_dir)?;
    let mut pruned = 0;
    if autosaves.len() > retain {
        for session in &autosaves[retain..] {
            delete_session(&session.name, sessions_dir)?;
            pruned += 1;
        }
    }
    Ok(pruned)
}

/// Parse `30m`, `24h`, `7d` into a duration.
pub fn parse_max_age(s: &str) -> Result<chrono::Duration, String> {
    let (num, unit) = s.split_at(s.len().saturating_sub(1));
    let n: i64 = num
        .parse()
        .map_err(|_| format!("invalid duration: '{s}'"))?;
    match unit {
        "m" => Ok(chrono::Duration::minutes(n)),
        "h" => Ok(chrono::Duration::hours(n)),
        "d" => Ok(chrono::Duration::days(n)),
        _ => Err(format!("invalid duration unit in '{s}'. Use m, h or d.")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn client(class: &str) -> SessionClient {
        SessionClient {
            class: class.to_string(),
            title: String::new(),
            workspace: WorkspaceRef::new(1, "1"),
            monitor: "eDP-1".to_string(),
            at: [0, 0],
            size: [800, 600],
            floating: false,
            pinned: false,
            fullscreen: 0,
            focus_history_id: 0,
            group: None,
            launch: LaunchInfo {
                argv: vec![class.to_string()],
                spawn: true,
            },
        }
    }

    fn session(name: &str, classes: &[&str]) -> Session {
        Session {
            name: name.to_string(),
            created_at: Utc::now(),
            hyprland_version: "0.56.2".to_string(),
            monitors: vec![],
            clients: classes.iter().map(|c| client(c)).collect(),
            browser_profiles: vec![],
        }
    }

    #[test]
    fn round_trips_through_disk() {
        let dir = TempDir::new().unwrap();
        let s = session("latest", &["foot", "google-chrome"]);
        save_session(&s, dir.path()).unwrap();
        let loaded = load_session("latest", dir.path()).unwrap();
        assert_eq!(loaded.clients.len(), 2);
        assert_eq!(loaded.clients[0].launch.argv, vec!["foot"]);
        assert_eq!(loaded.clients[0].workspace.selector(), "1");
    }

    #[test]
    fn saving_leaves_no_temp_file_behind() {
        let dir = TempDir::new().unwrap();
        save_session(&session("latest", &["foot"]), dir.path()).unwrap();
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .filter(|e| e.path().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty());
    }

    #[test]
    fn missing_session_is_reported() {
        let dir = TempDir::new().unwrap();
        assert!(matches!(
            load_session("nope", dir.path()),
            Err(SessionError::NotFound(_))
        ));
        assert!(!session_exists("nope", dir.path()));
    }

    #[test]
    fn listing_an_absent_directory_is_empty_not_an_error() {
        let dir = TempDir::new().unwrap();
        let missing = dir.path().join("nothing-here");
        assert!(list_sessions(&missing).unwrap().is_empty());
    }

    #[test]
    fn rotation_keeps_the_newest_autosaves_only() {
        let dir = TempDir::new().unwrap();
        for stamp in ["20260101T000000", "20260102T000000", "20260103T000000"] {
            let mut s = session(&format!("autosave-{stamp}"), &["foot"]);
            s.created_at = Utc::now();
            save_session(&s, dir.path()).unwrap();
        }
        save_session(&session("work", &["foot"]), dir.path()).unwrap();

        assert_eq!(rotate_autosaves(dir.path(), 2).unwrap(), 1);
        let names: Vec<_> = list_autosave_sessions(dir.path())
            .unwrap()
            .into_iter()
            .map(|s| s.name)
            .collect();
        assert_eq!(
            names,
            vec!["autosave-20260103T000000", "autosave-20260102T000000"]
        );
        assert!(
            session_exists("work", dir.path()),
            "named sessions are never rotated away"
        );
    }

    #[test]
    fn rotation_with_zero_retain_is_a_no_op() {
        let dir = TempDir::new().unwrap();
        save_session(&session("autosave-20260101T000000", &["foot"]), dir.path()).unwrap();
        assert_eq!(rotate_autosaves(dir.path(), 0).unwrap(), 0);
    }

    #[test]
    fn spawn_count_ignores_sweep_only_windows() {
        let mut s = session("latest", &["chrome", "chrome"]);
        s.clients[1].launch.spawn = false;
        assert_eq!(s.spawn_count(), 1);
    }

    #[test]
    fn durations_parse() {
        assert_eq!(parse_max_age("30m").unwrap(), chrono::Duration::minutes(30));
        assert_eq!(parse_max_age("24h").unwrap(), chrono::Duration::hours(24));
        assert_eq!(parse_max_age("7d").unwrap(), chrono::Duration::days(7));
    }

    #[test]
    fn bad_durations_are_rejected() {
        assert!(parse_max_age("24x").is_err());
        assert!(parse_max_age("h").is_err());
        assert!(parse_max_age("").is_err());
    }
}
