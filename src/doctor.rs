//! `hyprwake doctor` — what would happen if you saved and restored right now.

use crate::capture::relaunch_argv;
use crate::config::Config;
use crate::hyprctl::HyprctlClient;
use crate::process::ProcessInfoProvider;
use crate::session::{list_sessions, load_session, Session, SessionError, SCHEMA_VERSION};
use serde::Serialize;
use std::path::Path;

#[derive(Debug, PartialEq, Eq, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Ok,
    Warn,
    Fail,
}

impl Status {
    pub fn marker(&self) -> &'static str {
        match self {
            Status::Ok => "ok  ",
            Status::Warn => "warn",
            Status::Fail => "FAIL",
        }
    }
}

#[derive(Debug, Serialize)]
pub struct Check {
    pub name: String,
    pub status: Status,
    pub detail: String,
}

impl Check {
    fn new(name: &str, status: Status, detail: impl Into<String>) -> Self {
        Self {
            name: name.to_string(),
            status,
            detail: detail.into(),
        }
    }
}

/// Hyprland's Lua dispatcher syntax, which every restore call uses, landed in
/// 0.55.
pub fn supports_lua_dispatchers(version: &str) -> bool {
    let digits: Vec<u32> = version
        .trim_start_matches('v')
        .split(['.', '-'])
        .take(2)
        .filter_map(|p| p.parse().ok())
        .collect();
    match digits.as_slice() {
        [major, minor, ..] => *major > 0 || *minor >= 55,
        _ => false,
    }
}

/// Whether `watch` has an event stream to listen to.
///
/// Only a connection proves it. A stale signature, a compositor that has
/// gone away or the wrong `XDG_RUNTIME_DIR` all leave the environment looking
/// fine, and this check used to say "reachable" on the strength of a variable
/// being set.
fn event_socket_check(path: Option<std::path::PathBuf>) -> Check {
    let Some(path) = path else {
        return Check::new(
            "event socket",
            Status::Warn,
            "HYPRLAND_INSTANCE_SIGNATURE or XDG_RUNTIME_DIR unset; use `daemon` instead of `watch`",
        );
    };
    match std::os::unix::net::UnixStream::connect(&path) {
        Ok(_) => Check::new("event socket", Status::Ok, "reachable; `watch` can run"),
        Err(e) => Check::new(
            "event socket",
            Status::Warn,
            format!(
                "{} refused a connection ({e}); `watch` cannot run here, `daemon` polls instead",
                path.display()
            ),
        ),
    }
}

/// Whether saved split trees will be rebuilt, and whether the compositor
/// will keep them.
///
/// With `dwindle:preserve_split` off, Hyprland re-derives every split's
/// orientation from the shape of its box, so a rebuilt tree holds only while
/// the shapes do; capture already scores a tree that relies on anything else
/// as unreachable. Worth a warning only when replay is turned on.
fn layout_check(config: &Config, opts: &crate::layout::LayoutOptions) -> Check {
    use crate::config::LayoutMode;
    let mode = config.restore.layout;
    let kind = opts.kind();
    let detail = match (mode, &kind) {
        (LayoutMode::Off, _) => {
            return Check::new("layout", Status::Ok, "off: no trees recorded or rebuilt")
        }
        (LayoutMode::ReadingOrder, _) => {
            return Check::new(
                "layout",
                Status::Ok,
                "reading order; trees are recorded, set [restore] layout = \"best-effort\" to rebuild them",
            )
        }
        (_, None) => {
            return Check::new(
                "layout",
                Status::Warn,
                format!(
                    "general:layout is {}, which hyprwake cannot infer; workspaces restore in reading order",
                    opts.layout
                ),
            )
        }
        (_, Some(kind)) => format!(
            "{} rebuilding of {kind} trees",
            if mode == LayoutMode::Strict { "strict" } else { "best-effort" }
        ),
    };
    if opts.layout == "dwindle" && !opts.preserve_split {
        return Check::new(
            "layout",
            Status::Warn,
            format!(
                "{detail}; dwindle:preserve_split is off, so Hyprland re-derives split \
                 orientations from window shapes and a rebuilt tree can reflow when resized"
            ),
        );
    }
    Check::new("layout", Status::Ok, detail)
}

/// Rebuilding groups relies on `group:auto_group`: a window opening while a
/// group has focus joins it. Only worth a line when regrouping is on.
fn groups_check(config: &Config, hyprctl: &dyn HyprctlClient) -> Option<Check> {
    if config.restore.groups != crate::config::GroupMode::Regroup {
        return None;
    }
    let auto_group = hyprctl
        .get_option("group.auto_group")
        .and_then(|v| {
            v.get("bool")
                .and_then(|b| b.as_bool())
                .or_else(|| v.get("int").and_then(|i| i.as_i64()).map(|i| i != 0))
        })
        .unwrap_or(true);
    Some(if auto_group {
        Check::new("groups", Status::Ok, "window groups are rebuilt on restore")
    } else {
        Check::new(
            "groups",
            Status::Warn,
            "group:auto_group is off, so no window can be launched into a group; groups \
             will come back as separate windows",
        )
    })
}

/// What the snapshot on disk says it is, and what wrote it.
///
/// An older snapshot is fine — it is migrated on load — so this reports
/// rather than warns. It exists because "which schema was that" is the first
/// question a compatibility report has to answer.
fn schema_check(session: &Session) -> Check {
    let wrote = if session.created_by.is_empty() {
        "written before hyprwake recorded its own version".to_string()
    } else {
        format!("written by hyprwake {}", session.created_by)
    };
    let on_disk = session.loaded_from.unwrap_or(session.schema_version);
    let detail = if on_disk < SCHEMA_VERSION {
        format!("snapshot schema v{on_disk} (migrated to v{SCHEMA_VERSION} on load), {wrote}")
    } else {
        format!("snapshot schema v{on_disk}, {wrote}")
    };
    Check::new("schema", Status::Ok, detail)
}

pub fn run(
    hyprctl: &dyn HyprctlClient,
    process: &dyn ProcessInfoProvider,
    config: &Config,
    sessions_dir: &Path,
) -> Vec<Check> {
    let mut checks = Vec::new();

    // ── compositor ──
    match hyprctl.get_hyprland_version() {
        Ok(version) => {
            if supports_lua_dispatchers(&version) {
                checks.push(Check::new("hyprland", Status::Ok, version.to_string()));
            } else {
                checks.push(Check::new(
                    "hyprland",
                    Status::Fail,
                    format!(
                        "{version} is below 0.55; the Lua dispatchers restore needs are absent"
                    ),
                ));
            }
        }
        Err(e) => checks.push(Check::new("hyprland", Status::Fail, format!("{e}"))),
    }

    checks.push(event_socket_check(crate::autosave::event_socket_path()));

    // ── launching ──
    checks.push(if config.use_uwsm() {
        Check::new(
            "uwsm",
            Status::Ok,
            "restored apps get their own systemd scopes",
        )
    } else {
        Check::new(
            "uwsm",
            Status::Warn,
            "launching directly; on a uwsm session set launch.use_uwsm = true",
        )
    });

    // ── terminals ──
    let providers = crate::provider::ProviderSet::from_config(config);
    let mut missing = Vec::new();
    let mut present = Vec::new();
    for (provider, term) in providers.terminals() {
        let classes = provider.entry.classes.join(", ");
        if which::which(&term.binary).is_ok() {
            present.push(classes);
        } else {
            missing.push(format!("{classes} ({})", term.binary));
        }
    }
    present.sort();
    checks.push(if present.is_empty() {
        Check::new(
            "terminals",
            Status::Warn,
            format!(
                "none of the configured terminals are installed: {}",
                missing.join(", ")
            ),
        )
    } else {
        Check::new("terminals", Status::Ok, present.join(", "))
    });

    // ── provider declarations ──
    if !providers.problems().is_empty() {
        checks.push(Check::new(
            "providers",
            Status::Warn,
            format!(
                "{}; `hyprwake providers` lists them",
                providers.problems().join("; ")
            ),
        ));
    }

    // ── layout replay and groups ──
    checks.push(layout_check(
        config,
        &crate::layout::LayoutOptions::read(hyprctl),
    ));
    if let Some(check) = groups_check(config, hyprctl) {
        checks.push(check);
    }

    // ── external providers: a helper that will not be run ──
    let mut refused = Vec::new();
    for (provider, external) in providers.externals() {
        let helpers = std::iter::once(&external.capture).chain(external.relaunch.as_ref());
        for program in helpers.filter_map(|c| c.first()) {
            if let Err(why) = crate::external::trusted(program) {
                let line = format!("[providers.{}] {why}", provider.key);
                // Capture and relaunch are often one helper.
                if !refused.contains(&line) {
                    refused.push(line);
                }
            }
        }
    }
    if !refused.is_empty() {
        checks.push(Check::new(
            "external",
            Status::Warn,
            format!(
                "{}; those windows are saved and restored from their own argv instead",
                refused.join("; ")
            ),
        ));
    }

    // ── live windows ──
    match hyprctl.get_clients() {
        Ok(clients) => {
            let mut skipped_ignored = 0;
            let mut unrecoverable = Vec::new();
            let mut savable = 0;
            for client in &clients {
                if !client.is_restorable() {
                    continue;
                }
                if config.is_ignored(&client.class) {
                    skipped_ignored += 1;
                    continue;
                }
                if relaunch_argv(client, process, config).is_some() {
                    savable += 1;
                } else {
                    unrecoverable.push(client.class.clone());
                }
            }
            checks.push(Check::new(
                "windows",
                Status::Ok,
                format!("{savable} would be saved, {skipped_ignored} filtered out"),
            ));
            if !unrecoverable.is_empty() {
                unrecoverable.sort();
                unrecoverable.dedup();
                checks.push(Check::new(
                    "unrecoverable",
                    Status::Warn,
                    format!(
                        "no launch command could be derived for: {}",
                        unrecoverable.join(", ")
                    ),
                ));
            }
        }
        Err(e) => checks.push(Check::new("windows", Status::Fail, format!("{e}"))),
    }

    // ── stored sessions ──
    let default = &config.general.default_session;
    match load_session(default, sessions_dir) {
        Ok(session) => {
            let age = chrono::Utc::now() - session.created_at;
            checks.push(Check::new(
                "session",
                Status::Ok,
                format!(
                    "'{default}' holds {} windows, saved {} ago",
                    session.clients.len(),
                    format_age(age)
                ),
            ));
            checks.push(schema_check(&session));
        }
        // A snapshot from a newer hyprwake is not the same complaint as no
        // snapshot at all, and it is the one a compatibility report needs to
        // say out loud.
        Err(SessionError::TooNew { found, supported }) => checks.push(Check::new(
            "session",
            Status::Fail,
            format!(
                "'{default}' is schema v{found}; this build reads up to v{supported}. \
                 Upgrade hyprwake, or restore from an older snapshot."
            ),
        )),
        Err(_) => checks.push(Check::new(
            "session",
            Status::Warn,
            format!("no '{default}' session saved yet; run `hyprwake save`"),
        )),
    }
    if let Ok(all) = list_sessions(sessions_dir) {
        checks.push(Check::new(
            "stored",
            Status::Ok,
            format!(
                "{} session file(s) in {}",
                all.len(),
                sessions_dir.display()
            ),
        ));
    }

    // ── wiring ──
    if crate::omarchy::is_omarchy() {
        let hooks = crate::omarchy::hooks_dir();
        let installed = hooks.as_deref().is_some_and(crate::omarchy::is_installed);
        checks.push(if installed {
            Check::new("omarchy hooks", Status::Ok, "restore and watch run at boot")
        } else {
            Check::new(
                "omarchy hooks",
                Status::Warn,
                "not installed; run `hyprwake install`",
            )
        });
    }

    let service_dir = crate::service::systemd_user_dir();
    if crate::service::is_installed(&service_dir) {
        let active = crate::service::is_active();
        checks.push(Check::new(
            "watcher service",
            if active { Status::Ok } else { Status::Warn },
            if active {
                "running, and restarted if it dies".to_string()
            } else {
                format!(
                    "installed but not running: systemctl --user start {}",
                    crate::service::UNIT
                )
            },
        ));
    } else {
        checks.push(Check::new(
            "watcher service",
            Status::Warn,
            "not installed; `hyprwake install` adds a unit that keeps it alive",
        ));
    }

    let timer_dir = crate::autosave::systemd_user_dir();
    if crate::autosave::is_installed(&timer_dir) {
        let active = crate::autosave::is_active();
        checks.push(Check::new(
            "autosave timer",
            if active { Status::Ok } else { Status::Warn },
            if active {
                "installed and running"
            } else {
                "installed but not started: systemctl --user enable --now hyprwake-autosave.timer"
            },
        ));
    }

    checks
}

fn format_age(age: chrono::Duration) -> String {
    let secs = age.num_seconds().max(0);
    match secs {
        s if s < 90 => format!("{s}s"),
        s if s < 5400 => format!("{}m", s / 60),
        s if s < 172_800 => format!("{}h", s / 3600),
        s => format!("{}d", s / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hyprctl::mock::MockHyprctl;
    use crate::hyprctl::HyprClient;
    use crate::process::mock::MockProcessInfo;
    use crate::workspace::WorkspaceRef;
    use tempfile::TempDir;

    #[test]
    fn version_gate_matches_the_lua_era() {
        assert!(supports_lua_dispatchers("0.56.2"));
        assert!(supports_lua_dispatchers("0.55.0"));
        assert!(supports_lua_dispatchers("1.0.0"));
        assert!(!supports_lua_dispatchers("0.54.1"));
        assert!(!supports_lua_dispatchers("0.41.2"));
        assert!(!supports_lua_dispatchers("unknown"));
    }

    #[test]
    fn version_gate_tolerates_suffixes() {
        assert!(supports_lua_dispatchers("v0.56.2"));
        assert!(supports_lua_dispatchers("0.56-dirty"));
    }

    #[test]
    fn the_event_socket_is_only_reachable_if_something_accepts() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join(".socket2.sock");

        let absent = event_socket_check(Some(path.clone()));
        assert_eq!(
            absent.status,
            Status::Warn,
            "a set variable is not a socket"
        );
        assert!(absent.detail.contains("refused"), "{}", absent.detail);

        let _listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let present = event_socket_check(Some(path));
        assert_eq!(present.status, Status::Ok, "{}", present.detail);
    }

    #[test]
    fn layout_replay_without_preserve_split_is_a_warning() {
        use crate::config::LayoutMode;
        use crate::layout::LayoutOptions;
        let mut config: Config = toml::from_str("").unwrap();
        let opts = LayoutOptions::default();
        assert_eq!(layout_check(&config, &opts).status, Status::Ok);
        config.restore.layout = LayoutMode::BestEffort;
        let warn = layout_check(&config, &opts);
        assert_eq!(warn.status, Status::Warn);
        assert!(warn.detail.contains("preserve_split"), "{}", warn.detail);
        let kept = LayoutOptions {
            preserve_split: true,
            ..opts.clone()
        };
        assert_eq!(layout_check(&config, &kept).status, Status::Ok);
        let scrolling = LayoutOptions {
            layout: "scrolling".into(),
            ..opts
        };
        assert!(layout_check(&config, &scrolling)
            .detail
            .contains("cannot infer"));
    }

    #[test]
    fn regrouping_without_auto_group_is_a_warning() {
        let mut config: Config = toml::from_str("").unwrap();
        let mut hypr = crate::hyprctl::mock::MockHyprctl::default();
        assert!(
            groups_check(&config, &hypr).is_none(),
            "silent unless regrouping"
        );
        config.restore.groups = crate::config::GroupMode::Regroup;
        assert_eq!(groups_check(&config, &hypr).unwrap().status, Status::Ok);
        hypr.options.insert(
            "group.auto_group".into(),
            serde_json::json!({"bool": false}),
        );
        assert_eq!(groups_check(&config, &hypr).unwrap().status, Status::Warn);
    }

    #[test]
    fn no_signature_means_no_event_socket() {
        let check = event_socket_check(None);
        assert_eq!(check.status, Status::Warn);
        assert!(check.detail.contains("daemon"));
    }

    #[test]
    fn a_migrated_snapshot_says_which_version_it_came_from() {
        let session =
            crate::session::parse_session(include_str!("../tests/fixtures/session-v1.json"))
                .unwrap();
        let check = schema_check(&session);

        assert_eq!(check.status, Status::Ok, "an old snapshot still restores");
        assert!(check.detail.contains("v1"), "{}", check.detail);
        assert!(check.detail.contains("migrated"), "{}", check.detail);
        assert!(
            check
                .detail
                .contains("before hyprwake recorded its own version"),
            "{}",
            check.detail
        );
    }

    #[test]
    fn a_current_snapshot_names_the_hyprwake_that_wrote_it() {
        let dir = TempDir::new().unwrap();
        let mut session =
            crate::session::parse_session(include_str!("../tests/fixtures/session-v1.json"))
                .unwrap();
        session.name = "latest".to_string();
        session.created_by = "0.2.0".to_string();
        crate::session::save_session(&session, dir.path()).unwrap();

        let reloaded = crate::session::load_session("latest", dir.path()).unwrap();
        let check = schema_check(&reloaded);
        assert!(check.detail.contains("v2"), "{}", check.detail);
        assert!(!check.detail.contains("migrated"), "{}", check.detail);
        assert!(
            check.detail.contains("written by hyprwake 0.2.0"),
            "{}",
            check.detail
        );
    }

    #[test]
    fn checks_have_a_stable_machine_readable_shape() {
        let check = Check::new("session", Status::Warn, "none saved");
        assert_eq!(
            serde_json::to_value(check).unwrap(),
            serde_json::json!({
                "name": "session",
                "status": "warn",
                "detail": "none saved",
            })
        );
    }

    fn window(class: &str, pid: i32) -> HyprClient {
        HyprClient {
            address: format!("0x{pid}"),
            class: class.to_string(),
            initial_class: class.to_string(),
            title: String::new(),
            initial_title: String::new(),
            workspace: WorkspaceRef::new(1, "1"),
            monitor: 0,
            at: [0, 0],
            size: [10, 10],
            floating: false,
            pinned: false,
            fullscreen: 0,
            focus_history_id: 0,
            pid,
            mapped: true,
            grouped: vec![],
        }
    }

    fn find<'a>(checks: &'a [Check], name: &str) -> &'a Check {
        checks
            .iter()
            .find(|c| c.name == name)
            .expect("check missing")
    }

    #[test]
    fn reports_savable_and_filtered_windows() {
        let dir = TempDir::new().unwrap();
        let config: Config = toml::from_str("").unwrap();
        let mut proc = MockProcessInfo::default();
        proc.add(1, "foot", &["foot"], "/home/user");
        proc.add(2, "waybar", &["waybar"], "/");
        let hypr = MockHyprctl::new(vec![vec![window("foot", 1), window("waybar", 2)]]);

        let checks = run(&hypr, &proc, &config, dir.path());
        assert_eq!(
            find(&checks, "windows").detail,
            "1 would be saved, 1 filtered out"
        );
    }

    #[test]
    fn flags_windows_whose_process_is_unreadable() {
        let dir = TempDir::new().unwrap();
        let config: Config = toml::from_str("").unwrap();
        let proc = MockProcessInfo::default(); // knows nothing about any pid
        let hypr = MockHyprctl::new(vec![vec![window("obsidian", 9)]]);

        let checks = run(&hypr, &proc, &config, dir.path());
        let check = find(&checks, "unrecoverable");
        assert_eq!(check.status, Status::Warn);
        assert!(check.detail.contains("obsidian"));
    }

    #[test]
    fn warns_when_no_session_is_stored_yet() {
        let dir = TempDir::new().unwrap();
        let config: Config = toml::from_str("").unwrap();
        let checks = run(
            &MockHyprctl::default(),
            &MockProcessInfo::default(),
            &config,
            dir.path(),
        );
        assert_eq!(find(&checks, "session").status, Status::Warn);
    }

    #[test]
    fn ages_are_humanised() {
        assert_eq!(format_age(chrono::Duration::seconds(30)), "30s");
        assert_eq!(format_age(chrono::Duration::minutes(20)), "20m");
        assert_eq!(format_age(chrono::Duration::hours(5)), "5h");
        assert_eq!(format_age(chrono::Duration::days(3)), "3d");
    }
}
