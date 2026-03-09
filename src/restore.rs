use crate::config::Config;
use crate::hyprctl::{HyprctlClient, HyprctlError};
use crate::session::{Session, SessionClient};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

// ── Error ───────────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum RestoreError {
    #[error("hyprctl error: {0}")]
    Hyprctl(#[from] HyprctlError),
    #[error("no session found")]
    NoSession,
}

// ── Report ──────────────────────────────────────────────────────────────────

#[derive(Debug, Default)]
pub struct RestoreReport {
    pub restored: usize,
    pub skipped: usize,
    pub failed: usize,
    pub details: Vec<String>,
}

// ── Public entry point ───────────────────────────────────────────────────────

/// Restore a saved [`Session`] by launching every client and positioning
/// its window via `hyprctl dispatch`.
///
/// When `dry_run` is `true` no processes are spawned and no dispatches are
/// sent; the `details` field of the returned report lists what *would* have
/// been executed.
pub fn restore_session(
    session: &Session,
    hyprctl: &dyn HyprctlClient,
    config: &Config,
    dry_run: bool,
    verbose: bool,
) -> Result<RestoreReport, RestoreError> {
    let mut report = RestoreReport::default();

    // Fetch current windows once to detect already-running duplicates.
    let mut existing_counts: HashMap<(String, i32), usize> = HashMap::new();
    if !dry_run {
        if let Ok(current) = hyprctl.get_clients() {
            for c in &current {
                *existing_counts
                    .entry((c.class.clone(), c.workspace.id))
                    .or_insert(0) += 1;
            }
        }
    }

    // Detect if profile-based Brave restore applies.
    let has_brave_profiles = !session.brave_profiles.is_empty()
        && config.apps.contains_key("brave-browser");

    // Group by workspace (BTreeMap gives us sorted workspace order for free).
    let mut by_workspace: BTreeMap<i32, Vec<&SessionClient>> = BTreeMap::new();
    for client in &session.clients {
        by_workspace.entry(client.workspace).or_default().push(client);
    }

    for (ws, mut clients) in by_workspace {
        // Sort within each workspace: top row first, then left-to-right.
        clients.sort_by(|a, b| a.at[1].cmp(&b.at[1]).then(a.at[0].cmp(&b.at[0])));

        for client in clients {
            // Skip brave-browser windows when profiles are available (handled after main loop).
            if has_brave_profiles && client.class == "brave-browser" {
                continue;
            }

            if dry_run {
                let cmds = build_dispatch_commands(client);
                report.details.push(format!(
                    "[dry-run] ws={} {} → {}",
                    ws, client.class, client.launch.command
                ));
                for cmd in &cmds {
                    report.details.push(format!("  hyprctl dispatch {cmd}"));
                }
                report.restored += 1;
                continue;
            }

            // Count-based duplicate detection: skip if enough instances already exist.
            let key = (client.class.clone(), client.workspace);
            if let Some(count) = existing_counts.get_mut(&key) {
                if *count > 0 {
                    let msg = format!(
                        "SKIP: {} already on ws={}",
                        client.class, client.workspace
                    );
                    report.details.push(msg);
                    report.skipped += 1;
                    *count -= 1;
                    continue;
                }
            }

            // Validate the binary is available before attempting to spawn.
            if which::which(&client.launch.command).is_err() {
                let msg = format!(
                    "SKIP: binary '{}' not found for {}",
                    client.launch.command, client.class
                );
                if verbose {
                    report.details.push(msg);
                }
                report.skipped += 1;
                continue;
            }

            match restore_single_client(client, hyprctl, config, verbose) {
                Ok(msg) => {
                    if verbose {
                        report.details.push(msg);
                    }
                    report.restored += 1;
                }
                Err(e) => {
                    let msg = format!("FAIL: {} — {e}", client.class);
                    report.details.push(msg);
                    report.failed += 1;
                }
            }
        }
    }

    // Restore Brave profiles (one window per profile).
    if has_brave_profiles {
        let brave_config = config.apps.get("brave-browser");
        let binary = brave_config
            .and_then(|c| c.binary.clone())
            .unwrap_or_else(|| "brave".to_string());
        let default_ws = brave_config
            .and_then(|c| c.default_workspace)
            .unwrap_or(1);
        let profile_ws = brave_config
            .and_then(|c| c.profile_workspaces.as_ref());

        if !dry_run && which::which(&binary).is_err() {
            let msg = format!("SKIP: binary '{}' not found for Brave profiles", binary);
            report.details.push(msg);
            report.skipped += session.brave_profiles.len();
        } else {
            for profile in &session.brave_profiles {
                let ws = profile_ws
                    .and_then(|m| m.get(&profile.directory))
                    .copied()
                    .unwrap_or(default_ws);

                if dry_run {
                    report.details.push(format!(
                        "[dry-run] brave profile \"{}\" ({}) → ws={}",
                        profile.name, profile.directory, ws
                    ));
                    report.details.push(format!(
                        "  {} --profile-directory={}", binary, profile.directory
                    ));
                    report.details.push(format!(
                        "  hyprctl dispatch movetoworkspacesilent {},address:0xNEW", ws
                    ));
                    report.restored += 1;
                    continue;
                }

                // Snapshot existing addresses BEFORE spawning (avoid race condition).
                let before: HashSet<String> = hyprctl
                    .get_clients()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|c| c.address)
                    .collect();

                // Launch brave with profile directory.
                let spawn_result = Command::new(&binary)
                    .arg(format!("--profile-directory={}", profile.directory))
                    .spawn();

                match spawn_result {
                    Ok(_) => {
                        let timeout = Duration::from_millis(config.general.window_detect_timeout_ms);
                        let poll_interval = Duration::from_millis(100);
                        let start = Instant::now();

                        let new_addr = loop {
                            if start.elapsed() > timeout {
                                report.details.push(format!(
                                    "FAIL: timeout waiting for brave profile \"{}\"", profile.name
                                ));
                                report.failed += 1;
                                break None;
                            }
                            thread::sleep(poll_interval);

                            if let Ok(current) = hyprctl.get_clients() {
                                if let Some(w) = current
                                    .into_iter()
                                    .find(|c| !before.contains(&c.address) && c.class == "brave-browser")
                                {
                                    break Some(w.address);
                                }
                            }
                        };

                        if let Some(addr) = new_addr {
                            // Move to target workspace (no pixel positioning for Brave).
                            let _ = hyprctl.dispatch(&format!(
                                "movetoworkspacesilent {},address:{}", ws, addr
                            ));

                            if verbose {
                                report.details.push(format!(
                                    "OK: brave profile \"{}\" ({}) → ws={}",
                                    profile.name, profile.directory, ws
                                ));
                            }
                            report.restored += 1;
                        }

                        // Throttle between launches.
                        thread::sleep(Duration::from_millis(config.general.restore_delay_ms));
                    }
                    Err(e) => {
                        report.details.push(format!(
                            "FAIL: brave profile \"{}\" — spawn error: {}", profile.name, e
                        ));
                        report.failed += 1;
                    }
                }
            }
        }
    }

    Ok(report)
}

// ── Per-client restore logic ────────────────────────────────────────────────

fn restore_single_client(
    client: &SessionClient,
    hyprctl: &dyn HyprctlClient,
    config: &Config,
    _verbose: bool,
) -> Result<String, RestoreError> {
    // 1. Snapshot existing window addresses before launching.
    let before: HashSet<String> = hyprctl
        .get_clients()?
        .into_iter()
        .map(|c| c.address)
        .collect();

    // 2. Build and spawn the launch command.
    let launch_cmd = build_launch_command(client);
    Command::new(&launch_cmd[0])
        .args(&launch_cmd[1..])
        .spawn()
        .map_err(|e| {
            HyprctlError::CommandFailed(format!(
                "spawn '{}' failed: {e}",
                client.launch.command
            ))
        })?;

    // 3. Poll for the new window (address not in snapshot + class match).
    let timeout = Duration::from_millis(config.general.window_detect_timeout_ms);
    let poll_interval = Duration::from_millis(100);
    let start = Instant::now();

    let new_addr = loop {
        if start.elapsed() > timeout {
            return Err(RestoreError::Hyprctl(HyprctlError::CommandFailed(
                format!("timeout waiting for '{}' window to appear", client.class),
            )));
        }
        thread::sleep(poll_interval);

        let current = hyprctl.get_clients()?;
        if let Some(w) = current
            .into_iter()
            .find(|c| !before.contains(&c.address) && c.class == client.class)
        {
            break w.address;
        }
    };

    // 4. Move to target workspace (silently, without switching).
    hyprctl.dispatch(&format!(
        "movetoworkspacesilent {},address:{}",
        client.workspace, new_addr
    ))?;

    // 5. Resize then position (order matters: resize first, then move).
    hyprctl.dispatch(&format!(
        "resizewindowpixel exact {} {},address:{}",
        client.size[0], client.size[1], new_addr
    ))?;
    hyprctl.dispatch(&format!(
        "movewindowpixel exact {} {},address:{}",
        client.at[0], client.at[1], new_addr
    ))?;

    // 6. Apply floating / fullscreen state.
    if client.floating {
        hyprctl.dispatch(&format!("togglefloating address:{}", new_addr))?;
    }
    if client.fullscreen > 0 {
        hyprctl.dispatch(&format!("fullscreen {}", client.fullscreen))?;
    }

    // 7. Throttle subsequent launches to give the compositor time to settle.
    thread::sleep(Duration::from_millis(config.general.restore_delay_ms));

    Ok(format!(
        "OK: {} → ws={} at {:?}",
        client.class, client.workspace, client.at
    ))
}

// ── Command builders (pure functions, unit-testable) ─────────────────────────

/// Build the argv vector used to spawn `client`'s application.
///
/// For `kitty` windows that carry a `hint` (e.g. the last shell command),
/// we append `-e zsh -c "<hint>; exec zsh"` so the terminal opens with
/// that hint visible and then drops to an interactive shell.
pub fn build_launch_command(client: &SessionClient) -> Vec<String> {
    let mut cmd = vec![client.launch.command.clone()];
    cmd.extend(client.launch.args.clone());

    if client.class == "kitty" {
        if let Some(hint) = &client.launch.hint {
            // Single-quote-escape the hint so it survives the shell invocation.
            let escaped = hint.replace('\'', "'\\''");
            cmd.push("-e".to_string());
            cmd.push("zsh".to_string());
            cmd.push("-c".to_string());
            cmd.push(format!("echo '{escaped}'; exec zsh"));
        }
    }

    cmd
}

/// Build the list of `hyprctl dispatch` argument strings that would be
/// issued for a given client.  Used both by the dry-run path and by tests.
pub fn build_dispatch_commands(client: &SessionClient) -> Vec<String> {
    let addr = "address:0xNEW";
    let launch = build_launch_command(client);

    let mut cmds = vec![
        format!("exec {}", launch.join(" ")),
        format!("movetoworkspacesilent {},{}", client.workspace, addr),
        format!(
            "resizewindowpixel exact {} {},{}",
            client.size[0], client.size[1], addr
        ),
        format!(
            "movewindowpixel exact {} {},{}",
            client.at[0], client.at[1], addr
        ),
    ];

    if client.floating {
        cmds.push(format!("togglefloating {addr}"));
    }
    if client.fullscreen > 0 {
        cmds.push(format!("fullscreen {}", client.fullscreen));
    }

    cmds
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AppConfig, FilterConfig, GeneralConfig};
    use crate::hyprctl::{HyprClient, HyprMonitor};
    use crate::session::{BraveProfile, LaunchInfo, Session, SessionClient};
    use chrono::Utc;
    use std::cell::RefCell;

    // ── Helpers ──────────────────────────────────────────────────────────────

    fn make_client(
        class: &str,
        workspace: i32,
        at: [i32; 2],
        size: [i32; 2],
        floating: bool,
        fullscreen: u8,
        command: &str,
        args: Vec<String>,
        hint: Option<String>,
    ) -> SessionClient {
        SessionClient {
            class: class.to_string(),
            title: class.to_string(),
            workspace,
            monitor: "DP-1".to_string(),
            at,
            size,
            floating,
            fullscreen,
            focus_history_id: 0,
            launch: LaunchInfo {
                command: command.to_string(),
                args,
                hint,
            },
        }
    }

    fn make_session(clients: Vec<SessionClient>) -> Session {
        Session {
            name: "test".to_string(),
            created_at: Utc::now(),
            hyprland_version: "0.54.0".to_string(),
            monitors: vec![],
            clients,
            brave_profiles: vec![],
        }
    }

    // ── MockHyprctl ───────────────────────────────────────────────────────────

    /// A mock that returns pre-programmed client snapshots on successive
    /// `get_clients()` calls, simulating a new window appearing.
    struct MockHyprctl {
        /// One entry per `get_clients()` call; last entry is repeated if exhausted.
        client_states: RefCell<Vec<Vec<HyprClient>>>,
        state_index: RefCell<usize>,
        dispatches: RefCell<Vec<String>>,
    }

    impl MockHyprctl {
        fn new(client_states: Vec<Vec<HyprClient>>) -> Self {
            Self {
                client_states: RefCell::new(client_states),
                state_index: RefCell::new(0),
                dispatches: RefCell::new(Vec::new()),
            }
        }

        fn dispatches(&self) -> Vec<String> {
            self.dispatches.borrow().clone()
        }
    }

    impl HyprctlClient for MockHyprctl {
        fn get_clients(&self) -> Result<Vec<HyprClient>, HyprctlError> {
            let idx = *self.state_index.borrow();
            let states = self.client_states.borrow();
            // Clamp to last state once exhausted.
            let effective = idx.min(states.len().saturating_sub(1));
            let result = states.get(effective).cloned().unwrap_or_default();
            drop(states);
            *self.state_index.borrow_mut() = idx + 1;
            Ok(result)
        }

        fn get_monitors(&self) -> Result<Vec<HyprMonitor>, HyprctlError> {
            Ok(vec![])
        }

        fn dispatch(&self, args: &str) -> Result<(), HyprctlError> {
            self.dispatches.borrow_mut().push(args.to_string());
            Ok(())
        }

        fn get_hyprland_version(&self) -> Result<String, HyprctlError> {
            Ok("0.54.1".to_string())
        }
    }

    // ── Test: dry-run generates commands without dispatching ─────────────────

    #[test]
    fn test_restore_dry_run_generates_commands() {
        let client = make_client(
            "kitty",
            1,
            [100, 200],
            [800, 600],
            false,
            0,
            "kitty",
            vec!["--directory".to_string(), "/home/user".to_string()],
            Some("claude --continue".to_string()),
        );
        let session = make_session(vec![client]);
        let config = Config::default();
        // The mock will never be called for dispatches in dry-run mode.
        let mock = MockHyprctl::new(vec![]);

        let report = restore_session(&session, &mock, &config, true, true).unwrap();

        // Dry-run should count the client as "restored" and emit detail lines.
        assert_eq!(report.restored, 1);
        assert_eq!(report.skipped, 0);
        assert_eq!(report.failed, 0);
        // At minimum: one header line + one or more dispatch lines.
        assert!(
            !report.details.is_empty(),
            "dry-run should produce detail lines"
        );
        // Header line must mention the client class.
        let header = &report.details[0];
        assert!(
            header.contains("kitty"),
            "header should contain class name; got: {header}"
        );
        assert!(
            header.contains("[dry-run]"),
            "header should be tagged [dry-run]; got: {header}"
        );
        // No real dispatches should have been recorded.
        assert!(
            mock.dispatches().is_empty(),
            "dry-run must not send real hyprctl dispatches"
        );
    }

    // ── Test: build_launch_command for kitty with hint ───────────────────────

    #[test]
    fn test_build_launch_command_kitty_with_hint() {
        let client = make_client(
            "kitty",
            1,
            [0, 0],
            [800, 600],
            false,
            0,
            "kitty",
            vec!["--directory".to_string(), "/home/user/project".to_string()],
            Some("claude --continue".to_string()),
        );

        let cmd = build_launch_command(&client);

        // argv[0] is the binary.
        assert_eq!(cmd[0], "kitty");
        // Existing args are preserved before the hint block.
        assert!(
            cmd.contains(&"--directory".to_string()),
            "should keep --directory arg"
        );
        assert!(
            cmd.contains(&"/home/user/project".to_string()),
            "should keep directory value"
        );
        // The hint block must be present.
        let joined = cmd.join(" ");
        assert!(
            joined.contains("-e zsh -c"),
            "kitty hint should inject '-e zsh -c'; got: {joined}"
        );
        assert!(
            joined.contains("claude --continue"),
            "hint content should appear in command; got: {joined}"
        );
        assert!(
            joined.contains("exec zsh"),
            "hint block should drop to interactive zsh; got: {joined}"
        );
    }

    // ── Test: build_launch_command for a generic binary ─────────────────────

    #[test]
    fn test_build_launch_command_generic() {
        let client = make_client(
            "brave-browser",
            1,
            [0, 0],
            [1280, 800],
            false,
            0,
            "brave-browser",
            vec!["--profile-directory=Default".to_string()],
            None,
        );

        let cmd = build_launch_command(&client);

        assert_eq!(cmd[0], "brave-browser");
        assert_eq!(cmd[1], "--profile-directory=Default");
        assert_eq!(cmd.len(), 2, "no extra args should be appended for non-kitty");
    }

    // ── Test: build_dispatch_commands produces correct sequence ──────────────

    #[test]
    fn test_build_dispatch_commands() {
        let client = make_client(
            "obsidian",
            3,
            [50, 100],
            [1200, 900],
            true,  // floating
            0,
            "obsidian",
            vec![],
            None,
        );

        let cmds = build_dispatch_commands(&client);

        // Must start with exec.
        assert!(cmds[0].starts_with("exec "), "first command must be exec");
        // Workspace move must come before resize/move.
        let ws_idx = cmds.iter().position(|c| c.starts_with("movetoworkspacesilent")).unwrap();
        let resize_idx = cmds.iter().position(|c| c.starts_with("resizewindowpixel")).unwrap();
        let move_idx = cmds.iter().position(|c| c.starts_with("movewindowpixel")).unwrap();
        assert!(ws_idx < resize_idx, "workspace move must precede resize");
        assert!(resize_idx < move_idx, "resize must precede position move");

        // Workspace number must appear in the movetoworkspacesilent command.
        assert!(
            cmds[ws_idx].contains("3"),
            "workspace 3 must appear in dispatch; got: {}",
            cmds[ws_idx]
        );
        // Floating togglefloating must be present.
        let float_cmd = cmds.iter().find(|c| c.starts_with("togglefloating"));
        assert!(float_cmd.is_some(), "floating client should have togglefloating dispatch");

        // fullscreen=0 means no fullscreen dispatch.
        assert!(
            !cmds.iter().any(|c| c.starts_with("fullscreen")),
            "non-fullscreen client should not have fullscreen dispatch"
        );
    }

    // ── Test: skips client when binary is missing ────────────────────────────

    #[test]
    fn test_restore_skips_missing_binary() {
        let client = make_client(
            "nonexistent_app_xyz",
            1,
            [0, 0],
            [800, 600],
            false,
            0,
            "nonexistent_app_xyz_abc_123", // guaranteed not to exist
            vec![],
            None,
        );
        let session = make_session(vec![client]);
        let config = Config::default();
        let mock = MockHyprctl::new(vec![]);

        let report = restore_session(&session, &mock, &config, false, true).unwrap();

        assert_eq!(report.skipped, 1, "missing binary should be skipped");
        assert_eq!(report.restored, 0);
        assert_eq!(report.failed, 0);
        // No dispatches should have been sent.
        assert!(mock.dispatches().is_empty());
    }

    // ── Test: skips duplicate class+workspace already running ────────────

    #[test]
    fn test_restore_skips_duplicate_class_workspace() {
        let existing_window = HyprClient {
            address: "0xexisting".to_string(),
            class: "kitty".to_string(),
            title: "kitty".to_string(),
            workspace: crate::hyprctl::HyprWorkspace {
                id: 1,
                name: "1".to_string(),
            },
            monitor: 0,
            at: [0, 0],
            size: [800, 600],
            floating: false,
            fullscreen: 0,
            focus_history_id: 0,
            pid: 9999,
        };

        // First get_clients() call returns the existing window (duplicate check).
        // Subsequent calls would also return it (mock clamps to last state).
        let mock = MockHyprctl::new(vec![vec![existing_window]]);

        let client = make_client(
            "kitty",
            1,
            [0, 0],
            [800, 600],
            false,
            0,
            "kitty",
            vec![],
            None,
        );
        let session = make_session(vec![client]);
        let config = Config::default();

        let report = restore_session(&session, &mock, &config, false, true).unwrap();

        assert_eq!(report.skipped, 1, "duplicate should be skipped");
        assert_eq!(report.restored, 0);
        assert_eq!(report.failed, 0);
        assert!(
            report
                .details
                .iter()
                .any(|d| d.contains("SKIP: kitty already on ws=1")),
            "details should mention the skipped duplicate; got: {:?}",
            report.details
        );
        // No dispatches should have been sent.
        assert!(mock.dispatches().is_empty());
    }

    // ── Test: dry-run does NOT skip duplicates ──────────────────────────

    #[test]
    fn test_restore_dry_run_ignores_duplicates() {
        let existing_window = HyprClient {
            address: "0xexisting".to_string(),
            class: "kitty".to_string(),
            title: "kitty".to_string(),
            workspace: crate::hyprctl::HyprWorkspace {
                id: 1,
                name: "1".to_string(),
            },
            monitor: 0,
            at: [0, 0],
            size: [800, 600],
            floating: false,
            fullscreen: 0,
            focus_history_id: 0,
            pid: 9999,
        };

        // Even though the existing window matches, dry-run should ignore it.
        let mock = MockHyprctl::new(vec![vec![existing_window]]);

        let client = make_client(
            "kitty",
            1,
            [0, 0],
            [800, 600],
            false,
            0,
            "kitty",
            vec![],
            None,
        );
        let session = make_session(vec![client]);
        let config = Config::default();

        let report = restore_session(&session, &mock, &config, true, true).unwrap();

        assert_eq!(
            report.restored, 1,
            "dry-run should not skip duplicates"
        );
        assert_eq!(report.skipped, 0);
        assert_eq!(report.failed, 0);
        // No real dispatches in dry-run.
        assert!(mock.dispatches().is_empty());
    }

    // ── Test: partial duplicates — restore only the missing count ────────

    #[test]
    fn test_restore_partial_duplicate_restores_missing() {
        // 2 existing "testapp" windows on ws=5.
        let existing = vec![
            HyprClient {
                address: "0xaaa".to_string(),
                class: "testapp".to_string(),
                title: "testapp".to_string(),
                workspace: crate::hyprctl::HyprWorkspace {
                    id: 5,
                    name: "5".to_string(),
                },
                monitor: 0,
                at: [0, 0],
                size: [800, 600],
                floating: false,
                fullscreen: 0,
                focus_history_id: 0,
                pid: 1001,
            },
            HyprClient {
                address: "0xbbb".to_string(),
                class: "testapp".to_string(),
                title: "testapp".to_string(),
                workspace: crate::hyprctl::HyprWorkspace {
                    id: 5,
                    name: "5".to_string(),
                },
                monitor: 0,
                at: [100, 0],
                size: [800, 600],
                floating: false,
                fullscreen: 0,
                focus_history_id: 0,
                pid: 1002,
            },
        ];

        let mock = MockHyprctl::new(vec![existing]);

        // Session wants 3 "testapp" on ws=5 with a nonexistent binary.
        let clients: Vec<SessionClient> = (0..3)
            .map(|i| {
                make_client(
                    "testapp",
                    5,
                    [i * 100, 0],
                    [800, 600],
                    false,
                    0,
                    "nonexistent_binary_xyz_123",
                    vec![],
                    None,
                )
            })
            .collect();

        let session = make_session(clients);
        let config = Config::default();

        let report = restore_session(&session, &mock, &config, false, true).unwrap();

        // 2 skipped as duplicates, 1 skipped as binary-not-found → total 3.
        assert_eq!(report.skipped, 3, "expected 3 skipped; got {}", report.skipped);
        assert_eq!(report.restored, 0);
        assert_eq!(report.failed, 0);

        // Exactly 2 detail lines should mention "already on ws=".
        let dup_msgs: Vec<_> = report
            .details
            .iter()
            .filter(|d| d.contains("SKIP: testapp already on ws=5"))
            .collect();
        assert_eq!(
            dup_msgs.len(),
            2,
            "expected 2 duplicate-skip messages; got {:?}",
            report.details
        );

        // Exactly 1 detail line should mention "binary".
        let bin_msgs: Vec<_> = report
            .details
            .iter()
            .filter(|d| d.contains("binary"))
            .collect();
        assert_eq!(
            bin_msgs.len(),
            1,
            "expected 1 binary-not-found message; got {:?}",
            report.details
        );

        // No dispatches should have been sent.
        assert!(mock.dispatches().is_empty());
    }

    // ── Test: restore brave by profile in dry-run mode ───────────────────

    #[test]
    fn test_restore_brave_by_profile_dry_run() {
        let session = Session {
            name: "test".to_string(),
            created_at: Utc::now(),
            hyprland_version: "0.54.1".to_string(),
            monitors: vec![],
            clients: vec![
                make_client("brave-browser", 1, [0, 0], [800, 600], false, 0, "brave", vec![], None),
                make_client("brave-browser", 8, [0, 0], [800, 600], false, 0, "brave", vec![], None),
                make_client("kitty", 4, [0, 0], [800, 600], false, 0, "kitty", vec![], None),
            ],
            brave_profiles: vec![
                BraveProfile { directory: "Default".to_string(), name: "Credifit".to_string() },
                BraveProfile { directory: "Profile 1".to_string(), name: "LinkPJ".to_string() },
            ],
        };

        let mut apps = HashMap::new();
        apps.insert("brave-browser".to_string(), AppConfig {
            binary: Some("brave".to_string()),
            capture_cwd: None,
            capture_last_command: None,
            hint_template: None,
            profile_workspaces: Some(HashMap::from([
                ("Default".to_string(), 1),
                ("Profile 1".to_string(), 6),
            ])),
            default_workspace: Some(1),
        });

        let config = Config {
            general: GeneralConfig::default(),
            filters: FilterConfig::default(),
            apps,
        };

        let mock = MockHyprctl::new(vec![]);
        let report = restore_session(&session, &mock, &config, true, true).unwrap();

        // 2 profiles restored + 1 kitty = 3 restored
        assert_eq!(report.restored, 3, "should restore 2 profiles + 1 kitty; got details: {:?}", report.details);

        // Brave individual windows should NOT appear in details (they were skipped)
        let brave_individual: Vec<_> = report.details.iter()
            .filter(|d| d.contains("[dry-run] ws=") && d.contains("brave"))
            .collect();
        assert!(brave_individual.is_empty(), "individual brave windows should be skipped; got: {:?}", brave_individual);

        // Profile entries should appear
        assert!(report.details.iter().any(|d| d.contains("Credifit")), "should have Credifit profile; got: {:?}", report.details);
        assert!(report.details.iter().any(|d| d.contains("LinkPJ")), "should have LinkPJ profile; got: {:?}", report.details);

        // Kitty should still be present
        assert!(report.details.iter().any(|d| d.contains("kitty")), "kitty should be restored normally; got: {:?}", report.details);
    }

    // ── Test: unmapped profile uses default_workspace fallback ──────────

    #[test]
    fn test_restore_brave_profile_uses_default_workspace() {
        let session = Session {
            name: "test".to_string(),
            created_at: Utc::now(),
            hyprland_version: "0.54.1".to_string(),
            monitors: vec![],
            clients: vec![],
            brave_profiles: vec![
                BraveProfile {
                    directory: "Profile 9".to_string(),
                    name: "Unmapped".to_string(),
                },
            ],
        };

        let mut apps = HashMap::new();
        apps.insert("brave-browser".to_string(), AppConfig {
            binary: Some("brave".to_string()),
            capture_cwd: None,
            capture_last_command: None,
            hint_template: None,
            profile_workspaces: Some(HashMap::from([
                ("Default".to_string(), 1),
            ])),
            default_workspace: Some(3),
        });

        let config = Config {
            general: GeneralConfig::default(),
            filters: FilterConfig { ignore_classes: vec![] },
            apps,
        };

        let mock = MockHyprctl::new(vec![]);
        let report = restore_session(&session, &mock, &config, true, true).unwrap();

        assert_eq!(report.restored, 1);
        // Should use default_workspace=3 since "Profile 9" has no mapping
        assert!(
            report.details.iter().any(|d| d.contains("ws=3")),
            "unmapped profile should use default_workspace=3; got: {:?}",
            report.details
        );
    }

    // ── Test: without profiles, brave windows restore individually ───────

    #[test]
    fn test_restore_brave_without_profiles_falls_back() {
        // Session WITHOUT brave_profiles — should restore brave windows normally
        let session = Session {
            name: "test".to_string(),
            created_at: Utc::now(),
            hyprland_version: "0.54.1".to_string(),
            monitors: vec![],
            clients: vec![
                make_client("brave-browser", 1, [0, 0], [800, 600], false, 0, "brave", vec![], None),
            ],
            brave_profiles: vec![],  // no profiles
        };

        let config = Config::default();
        let mock = MockHyprctl::new(vec![]);
        let report = restore_session(&session, &mock, &config, true, true).unwrap();

        // Without profiles, brave windows are restored individually
        assert_eq!(report.restored, 1);
        assert!(report.details.iter().any(|d| d.contains("[dry-run]") && d.contains("brave")));
    }
}
