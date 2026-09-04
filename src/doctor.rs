//! `hyprwake doctor` — what would happen if you saved and restored right now.

use crate::capture::relaunch_argv;
use crate::config::Config;
use crate::hyprctl::HyprctlClient;
use crate::process::ProcessInfoProvider;
use crate::session::{list_sessions, load_session};
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

    let has_signature = std::env::var("HYPRLAND_INSTANCE_SIGNATURE").is_ok();
    checks.push(if has_signature {
        Check::new("event socket", Status::Ok, "reachable; `watch` can run")
    } else {
        Check::new(
            "event socket",
            Status::Warn,
            "HYPRLAND_INSTANCE_SIGNATURE unset; use `daemon` instead of `watch`",
        )
    });

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
    let mut missing = Vec::new();
    let mut present = Vec::new();
    for (class, term) in &config.terminals {
        if which::which(&term.binary).is_ok() {
            present.push(class.clone());
        } else {
            missing.push(format!("{class} ({})", term.binary));
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
        }
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
