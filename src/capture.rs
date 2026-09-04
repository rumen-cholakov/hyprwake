//! Turning the live desktop into a [`Session`].

use crate::browsers;
use crate::config::{AppConfig, Config};
use crate::hyprctl::{HyprClient, HyprctlClient, HyprctlError};
use crate::process::ProcessInfoProvider;
use crate::session::{BrowserProfile, LaunchInfo, Monitor, Session, SessionClient};
use chrono::Utc;
use std::collections::{HashMap, HashSet};

#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error("hyprctl error: {0}")]
    Hyprctl(#[from] HyprctlError),
}

/// Arguments that name a per-run temporary file. Replaying them would point
/// the restored program at a path that no longer exists.
const VOLATILE_ARG_PREFIXES: &[&str] = &["--cwd-file", "--chooser-file", "--choosefile"];

/// How deep to look inside a terminal for a shell or a TUI.
const PROCESS_SEARCH_DEPTH: usize = 5;

pub fn capture_session(
    name: &str,
    hyprctl: &dyn HyprctlClient,
    process: &dyn ProcessInfoProvider,
    config: &Config,
) -> Result<Session, CaptureError> {
    let raw_clients = hyprctl.get_clients()?;
    let raw_monitors = hyprctl.get_monitors().unwrap_or_default();
    let version = hyprctl
        .get_hyprland_version()
        .unwrap_or_else(|_| "unknown".to_string());

    let monitor_names: HashMap<i32, String> = raw_monitors
        .iter()
        .map(|m| (m.id, m.name.clone()))
        .collect();

    let browser_profiles = collect_browser_profiles(&raw_clients, config);
    // Classes whose windows the browser itself will reopen, one per profile.
    let profile_driven: HashSet<&str> = browser_profiles.iter().map(|p| p.class.as_str()).collect();

    let mut clients = Vec::new();
    let mut seen_pids = HashSet::new();

    for raw in &raw_clients {
        if !raw.is_restorable() || config.is_ignored(&raw.class) {
            continue;
        }
        let Some(argv) = relaunch_argv(raw, process, config) else {
            // No argv means the process vanished between the query and the
            // read, or it is not ours to relaunch.
            continue;
        };

        let app = config.apps.get(&raw.class);
        let first_of_process = seen_pids.insert(raw.pid);
        let spawn = first_of_process
            && !app.is_some_and(|a| a.no_spawn)
            && !profile_driven.contains(raw.class.as_str());

        clients.push(SessionClient {
            class: raw.class.clone(),
            title: raw.title.clone(),
            workspace: raw.workspace.clone(),
            monitor: monitor_names
                .get(&raw.monitor)
                .cloned()
                .unwrap_or_else(|| format!("monitor-{}", raw.monitor)),
            at: raw.at,
            size: raw.size,
            floating: raw.floating,
            pinned: raw.pinned,
            fullscreen: raw.fullscreen,
            focus_history_id: raw.focus_history_id,
            launch: LaunchInfo { argv, spawn },
        });
    }

    Ok(Session {
        name: name.to_string(),
        created_at: Utc::now(),
        hyprland_version: version,
        monitors: raw_monitors
            .iter()
            .map(|m| Monitor {
                name: m.name.clone(),
                width: m.width,
                height: m.height,
                transform: m.transform,
            })
            .collect(),
        clients,
        browser_profiles,
    })
}

/// The argv that recreates this window's process, or `None` when there is
/// nothing usable to replay.
pub fn relaunch_argv(
    client: &HyprClient,
    process: &dyn ProcessInfoProvider,
    config: &Config,
) -> Option<Vec<String>> {
    if let Some(term) = config.terminal_for(&client.class) {
        let home = home_dir();

        // A TUI is the reason the terminal is open; reopen it in place.
        if let Some(tui) =
            process.find_descendant(client.pid, &config.tui_programs(), PROCESS_SEARCH_DEPTH)
        {
            let argv = process
                .cmdline(tui)
                .or_else(|| process.comm(tui).map(|c| vec![c]))?;
            let argv: Vec<String> = argv
                .into_iter()
                .filter(|a| !VOLATILE_ARG_PREFIXES.iter().any(|p| a.starts_with(p)))
                .collect();
            let cwd = process
                .cwd(tui)
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or(home);
            return Some(term.build_argv(&cwd, Some(&argv)));
        }

        // Otherwise reopen the terminal where its shell was standing.
        let shell = process.find_descendant(client.pid, &config.shells(), PROCESS_SEARCH_DEPTH);
        let cwd = shell
            .and_then(|pid| process.cwd(pid))
            .or_else(|| process.cwd(client.pid))
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or(home);
        return Some(term.build_argv(&cwd, None));
    }

    let mut argv = split_blob_cmdline(process.cmdline(client.pid)?);
    if let Some(app) = config.apps.get(&client.class) {
        apply_app_overrides(&mut argv, app);
    }
    if argv.is_empty() {
        return None;
    }
    Some(argv)
}

/// Recover an argv that the process flattened into one blob.
///
/// Chromium-family browsers rewrite their own argv so the process title reads
/// as a single string, and `/proc/<pid>/cmdline` then holds one entry with
/// spaces instead of NUL-separated arguments. Replaying that verbatim asks
/// the shell to run a binary whose name is the entire command line.
///
/// Splitting is only safe when the leading token is itself executable, which
/// leaves genuine paths containing spaces untouched.
pub fn split_blob_cmdline(argv: Vec<String>) -> Vec<String> {
    if argv.len() != 1 {
        return argv;
    }
    let Some((first, rest)) = argv[0].split_once(' ') else {
        return argv;
    };
    if rest.is_empty() {
        return argv;
    }
    let looks_executable =
        std::path::Path::new(first).is_file() || which::which(first).is_ok();
    if !looks_executable {
        return argv;
    }
    argv[0].split_whitespace().map(String::from).collect()
}

fn apply_app_overrides(argv: &mut Vec<String>, app: &AppConfig) {
    if let Some(binary) = &app.binary {
        if argv.is_empty() {
            argv.push(binary.clone());
        } else {
            argv[0] = binary.clone();
        }
    }
    if !app.strip_args.is_empty() {
        argv.retain(|a| !app.strip_args.iter().any(|s| a.starts_with(s)));
    }
}

/// Profile windows for every configured browser that is currently open.
fn collect_browser_profiles(clients: &[HyprClient], config: &Config) -> Vec<BrowserProfile> {
    let open: HashSet<&str> = clients
        .iter()
        .filter(|c| c.is_restorable())
        .map(|c| c.class.as_str())
        .collect();

    let mut out = Vec::new();
    for (class, browser) in &config.browsers {
        if !open.contains(class.as_str()) || browser.profile_workspaces.is_empty() {
            continue;
        }
        match browsers::read_profiles(class, browser) {
            Ok(profiles) => out.extend(browsers::assign_workspaces(profiles, browser)),
            Err(e) => {
                eprintln!("hyprwake: could not read {class} profiles: {e}");
                crate::logging::log(format!("browser profile read failed for {class}: {e}"));
            }
        }
    }
    out.sort_by(|a, b| a.directory.cmp(&b.directory));
    out
}

fn home_dir() -> String {
    dirs::home_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "/".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hyprctl::mock::MockHyprctl;
    use crate::process::mock::MockProcessInfo;
    use crate::workspace::WorkspaceRef;

    fn client(class: &str, pid: i32) -> HyprClient {
        HyprClient {
            address: format!("0x{pid}"),
            class: class.to_string(),
            initial_class: class.to_string(),
            title: "t".to_string(),
            workspace: WorkspaceRef::new(1, "1"),
            monitor: 0,
            at: [0, 0],
            size: [800, 600],
            floating: false,
            pinned: false,
            fullscreen: 0,
            focus_history_id: 0,
            pid,
            mapped: true,
        }
    }

    fn config() -> Config {
        toml::from_str("").unwrap()
    }

    #[test]
    fn plain_app_replays_its_own_argv() {
        let mut proc = MockProcessInfo::default();
        proc.add(
            50,
            "chrome",
            &["/opt/google/chrome/chrome", "--ozone-platform=wayland"],
            "/home/rc",
        );
        let argv = relaunch_argv(&client("google-chrome", 50), &proc, &config()).unwrap();
        assert_eq!(argv[0], "/opt/google/chrome/chrome");
        assert_eq!(argv[1], "--ozone-platform=wayland");
    }

    #[test]
    fn terminal_reopens_at_its_shell_directory() {
        let mut proc = MockProcessInfo::default();
        proc.add(10, "foot", &["foot"], "/home/rc");
        proc.add(11, "fish", &["fish"], "/home/rc/Work/hyprwake");
        proc.link(10, 11);
        let argv = relaunch_argv(&client("foot", 10), &proc, &config()).unwrap();
        assert_eq!(argv, vec!["foot", "-D", "/home/rc/Work/hyprwake"]);
    }

    #[test]
    fn terminal_running_a_tui_reopens_the_tui() {
        let mut proc = MockProcessInfo::default();
        proc.add(10, "foot", &["foot"], "/home/rc");
        proc.add(11, "fish", &["fish"], "/home/rc");
        proc.add(12, "nvim", &["nvim", "src/main.rs"], "/home/rc/Work");
        proc.link(10, 11).link(11, 12);
        let argv = relaunch_argv(&client("foot", 10), &proc, &config()).unwrap();
        assert_eq!(
            argv,
            vec!["foot", "-D", "/home/rc/Work", "nvim", "src/main.rs"]
        );
    }

    #[test]
    fn volatile_tui_arguments_are_dropped() {
        let mut proc = MockProcessInfo::default();
        proc.add(10, "foot", &["foot"], "/home/rc");
        proc.add(
            11,
            "yazi",
            &["yazi", "--cwd-file=/run/user/1000/yazi-cwd.XXXX"],
            "/home/rc/Downloads",
        );
        proc.link(10, 11);
        let argv = relaunch_argv(&client("foot", 10), &proc, &config()).unwrap();
        assert_eq!(argv, vec!["foot", "-D", "/home/rc/Downloads", "yazi"]);
    }

    #[test]
    fn terminal_falls_back_to_its_own_cwd_without_a_shell() {
        let mut proc = MockProcessInfo::default();
        proc.add(10, "foot", &["foot"], "/var/tmp");
        let argv = relaunch_argv(&client("foot", 10), &proc, &config()).unwrap();
        assert_eq!(argv, vec!["foot", "-D", "/var/tmp"]);
    }

    #[test]
    fn a_flattened_argv_is_split_back_apart() {
        // Chromium rewrites its argv into one blob; /bin/sh is a stand-in for
        // "the leading token really is an executable".
        let argv = vec!["/bin/sh --flag-one --flag-two=x,y".to_string()];
        assert_eq!(
            split_blob_cmdline(argv),
            vec!["/bin/sh", "--flag-one", "--flag-two=x,y"]
        );
    }

    #[test]
    fn a_path_with_spaces_is_left_alone() {
        // "/home/rc/my" is not an executable, so this is one real argument.
        let argv = vec!["/home/rc/my program".to_string()];
        assert_eq!(split_blob_cmdline(argv.clone()), argv);
    }

    #[test]
    fn a_normal_argv_is_untouched() {
        let argv = vec!["foot".to_string(), "-D".to_string(), "/tmp".to_string()];
        assert_eq!(split_blob_cmdline(argv.clone()), argv);
    }

    #[test]
    fn a_single_bare_binary_is_untouched() {
        let argv = vec!["/bin/sh".to_string()];
        assert_eq!(split_blob_cmdline(argv.clone()), argv);
    }

    #[test]
    fn a_dead_process_yields_no_argv() {
        let proc = MockProcessInfo::default();
        assert!(relaunch_argv(&client("google-chrome", 50), &proc, &config()).is_none());
    }

    #[test]
    fn app_overrides_replace_the_binary_and_strip_arguments() {
        let mut cfg = config();
        cfg.apps.insert(
            "signal".to_string(),
            AppConfig {
                binary: Some("signal-desktop".to_string()),
                no_spawn: false,
                strip_args: vec!["--start-in-tray".to_string()],
            },
        );
        let mut proc = MockProcessInfo::default();
        proc.add(
            70,
            "signal",
            &["/usr/lib/signal/signal", "--start-in-tray"],
            "/",
        );
        let argv = relaunch_argv(&client("signal", 70), &proc, &cfg).unwrap();
        assert_eq!(argv, vec!["signal-desktop"]);
    }

    #[test]
    fn capture_skips_ignored_and_unmapped_windows() {
        let mut bar = client("waybar", 20);
        bar.class = "waybar".to_string();
        let mut hidden = client("foot", 21);
        hidden.mapped = false;

        let mut proc = MockProcessInfo::default();
        proc.add(20, "waybar", &["waybar"], "/");
        proc.add(21, "foot", &["foot"], "/");
        proc.add(22, "foot", &["foot"], "/home/rc");

        let hypr = MockHyprctl::new(vec![vec![bar, hidden, client("foot", 22)]]);
        let session = capture_session("latest", &hypr, &proc, &config()).unwrap();
        assert_eq!(session.clients.len(), 1);
        assert_eq!(session.clients[0].launch.argv[0], "foot");
    }

    #[test]
    fn only_the_first_window_of_a_process_is_spawned() {
        let mut proc = MockProcessInfo::default();
        proc.add(90, "chrome", &["/opt/google/chrome/chrome"], "/home/rc");
        let hypr = MockHyprctl::new(vec![vec![
            client("google-chrome", 90),
            client("google-chrome", 90),
        ]]);
        let session = capture_session("latest", &hypr, &proc, &config()).unwrap();
        assert_eq!(session.clients.len(), 2);
        assert!(session.clients[0].launch.spawn);
        assert!(
            !session.clients[1].launch.spawn,
            "the second window comes back with the process, not from a new launch"
        );
        assert_eq!(session.spawn_count(), 1);
    }

    #[test]
    fn no_spawn_apps_are_recorded_but_never_launched() {
        let mut cfg = config();
        cfg.apps.insert(
            "Spotify".to_string(),
            AppConfig {
                binary: None,
                no_spawn: true,
                strip_args: vec![],
            },
        );
        let mut proc = MockProcessInfo::default();
        proc.add(80, "spotify", &["spotify"], "/");
        let hypr = MockHyprctl::new(vec![vec![client("Spotify", 80)]]);
        let session = capture_session("latest", &hypr, &proc, &cfg).unwrap();
        assert_eq!(session.clients.len(), 1);
        assert!(!session.clients[0].launch.spawn);
    }

    #[test]
    fn workspace_identity_survives_capture() {
        let mut scratch = client("foot", 30);
        scratch.workspace = WorkspaceRef::new(-99, "special:magic");
        let mut proc = MockProcessInfo::default();
        proc.add(30, "foot", &["foot"], "/home/rc");
        let hypr = MockHyprctl::new(vec![vec![scratch]]);
        let session = capture_session("latest", &hypr, &proc, &config()).unwrap();
        assert_eq!(
            session.clients[0].workspace.selector(),
            "special:magic",
            "scratchpad windows must not collapse to a numeric id"
        );
    }

    #[test]
    fn monitor_ids_resolve_to_names() {
        let mut proc = MockProcessInfo::default();
        proc.add(40, "foot", &["foot"], "/home/rc");
        let hypr = MockHyprctl::new(vec![vec![client("foot", 40)]]);
        let session = capture_session("latest", &hypr, &proc, &config()).unwrap();
        assert_eq!(session.clients[0].monitor, "eDP-1");
    }
}
