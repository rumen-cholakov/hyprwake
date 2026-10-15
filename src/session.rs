//! The saved session: what a snapshot contains and how it is stored.

use crate::workspace::WorkspaceRef;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// The snapshot format this binary writes and understands.
///
/// Bumped once per release that changes the meaning of a field, never once
/// per field. Everything 0.2.0 needs is reserved in v2 up front, including
/// fields nothing reads yet, so that two binaries never disagree about a
/// snapshot with no arbitrator between them.
pub const SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    /// Absent means 1: every snapshot written before 0.2.0.
    #[serde(default = "schema_v1")]
    pub schema_version: u32,
    /// The hyprwake that wrote this, for support bundles and bug reports.
    #[serde(default)]
    pub created_by: String,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub hyprland_version: String,
    #[serde(default)]
    pub monitors: Vec<Monitor>,
    /// One entry per workspace present in the snapshot.
    ///
    /// Carries the inferred split tree once layout inference lands; until
    /// then it records which monitor each workspace was on, which is the
    /// half of the answer that does not need inference.
    #[serde(default)]
    pub workspaces: Vec<Workspace>,
    pub clients: Vec<SessionClient>,
    #[serde(default)]
    pub browser_profiles: Vec<BrowserProfile>,
    /// The version this snapshot carried *on disk*, before migration.
    ///
    /// Not serialised: it is a fact about the file just read, not about the
    /// session, and `schema_version` above always reads as current once
    /// migration has run. `None` for a session built in memory rather than
    /// loaded. Support bundles and `doctor` want the on-disk answer.
    #[serde(skip)]
    pub loaded_from: Option<u32>,
}

fn schema_v1() -> u32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Monitor {
    /// The connector the display was plugged into. Not an identity: see
    /// `make`/`model`/`serial`.
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
    /// Stable panel identity, for recognising the same display on a
    /// different port. Captured from 0.2.0; empty on older snapshots, and
    /// `serial` is empty on many built-in panels even when captured.
    #[serde(default)]
    pub make: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub serial: String,
    #[serde(default = "default_scale")]
    pub scale: f32,
    /// Position in the layout, in logical pixels.
    #[serde(default)]
    pub position: [i32; 2],
}

fn default_scale() -> f32 {
    1.0
}

impl Monitor {
    /// Whether this monitor carries enough identity to be matched by
    /// something better than its connector name.
    pub fn has_stable_identity(&self) -> bool {
        !self.serial.is_empty() || !self.make.is_empty() || !self.model.is_empty()
    }
}

/// A workspace as it stood in the snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workspace {
    pub workspace: WorkspaceRef,
    /// The monitor this workspace lived on, by connector name.
    #[serde(default)]
    pub monitor: String,
    /// The split tree its tiled windows were in, inferred from their
    /// geometry at save (`layout.rs`). `None` when no tree could be read,
    /// or the snapshot predates inference; restore then launches in reading
    /// order.
    #[serde(default)]
    pub layout: Option<LayoutNode>,
    /// `"dwindle"`, or `"master-<side>"` with the side the masters were on
    /// (`left`, `right`, `top`, `bottom`). Absent for a layout inference
    /// does not understand.
    #[serde(default)]
    pub layout_kind: Option<String>,
    /// 0.0–1.0: how cleanly the windows tiled the workspace. 1.0 is a clean
    /// tiling; 0.0 means no tree was recorded.
    #[serde(default)]
    pub layout_confidence: Option<f32>,
}

/// A binary split tree over the tiled windows of one workspace.
///
/// The inference that produces one lives in `layout.rs`; the type lives here
/// because it is part of the on-disk schema and has to version with it.
/// `Leaf` is an index into the session's `clients`. A `Split` divides its box
/// along `axis` — `Horizontal` puts `a` left of `b`, `Vertical` puts `a`
/// above `b` — and `ratio` is the share of the box `a` takes, 0.0–1.0.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "node", rename_all = "kebab-case")]
pub enum LayoutNode {
    Leaf {
        client: usize,
    },
    Split {
        axis: Axis,
        ratio: f32,
        a: Box<LayoutNode>,
        b: Box<LayoutNode>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Axis {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionClient {
    pub class: String,
    /// The class the window announced when it first mapped.
    ///
    /// Some applications rename themselves afterwards, and XWayland windows
    /// can differ; this is what makes them findable again.
    #[serde(default)]
    pub initial_class: String,
    /// Set by the running program, so it changes and can never decide
    /// whether a window matches. It only orders candidates that are equal on
    /// everything else: see `matching`.
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
    /// Reassembled on restore with `[restore] groups = "regroup"`; see
    /// `plan::group_rebuild`.
    #[serde(default)]
    pub group: Option<u32>,
    /// Its place in that group, 0 first, as the group bar orders them.
    #[serde(default)]
    pub group_position: Option<u32>,
    /// What else is known about the window, for diagnosis.
    ///
    /// Every field is optional by design: a `None` is better than a wrong
    /// `Some`. None of them takes part in matching today — within a class
    /// they turned out to be identical on real windows.
    #[serde(default)]
    pub identity: Identity,
    pub launch: LaunchInfo,
}

/// What is known about a window beyond its class, used to tell two windows
/// of the same application apart.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Identity {
    /// Resolved from `/proc/<pid>/exe`.
    #[serde(default)]
    pub executable: Option<std::path::PathBuf>,
    /// Only set when a `.desktop` file was found whose `Exec` resolves to
    /// `executable`. Guessing it from the class is the heuristic this exists
    /// to escape.
    #[serde(default)]
    pub desktop_file: Option<String>,
    #[serde(default)]
    pub app_id: Option<String>,
    #[serde(default)]
    pub initial_title: Option<String>,
    /// Browser profile directory, where the provider knows one.
    #[serde(default)]
    pub profile: Option<String>,
    /// Config key of the provider that captured this window.
    #[serde(default)]
    pub provider: Option<String>,
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
    /// Which provider captured this, by config key.
    #[serde(default)]
    pub provider: Option<String>,
    /// Provider-owned opaque state, handed back to the same provider on
    /// restore. Never interpreted here.
    #[serde(default)]
    pub provider_state: Option<serde_json::Value>,
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
    #[error(
        "session was written by a newer hyprwake (snapshot schema v{found}, \
         this build understands up to v{supported}) — upgrade hyprwake to read it"
    )]
    TooNew { found: u32, supported: u32 },
    #[error("no migration from snapshot schema v{0}")]
    NoMigration(u32),
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
    parse_session(&std::fs::read_to_string(path)?)
}

/// Read the schema version *before* deserialising into `Session`.
///
/// The version has to be established against loose JSON, because the whole
/// point is to say something honest about a snapshot whose shape we do not
/// know. Going straight to `Session` turns "written by a newer hyprwake"
/// into a serde error about an unexpected field, which tells the user
/// nothing they can act on.
pub fn parse_session(content: &str) -> Result<Session, SessionError> {
    let value: serde_json::Value = serde_json::from_str(content)?;

    // Unreadable or absent means v1: every snapshot written before 0.2.0
    // carries no version at all.
    let found = value
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        .and_then(|v| u32::try_from(v).ok())
        .unwrap_or(1);

    if found > SCHEMA_VERSION {
        return Err(SessionError::TooNew {
            found,
            supported: SCHEMA_VERSION,
        });
    }

    let value = migrate(value, found)?;
    let mut session: Session = serde_json::from_value(value)?;
    session.loaded_from = Some(found);
    Ok(session)
}

/// Step a snapshot forward one version at a time.
///
/// A function per step and a loop. A trait with `from_version`/`to_version`
/// would be ceremony for a one-element set; the loop is here so that both it
/// and its tests exist before the second migration needs them.
fn migrate(mut value: serde_json::Value, from: u32) -> Result<serde_json::Value, SessionError> {
    let mut v = from;
    while v < SCHEMA_VERSION {
        value = match v {
            1 => v1_to_v2(value)?,
            _ => return Err(SessionError::NoMigration(v)),
        };
        v += 1;
    }
    Ok(value)
}

/// v1 → v2.
///
/// Nearly empty by construction: every field 0.2.0 adds is
/// `#[serde(default)]`, so a v1 document already deserialises into the v2
/// struct. Stamping the version is what makes the result round-trip as v2
/// rather than being re-migrated on every load.
fn v1_to_v2(mut value: serde_json::Value) -> Result<serde_json::Value, SessionError> {
    if let Some(obj) = value.as_object_mut() {
        obj.insert("schema_version".to_string(), serde_json::json!(2));
    }
    Ok(value)
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
            if let Ok(session) = parse_session(&content) {
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
            initial_class: class.to_string(),
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
            group_position: None,
            identity: Identity::default(),
            launch: LaunchInfo {
                argv: vec![class.to_string()],
                spawn: true,
                provider: None,
                provider_state: None,
            },
        }
    }

    fn session(name: &str, classes: &[&str]) -> Session {
        Session {
            name: name.to_string(),
            schema_version: SCHEMA_VERSION,
            created_by: env!("CARGO_PKG_VERSION").to_string(),
            loaded_from: None,
            workspaces: vec![],
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

    // -- Schema versioning ---------------------------------------------

    /// The regression that has to outlive every later phase: a snapshot
    /// written by 0.1.3, before the schema carried a version at all, still
    /// loads and still says the same thing about the windows in it.
    #[test]
    fn a_v1_snapshot_still_loads() {
        let raw = include_str!("../tests/fixtures/session-v1.json");
        assert!(
            !raw.contains("schema_version"),
            "the fixture stops being a v1 fixture the moment it carries a version"
        );

        let session = parse_session(raw).expect("a 0.1.3 snapshot must still load");

        assert_eq!(
            session.schema_version, SCHEMA_VERSION,
            "migrated to current"
        );
        assert_eq!(session.clients.len(), 5);
        assert_eq!(session.clients[0].class, "foot");
        assert_eq!(
            session.clients[0].launch.argv,
            vec!["foot", "-D", "/home/user/projects/api"],
            "argv is what restore relaunches; it must survive migration verbatim"
        );
        assert!(session.clients[0].launch.spawn);
        assert_eq!(session.monitors.len(), 1);
        assert_eq!(session.monitors[0].name, "eDP-1");
    }

    /// Fields v1 never had take their defaults rather than failing the load.
    #[test]
    fn a_v1_snapshot_defaults_the_fields_it_predates() {
        let session = parse_session(include_str!("../tests/fixtures/session-v1.json")).unwrap();

        assert_eq!(session.created_by, "", "v1 did not record who wrote it");
        assert!(session.workspaces.is_empty(), "v1 had no workspace records");
        assert_eq!(session.clients[0].identity, Identity::default());
        assert_eq!(session.clients[0].launch.provider, None);
        assert_eq!(session.clients[0].launch.provider_state, None);
        assert_eq!(session.monitors[0].serial, "");
        assert_eq!(
            session.monitors[0].scale, 1.0,
            "not zero, which is what a bare serde default would give"
        );
        assert_eq!(session.monitors[0].position, [0, 0]);
        assert!(!session.monitors[0].has_stable_identity());
    }

    #[test]
    fn a_newer_snapshot_names_its_version_instead_of_failing_to_parse() {
        let dir = TempDir::new().unwrap();
        let mut s = session("future", &["foot"]);
        s.schema_version = 99;
        save_session(&s, dir.path()).unwrap();

        match load_session("future", dir.path()) {
            Err(SessionError::TooNew { found, supported }) => {
                assert_eq!(found, 99);
                assert_eq!(supported, SCHEMA_VERSION);
                let message = SessionError::TooNew { found, supported }.to_string();
                assert!(
                    message.contains("99"),
                    "the message names the version: {message}"
                );
                assert!(
                    message.contains("upgrade"),
                    "and says what to do: {message}"
                );
            }
            other => panic!("expected TooNew, got {other:?}"),
        }
    }

    /// A snapshot from the future is skipped by `list`, not silently
    /// mis-summarised by whatever fields happen to still parse.
    #[test]
    fn listing_skips_snapshots_it_cannot_understand() {
        let dir = TempDir::new().unwrap();
        save_session(&session("now", &["foot"]), dir.path()).unwrap();
        let mut future = session("future", &["foot"]);
        future.schema_version = 99;
        save_session(&future, dir.path()).unwrap();

        let names: Vec<_> = list_sessions(dir.path())
            .unwrap()
            .into_iter()
            .map(|s| s.name)
            .collect();
        assert_eq!(names, vec!["now"]);
    }

    #[test]
    fn a_current_snapshot_round_trips_without_migration() {
        let dir = TempDir::new().unwrap();
        let mut s = session("latest", &["foot"]);
        s.created_by = "0.2.0".to_string();
        s.workspaces = vec![Workspace {
            workspace: WorkspaceRef::new(1, "1"),
            monitor: "eDP-1".to_string(),
            layout: None,
            layout_kind: None,
            layout_confidence: None,
        }];
        save_session(&s, dir.path()).unwrap();

        let loaded = load_session("latest", dir.path()).unwrap();
        assert_eq!(loaded.schema_version, SCHEMA_VERSION);
        assert_eq!(loaded.created_by, "0.2.0");
        assert_eq!(loaded.workspaces.len(), 1);
        assert_eq!(loaded.workspaces[0].monitor, "eDP-1");
    }

    #[test]
    fn migration_stamps_the_version_so_it_does_not_run_twice() {
        let migrated = migrate(serde_json::json!({"name": "x"}), 1).unwrap();
        assert_eq!(migrated["schema_version"], serde_json::json!(2));
    }

    #[test]
    fn a_version_with_no_migration_step_is_named() {
        // Reachable once SCHEMA_VERSION moves past a version whose step was
        // never written; asserted now so the loop's failure mode is defined
        // before it can happen quietly.
        assert!(matches!(
            migrate(serde_json::json!({}), 0),
            Err(SessionError::NoMigration(0))
        ));
    }

    #[test]
    fn a_layout_tree_survives_the_round_trip() {
        let tree = LayoutNode::Split {
            axis: Axis::Horizontal,
            ratio: 0.6,
            a: Box::new(LayoutNode::Leaf { client: 0 }),
            b: Box::new(LayoutNode::Split {
                axis: Axis::Vertical,
                ratio: 0.5,
                a: Box::new(LayoutNode::Leaf { client: 1 }),
                b: Box::new(LayoutNode::Leaf { client: 2 }),
            }),
        };
        let json = serde_json::to_string(&tree).unwrap();
        assert_eq!(serde_json::from_str::<LayoutNode>(&json).unwrap(), tree);
    }

    #[test]
    fn a_monitor_with_only_a_connector_name_has_no_stable_identity() {
        let mut m = Monitor {
            name: "DP-1".to_string(),
            width: 2560,
            height: 1440,
            transform: 0,
            active_workspace: None,
            focused: false,
            make: String::new(),
            model: String::new(),
            serial: String::new(),
            scale: 1.0,
            position: [0, 0],
        };
        assert!(!m.has_stable_identity());
        // A built-in panel reporting no serial is still identifiable.
        m.make = "Sharp Corporation".to_string();
        assert!(m.has_stable_identity());
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
