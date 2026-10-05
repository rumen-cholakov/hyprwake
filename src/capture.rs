//! Turning the live desktop into a [`Session`].

use crate::browsers;
use crate::config::{Config, TerminalConfig};
use crate::hyprctl::{HyprClient, HyprctlClient, HyprctlError};
use crate::process::ProcessInfoProvider;
use crate::provider::{CommandConfig, ProviderSet, ProviderSpec, Resolution};
use crate::session::{
    self, BrowserProfile, Identity, LaunchInfo, Monitor, Session, SessionClient, Workspace,
};
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

/// A session lookup runs on every save; it must not stall one.
const ID_COMMAND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

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

    let providers = ProviderSet::from_config(config);
    for problem in providers.problems() {
        crate::logging::log(format!("providers: {problem}"));
    }

    let browser_profiles = collect_browser_profiles(&raw_clients, &providers);
    // Classes whose windows the browser itself will reopen, one per profile.
    let profile_driven: HashSet<&str> = browser_profiles.iter().map(|p| p.class.as_str()).collect();

    let group_ids = group_indices(&raw_clients);

    let mut clients = Vec::new();
    let mut seen_pids = HashSet::new();

    for raw in &raw_clients {
        if !raw.is_restorable() || config.is_ignored(&raw.class) {
            continue;
        }
        let resolution = providers.resolve(&raw.class);
        let Some(argv) = launch_argv(raw, process, config, &resolution) else {
            // No argv means the process vanished between the query and the
            // read, or it is not ours to relaunch.
            continue;
        };

        let first_of_process = seen_pids.insert(raw.pid);
        let spawn =
            first_of_process && resolution.spawn && !profile_driven.contains(raw.class.as_str());

        clients.push(SessionClient {
            class: raw.class.clone(),
            initial_class: raw.initial_class.clone(),
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
            group: group_ids.get(&raw.address).copied(),
            // Populated in the ranked-matching phase; reserved here so the
            // schema bumps once for the whole release.
            identity: Identity::default(),
            launch: LaunchInfo {
                argv,
                spawn,
                provider: Some(resolution.key.to_string()),
                provider_state: None,
            },
        });
    }

    let monitors: Vec<Monitor> = raw_monitors
        .iter()
        .map(|m| Monitor {
            name: m.name.clone(),
            width: m.width,
            height: m.height,
            transform: m.transform,
            active_workspace: m.active_workspace.clone(),
            focused: m.focused,
            make: m.make.clone(),
            model: m.model.clone(),
            serial: m.serial.clone(),
            scale: m.scale,
            position: [m.x, m.y],
        })
        .collect();

    Ok(Session {
        schema_version: session::SCHEMA_VERSION,
        created_by: env!("CARGO_PKG_VERSION").to_string(),
        loaded_from: None,
        name: name.to_string(),
        created_at: Utc::now(),
        hyprland_version: version,
        workspaces: workspaces_of(&clients),
        monitors,
        clients,
        browser_profiles,
    })
}

/// One entry per workspace the snapshot actually contains, in first-seen
/// order.
///
/// The monitor comes from the windows on it rather than from `hyprctl
/// workspaces`, so a workspace with no windows produces no entry — there is
/// nothing to restore onto it.
fn workspaces_of(clients: &[SessionClient]) -> Vec<Workspace> {
    let mut seen: Vec<Workspace> = Vec::new();
    for client in clients {
        if seen
            .iter()
            .any(|w| w.workspace.selector() == client.workspace.selector())
        {
            continue;
        }
        seen.push(Workspace {
            workspace: client.workspace.clone(),
            monitor: client.monitor.clone(),
            layout: None,
            layout_kind: None,
            layout_confidence: None,
        });
    }
    seen
}

/// Assign a stable index to each window group, keyed by window address.
///
/// Every member of a group reports the same member list, so the sorted list
/// identifies the group. The indices are per-snapshot; they exist so members
/// can be recognised as belonging together, not to be matched across
/// sessions.
fn group_indices(clients: &[HyprClient]) -> HashMap<String, u32> {
    let mut keys: Vec<String> = Vec::new();
    let mut out = HashMap::new();
    for client in clients {
        if client.grouped.len() < 2 {
            // A group of one is Hyprland's representation of a window that
            // merely has a group bar; there is nothing to remember.
            continue;
        }
        let mut members = client.grouped.clone();
        members.sort();
        let key = members.join(",");
        let index = match keys.iter().position(|k| k == &key) {
            Some(i) => i,
            None => {
                keys.push(key);
                keys.len() - 1
            }
        };
        out.insert(client.address.clone(), index as u32);
    }
    out
}

/// The argv that recreates this window's process, or `None` when there is
/// nothing usable to replay.
pub fn relaunch_argv(
    client: &HyprClient,
    process: &dyn ProcessInfoProvider,
    config: &Config,
) -> Option<Vec<String>> {
    let providers = ProviderSet::from_config(config);
    launch_argv(client, process, config, &providers.resolve(&client.class))
}

/// Dispatch on the mechanism the class resolved to. Adding a provider kind
/// makes this `match` refuse to compile until the kind is handled.
fn launch_argv(
    client: &HyprClient,
    process: &dyn ProcessInfoProvider,
    config: &Config,
    resolution: &Resolution<'_>,
) -> Option<Vec<String>> {
    match resolution.spec {
        ProviderSpec::Terminal(term) => terminal_argv(client, process, config, term),
        ProviderSpec::Chromium(_) | ProviderSpec::Firefox(_) | ProviderSpec::Command(_) => {
            replayed_argv(client, process, &resolution.command())
        }
    }
}

/// A terminal reopened where it stood: running the TUI that was the reason it
/// was open, resumed where the program knows how, or else at its shell's
/// directory.
fn terminal_argv(
    client: &HyprClient,
    process: &dyn ProcessInfoProvider,
    config: &Config,
    term: &TerminalConfig,
) -> Option<Vec<String>> {
    let home = home_dir();

    if let Some(tui) =
        process.find_descendant(client.pid, &config.tui_programs(), PROCESS_SEARCH_DEPTH)
    {
        let argv = process
            .cmdline(tui)
            .or_else(|| process.comm(tui).map(|c| vec![c]))?;
        let mut argv: Vec<String> = argv
            .into_iter()
            .filter(|a| !VOLATILE_ARG_PREFIXES.iter().any(|p| a.starts_with(p)))
            .collect();
        let cwd = process
            .cwd(tui)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or(home);
        argv = with_resume_args(argv, tui, &cwd, process, config);
        return Some(term.build_argv(&cwd, Some(&argv)));
    }

    let shell = process.find_descendant(client.pid, &config.shells(), PROCESS_SEARCH_DEPTH);
    let cwd = shell
        .and_then(|pid| process.cwd(pid))
        .or_else(|| process.cwd(client.pid))
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or(home);
    Some(term.build_argv(&cwd, None))
}

/// The window's own argv, replayed, with the provider's overrides applied.
fn replayed_argv(
    client: &HyprClient,
    process: &dyn ProcessInfoProvider,
    command: &CommandConfig,
) -> Option<Vec<String>> {
    let mut argv = split_blob_cmdline(process.cmdline(client.pid)?);
    apply_overrides(&mut argv, command);
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
    let looks_executable = std::path::Path::new(first).is_file() || which::which(first).is_ok();
    if !looks_executable {
        return argv;
    }
    argv[0].split_whitespace().map(String::from).collect()
}

/// Ask a program to reopen the session it was in.
///
/// The session id is recovered from the program's own open files, so several
/// sessions of the same program in the same directory each come back as
/// themselves rather than all resuming the newest one.
fn with_resume_args(
    argv: Vec<String>,
    pid: i32,
    cwd: &str,
    process: &dyn ProcessInfoProvider,
    config: &Config,
) -> Vec<String> {
    let Some(program) = process.comm(pid) else {
        return argv;
    };
    let Some(rule) = config.tui.resume.get(&program) else {
        return argv;
    };

    // An id read from the program's own open files is exact; a lookup by
    // working directory is the fallback for programs that expose nothing.
    let from_fd = rule.fd_glob.as_ref().and_then(|glob| {
        let open = process.open_files(pid);
        crate::resume::find_id(glob, open.iter().map(|s| s.as_str()))
    });
    let id = from_fd.or_else(|| {
        if rule.id_command.is_empty() {
            return None;
        }
        let command = crate::resume::render_command(&rule.id_command, cwd, &home_dir())?;
        crate::resume::run_id_command(&command, ID_COMMAND_TIMEOUT)
    });

    let extra = match id {
        Some(id) => crate::resume::render_args(&rule.args, &id),
        None => rule.fallback.clone(),
    };
    if extra.is_empty() {
        return argv;
    }

    let mut argv = strip_flags(argv, &rule.strip_flags);
    argv.extend(extra);
    argv
}

/// Drop `flags` from an argv, along with a value following one of them.
fn strip_flags(argv: Vec<String>, flags: &[String]) -> Vec<String> {
    if flags.is_empty() || argv.is_empty() {
        return argv;
    }
    let mut out = vec![argv[0].clone()];
    let mut i = 1;
    while i < argv.len() {
        if flags.contains(&argv[i]) {
            i += 1;
            if i < argv.len() && !argv[i].starts_with('-') {
                i += 1; // the flag's value
            }
            continue;
        }
        out.push(argv[i].clone());
        i += 1;
    }
    out
}

fn apply_overrides(argv: &mut Vec<String>, app: &CommandConfig) {
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

/// Profile windows for every browser provider that has a window open.
///
/// Read once per provider, not once per class: a provider claiming both
/// `zen` and `zen-alpha` reads one `profiles.ini`, and reading it twice would
/// reopen every profile twice.
fn collect_browser_profiles(
    clients: &[HyprClient],
    providers: &ProviderSet,
) -> Vec<BrowserProfile> {
    let mut open: Vec<&str> = clients
        .iter()
        .filter(|c| c.is_restorable())
        .map(|c| c.class.as_str())
        .collect();
    open.sort();
    open.dedup();

    let mut read: HashSet<&str> = HashSet::new();
    let mut out = Vec::new();
    for class in open {
        let resolution = providers.resolve(class);
        let Some(browser) = resolution.spec.browser() else {
            continue;
        };
        if browser.profile_workspaces.is_empty() || !read.insert(resolution.key) {
            continue;
        }
        match browsers::read_profiles(class, &browser) {
            Ok(profiles) => out.extend(browsers::assign_workspaces(profiles, &browser)),
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
    use crate::config::AppConfig;
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
            grouped: vec![],
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
            "/home/user",
        );
        let argv = relaunch_argv(&client("google-chrome", 50), &proc, &config()).unwrap();
        assert_eq!(argv[0], "/opt/google/chrome/chrome");
        assert_eq!(argv[1], "--ozone-platform=wayland");
    }

    #[test]
    fn terminal_reopens_at_its_shell_directory() {
        let mut proc = MockProcessInfo::default();
        proc.add(10, "foot", &["foot"], "/home/user");
        proc.add(11, "fish", &["fish"], "/home/user/Work/hyprwake");
        proc.link(10, 11);
        let argv = relaunch_argv(&client("foot", 10), &proc, &config()).unwrap();
        assert_eq!(argv, vec!["foot", "-D", "/home/user/Work/hyprwake"]);
    }

    #[test]
    fn terminal_running_a_tui_reopens_the_tui() {
        let mut proc = MockProcessInfo::default();
        proc.add(10, "foot", &["foot"], "/home/user");
        proc.add(11, "fish", &["fish"], "/home/user");
        proc.add(12, "nvim", &["nvim", "src/main.rs"], "/home/user/Work");
        proc.link(10, 11).link(11, 12);
        let argv = relaunch_argv(&client("foot", 10), &proc, &config()).unwrap();
        assert_eq!(
            argv,
            vec!["foot", "-D", "/home/user/Work", "nvim", "src/main.rs"]
        );
    }

    #[test]
    fn a_resumable_tui_comes_back_on_its_own_session() {
        let mut proc = MockProcessInfo::default();
        proc.add(10, "foot", &["foot"], "/home/user");
        proc.add(11, "claude", &["claude"], "/home/user/Work");
        proc.link(10, 11);
        // The session id lives in the path of a file the program holds open.
        proc.open(
            11,
            &[
                "/proc/11/statm",
                "/tmp/claude-1000/-home-user-Work/0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0/tasks",
            ],
        );
        let argv = relaunch_argv(&client("foot", 10), &proc, &config()).unwrap();
        assert_eq!(
            argv,
            vec![
                "foot",
                "-D",
                "/home/user/Work",
                "claude",
                "--resume",
                "0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0"
            ]
        );
    }

    #[test]
    fn an_unidentifiable_session_falls_back_to_continue() {
        let mut proc = MockProcessInfo::default();
        proc.add(10, "foot", &["foot"], "/home/user");
        proc.add(11, "claude", &["claude"], "/home/user/Work");
        proc.link(10, 11);
        proc.open(11, &["/dev/null"]);
        let argv = relaunch_argv(&client("foot", 10), &proc, &config()).unwrap();
        assert_eq!(argv.last().unwrap(), "--continue");
    }

    #[test]
    fn a_session_already_started_with_a_resume_flag_is_not_resumed_twice() {
        let mut proc = MockProcessInfo::default();
        proc.add(10, "foot", &["foot"], "/home/user");
        proc.add(
            11,
            "claude",
            &["claude", "--resume", "old-id", "--effort", "high"],
            "/home/user",
        );
        proc.link(10, 11);
        proc.open(11, &["/tmp/claude-1000/-home-user/new-id/tasks"]);
        let argv = relaunch_argv(&client("foot", 10), &proc, &config()).unwrap();
        assert_eq!(
            argv,
            vec![
                "foot",
                "-D",
                "/home/user",
                "claude",
                "--effort",
                "high",
                "--resume",
                "new-id"
            ],
            "the stale id must be replaced, and unrelated flags kept"
        );
    }

    #[test]
    fn a_lookup_based_resume_uses_the_terminals_directory() {
        let mut cfg = config();
        cfg.tui.resume.insert(
            "myagent".to_string(),
            crate::config::ResumeConfig {
                fd_glob: None,
                // Only prints an id when {cwd} arrived as the terminal's
                // directory, so this proves the substitution too.
                id_command: vec![
                    "sh".to_string(),
                    "-c".to_string(),
                    "[ '{cwd}' = /tmp/x ] && printf %s abc-123".to_string(),
                ],
                args: vec!["resume".to_string(), "{id}".to_string()],
                fallback: vec!["resume".to_string(), "--last".to_string()],
                strip_flags: vec![],
            },
        );
        cfg.tui.programs.push("myagent".to_string());

        let mut proc = MockProcessInfo::default();
        proc.add(10, "foot", &["foot"], "/home/user");
        proc.add(11, "myagent", &["myagent"], "/tmp/x");
        proc.link(10, 11);

        let argv = relaunch_argv(&client("foot", 10), &proc, &cfg).unwrap();
        assert_eq!(
            argv,
            vec!["foot", "-D", "/tmp/x", "myagent", "resume", "abc-123"]
        );
    }

    #[test]
    fn a_lookup_that_finds_nothing_falls_back() {
        let mut cfg = config();
        cfg.tui.resume.insert(
            "myagent".to_string(),
            crate::config::ResumeConfig {
                fd_glob: None,
                id_command: vec!["true".to_string()],
                args: vec!["resume".to_string(), "{id}".to_string()],
                fallback: vec!["resume".to_string(), "--last".to_string()],
                strip_flags: vec![],
            },
        );
        cfg.tui.programs.push("myagent".to_string());

        let mut proc = MockProcessInfo::default();
        proc.add(10, "foot", &["foot"], "/home/user");
        proc.add(11, "myagent", &["myagent"], "/tmp/x");
        proc.link(10, 11);

        let argv = relaunch_argv(&client("foot", 10), &proc, &cfg).unwrap();
        assert_eq!(argv.last().unwrap(), "--last");
    }

    #[test]
    fn a_tui_without_a_resume_rule_is_untouched() {
        let mut proc = MockProcessInfo::default();
        proc.add(10, "foot", &["foot"], "/home/user");
        proc.add(11, "btop", &["btop"], "/home/user");
        proc.link(10, 11);
        proc.open(11, &["/tmp/claude-1000/x/some-id/tasks"]);
        let argv = relaunch_argv(&client("foot", 10), &proc, &config()).unwrap();
        assert_eq!(argv, vec!["foot", "-D", "/home/user", "btop"]);
    }

    #[test]
    fn strip_flags_keeps_the_binary_and_unrelated_arguments() {
        let argv: Vec<String> = ["claude", "--resume", "id", "-c", "--effort", "high"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let flags: Vec<String> = ["--resume", "-c"].iter().map(|s| s.to_string()).collect();
        assert_eq!(
            strip_flags(argv, &flags),
            vec!["claude", "--effort", "high"]
        );
    }

    #[test]
    fn volatile_tui_arguments_are_dropped() {
        let mut proc = MockProcessInfo::default();
        proc.add(10, "foot", &["foot"], "/home/user");
        proc.add(
            11,
            "yazi",
            &["yazi", "--cwd-file=/run/user/1000/yazi-cwd.XXXX"],
            "/home/user/Downloads",
        );
        proc.link(10, 11);
        let argv = relaunch_argv(&client("foot", 10), &proc, &config()).unwrap();
        assert_eq!(argv, vec!["foot", "-D", "/home/user/Downloads", "yazi"]);
    }

    #[test]
    fn terminal_falls_back_to_its_own_cwd_without_a_shell() {
        let mut proc = MockProcessInfo::default();
        proc.add(10, "foot", &["foot"], "/var/tmp");
        let argv = relaunch_argv(&client("foot", 10), &proc, &config()).unwrap();
        assert_eq!(argv, vec!["foot", "-D", "/var/tmp"]);
    }

    #[test]
    fn group_membership_is_recorded() {
        let mut a = client("foot", 1);
        let mut b = client("foot", 2);
        a.address = "0xa".into();
        b.address = "0xb".into();
        a.grouped = vec!["0xa".into(), "0xb".into()];
        b.grouped = vec!["0xb".into(), "0xa".into()]; // order differs per member
        let mut proc = MockProcessInfo::default();
        proc.add(1, "foot", &["foot"], "/home/user");
        proc.add(2, "foot", &["foot"], "/home/user");

        let session = capture_session(
            "latest",
            &MockHyprctl::new(vec![vec![a, b]]),
            &proc,
            &config(),
        )
        .unwrap();
        assert_eq!(session.clients[0].group, Some(0));
        assert_eq!(
            session.clients[1].group,
            Some(0),
            "both members belong to the same group regardless of listing order"
        );
    }

    #[test]
    fn separate_groups_get_separate_indices() {
        let mut a = client("foot", 1);
        let mut b = client("foot", 2);
        let mut c = client("foot", 3);
        let mut d = client("foot", 4);
        for (w, addr) in [
            (&mut a, "0xa"),
            (&mut b, "0xb"),
            (&mut c, "0xc"),
            (&mut d, "0xd"),
        ] {
            w.address = addr.into();
        }
        a.grouped = vec!["0xa".into(), "0xb".into()];
        b.grouped = vec!["0xa".into(), "0xb".into()];
        c.grouped = vec!["0xc".into(), "0xd".into()];
        d.grouped = vec!["0xc".into(), "0xd".into()];
        let mut proc = MockProcessInfo::default();
        for pid in 1..=4 {
            proc.add(pid, "foot", &["foot"], "/home/user");
        }
        let session = capture_session(
            "latest",
            &MockHyprctl::new(vec![vec![a, b, c, d]]),
            &proc,
            &config(),
        )
        .unwrap();
        let groups: Vec<_> = session.clients.iter().map(|c| c.group).collect();
        assert_eq!(groups, vec![Some(0), Some(0), Some(1), Some(1)]);
    }

    #[test]
    fn a_lone_window_with_a_group_bar_is_not_a_group() {
        // Hyprland reports a single-member "group" for a window that merely
        // carries a group bar; there is nothing to remember about that.
        let mut a = client("foot", 1);
        a.address = "0xa".into();
        a.grouped = vec!["0xa".into()];
        let mut proc = MockProcessInfo::default();
        proc.add(1, "foot", &["foot"], "/home/user");
        let session =
            capture_session("latest", &MockHyprctl::new(vec![vec![a]]), &proc, &config()).unwrap();
        assert_eq!(session.clients[0].group, None);
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
        // "/home/user/my" is not an executable, so this is one real argument.
        let argv = vec!["/home/user/my program".to_string()];
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
        proc.add(22, "foot", &["foot"], "/home/user");

        let hypr = MockHyprctl::new(vec![vec![bar, hidden, client("foot", 22)]]);
        let session = capture_session("latest", &hypr, &proc, &config()).unwrap();
        assert_eq!(session.clients.len(), 1);
        assert_eq!(session.clients[0].launch.argv[0], "foot");
    }

    #[test]
    fn only_the_first_window_of_a_process_is_spawned() {
        let mut proc = MockProcessInfo::default();
        proc.add(90, "chrome", &["/opt/google/chrome/chrome"], "/home/user");
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
        proc.add(30, "foot", &["foot"], "/home/user");
        let hypr = MockHyprctl::new(vec![vec![scratch]]);
        let session = capture_session("latest", &hypr, &proc, &config()).unwrap();
        assert_eq!(
            session.clients[0].workspace.selector(),
            "special:magic",
            "scratchpad windows must not collapse to a numeric id"
        );
    }

    // ── golden: argv byte-identical to 0.1.3 ──
    //
    // Every built-in capture path, recorded against the 0.1.3 implementation
    // before the provider refactor. A change here is a change to what restore
    // launches, so it must be deliberate: edit the expectation *and* say why
    // in the case's note.

    struct Golden {
        note: &'static str,
        class: &'static str,
        /// (pid, comm, argv, cwd); the first entry is the window's own process
        /// and each later one is a child of the entry before it.
        chain: Vec<(i32, &'static str, Vec<&'static str>, &'static str)>,
        open: Vec<&'static str>,
        tweak: fn(&mut Config),
        expect: Option<Vec<String>>,
    }

    fn v(parts: &[&str]) -> Option<Vec<String>> {
        Some(parts.iter().map(|s| s.to_string()).collect())
    }

    fn untouched(_: &mut Config) {}

    const CLAUDE_FD: &str = "/tmp/claude-1000/-w/0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0/tasks";
    const CLAUDE_ID: &str = "0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0";

    fn golden_cases() -> Vec<Golden> {
        vec![
            // ── every built-in terminal at its shell's directory ──
            Golden {
                note: "foot takes the directory as a separate argument",
                class: "foot",
                chain: vec![
                    (10, "foot", vec!["foot"], "/"),
                    (11, "fish", vec!["fish"], "/w"),
                ],
                open: vec![],
                tweak: untouched,
                expect: v(&["foot", "-D", "/w"]),
            },
            Golden {
                note: "omarchy's terminal keeps its app-id",
                class: "org.omarchy.terminal",
                chain: vec![
                    (10, "foot", vec!["foot"], "/"),
                    (11, "bash", vec!["bash"], "/w"),
                ],
                open: vec![],
                tweak: untouched,
                expect: v(&["foot", "-D", "/w", "--app-id=org.omarchy.terminal"]),
            },
            Golden {
                note: "kitty's directory flag is lower-case",
                class: "kitty",
                chain: vec![
                    (10, "kitty", vec!["kitty"], "/"),
                    (11, "zsh", vec!["zsh"], "/w"),
                ],
                open: vec![],
                tweak: untouched,
                expect: v(&["kitty", "-d", "/w"]),
            },
            Golden {
                note: "alacritty's class is capitalised, its binary is not",
                class: "Alacritty",
                chain: vec![
                    (10, "alacritty", vec!["alacritty"], "/"),
                    (11, "fish", vec!["fish"], "/w"),
                ],
                open: vec![],
                tweak: untouched,
                expect: v(&["alacritty", "--working-directory", "/w"]),
            },
            Golden {
                note: "ghostty's flag is joined to its value",
                class: "com.mitchellh.ghostty",
                chain: vec![
                    (10, "ghostty", vec!["ghostty"], "/"),
                    (11, "fish", vec!["fish"], "/w"),
                ],
                open: vec![],
                tweak: untouched,
                expect: v(&["ghostty", "--working-directory=/w"]),
            },
            // ── a TUI inside each exec-flag variant ──
            Golden {
                note: "foot takes the program positionally",
                class: "foot",
                chain: vec![
                    (10, "foot", vec!["foot"], "/"),
                    (11, "fish", vec!["fish"], "/"),
                    (12, "nvim", vec!["nvim", "a.rs"], "/w"),
                ],
                open: vec![],
                tweak: untouched,
                expect: v(&["foot", "-D", "/w", "nvim", "a.rs"]),
            },
            Golden {
                note: "extra_args come before the program",
                class: "org.omarchy.terminal",
                chain: vec![
                    (10, "foot", vec!["foot"], "/"),
                    (11, "nvim", vec!["nvim", "a.rs"], "/w"),
                ],
                open: vec![],
                tweak: untouched,
                expect: v(&[
                    "foot",
                    "-D",
                    "/w",
                    "--app-id=org.omarchy.terminal",
                    "nvim",
                    "a.rs",
                ]),
            },
            Golden {
                note: "alacritty needs -e before the program",
                class: "Alacritty",
                chain: vec![
                    (10, "alacritty", vec!["alacritty"], "/"),
                    (11, "nvim", vec!["nvim", "a.rs"], "/w"),
                ],
                open: vec![],
                tweak: untouched,
                expect: v(&[
                    "alacritty",
                    "--working-directory",
                    "/w",
                    "-e",
                    "nvim",
                    "a.rs",
                ]),
            },
            Golden {
                note: "ghostty: joined cwd flag and -e",
                class: "com.mitchellh.ghostty",
                chain: vec![
                    (10, "ghostty", vec!["ghostty"], "/"),
                    (11, "yazi", vec!["yazi"], "/w"),
                ],
                open: vec![],
                tweak: untouched,
                expect: v(&["ghostty", "--working-directory=/w", "-e", "yazi"]),
            },
            Golden {
                note: "the TUI wins over the shell it runs under",
                class: "kitty",
                chain: vec![
                    (10, "kitty", vec!["kitty"], "/"),
                    (11, "zsh", vec!["zsh"], "/shell"),
                    (12, "btop", vec!["btop"], "/tui"),
                ],
                open: vec![],
                tweak: untouched,
                expect: v(&["kitty", "-d", "/tui", "btop"]),
            },
            Golden {
                note: "a TUI whose cmdline is unreadable is relaunched by comm",
                class: "foot",
                chain: vec![(10, "foot", vec!["foot"], "/"), (11, "htop", vec![], "/w")],
                open: vec![],
                tweak: untouched,
                // An empty cmdline is Some(vec![]), not None: the program is
                // relaunched with no arguments at all, not even its name.
                expect: v(&["foot", "-D", "/w"]),
            },
            Golden {
                note: "volatile temp-file arguments are dropped",
                class: "foot",
                chain: vec![
                    (10, "foot", vec!["foot"], "/"),
                    (
                        11,
                        "yazi",
                        vec![
                            "yazi",
                            "--cwd-file=/run/user/1000/x",
                            "--chooser-file=/tmp/y",
                        ],
                        "/w",
                    ),
                ],
                open: vec![],
                tweak: untouched,
                expect: v(&["foot", "-D", "/w", "yazi"]),
            },
            // ── resume ──
            Golden {
                note: "claude resumes the exact session from its open file",
                class: "kitty",
                chain: vec![
                    (10, "kitty", vec!["kitty"], "/"),
                    (11, "claude", vec!["claude"], "/w"),
                ],
                open: vec![CLAUDE_FD],
                tweak: untouched,
                expect: v(&["kitty", "-d", "/w", "claude", "--resume", CLAUDE_ID]),
            },
            Golden {
                note: "claude falls back to --continue with no open session file",
                class: "foot",
                chain: vec![
                    (10, "foot", vec!["foot"], "/"),
                    (11, "claude", vec!["claude"], "/w"),
                ],
                open: vec![],
                tweak: untouched,
                expect: v(&["foot", "-D", "/w", "claude", "--continue"]),
            },
            Golden {
                note: "a stale resume flag and its value are replaced, others kept",
                class: "foot",
                chain: vec![
                    (10, "foot", vec!["foot"], "/"),
                    (
                        11,
                        "claude",
                        vec!["claude", "--resume", "old", "-c", "--model", "opus"],
                        "/w",
                    ),
                ],
                open: vec![CLAUDE_FD],
                tweak: untouched,
                expect: v(&[
                    "foot", "-D", "/w", "claude", "--model", "opus", "--resume", CLAUDE_ID,
                ]),
            },
            Golden {
                note: "an id_command rule substitutes the TUI's directory",
                class: "foot",
                chain: vec![
                    (10, "foot", vec!["foot"], "/"),
                    (11, "myagent", vec!["myagent"], "/tmp/x"),
                ],
                open: vec![],
                tweak: |c| {
                    c.tui.programs.push("myagent".to_string());
                    c.tui.resume.insert(
                        "myagent".to_string(),
                        crate::config::ResumeConfig {
                            fd_glob: None,
                            id_command: vec![
                                "sh".to_string(),
                                "-c".to_string(),
                                "[ '{cwd}' = /tmp/x ] && printf %s abc-123".to_string(),
                            ],
                            args: vec!["--session".to_string(), "{id}".to_string()],
                            fallback: vec![],
                            strip_flags: vec![],
                        },
                    );
                },
                expect: v(&["foot", "-D", "/tmp/x", "myagent", "--session", "abc-123"]),
            },
            // ── no shell, no TUI ──
            Golden {
                note: "a terminal with no shell reopens at its own directory",
                class: "foot",
                chain: vec![(10, "foot", vec!["foot"], "/var/tmp")],
                open: vec![],
                tweak: untouched,
                expect: v(&["foot", "-D", "/var/tmp"]),
            },
            Golden {
                note: "a TUI nested beyond the search depth is not found",
                class: "foot",
                chain: vec![
                    (10, "foot", vec!["foot"], "/"),
                    (11, "fish", vec!["fish"], "/shell"),
                    (12, "a", vec!["a"], "/"),
                    (13, "b", vec!["b"], "/"),
                    (14, "c", vec!["c"], "/"),
                    (15, "d", vec!["d"], "/"),
                    (16, "nvim", vec!["nvim"], "/deep"),
                ],
                open: vec![],
                tweak: untouched,
                expect: v(&["foot", "-D", "/shell"]),
            },
            // ── the session-manager family, recorded as it stands ──
            Golden {
                note: "herdr is treated as a TUI; its --session is replayed verbatim",
                class: "foot",
                chain: vec![
                    (10, "foot", vec!["foot"], "/"),
                    (11, "herdr", vec!["herdr", "--session", "backend"], "/w"),
                ],
                open: vec![],
                tweak: untouched,
                expect: v(&["foot", "-D", "/w", "herdr", "--session", "backend"]),
            },
            // ── generic applications ──
            Golden {
                note: "a plain app replays its own argv",
                class: "obsidian",
                chain: vec![(
                    50,
                    "obsidian",
                    vec!["/usr/bin/obsidian", "--enable-wayland-ime"],
                    "/",
                )],
                open: vec![],
                tweak: untouched,
                expect: v(&["/usr/bin/obsidian", "--enable-wayland-ime"]),
            },
            Golden {
                note: "a flattened chromium-style argv is split back apart",
                class: "google-chrome",
                chain: vec![(
                    50,
                    "chrome",
                    vec!["/bin/sh --ozone-platform=wayland --x=a,b"],
                    "/",
                )],
                open: vec![],
                tweak: untouched,
                expect: v(&["/bin/sh", "--ozone-platform=wayland", "--x=a,b"]),
            },
            Golden {
                note: "a class claimed by no terminal is generic even if it hosts a TUI",
                class: "code",
                chain: vec![
                    (50, "code", vec!["/usr/share/code/code"], "/"),
                    (51, "nvim", vec!["nvim"], "/w"),
                ],
                open: vec![],
                tweak: untouched,
                expect: v(&["/usr/share/code/code"]),
            },
            Golden {
                note: "an empty argv yields nothing to replay",
                class: "obsidian",
                chain: vec![(50, "obsidian", vec![], "/")],
                open: vec![],
                tweak: untouched,
                expect: None,
            },
            // ── [apps] overrides ──
            Golden {
                note: "apps: binary replaced, arguments stripped by prefix",
                class: "signal",
                chain: vec![(
                    70,
                    "signal",
                    vec!["/usr/lib/signal/signal", "--start-in-tray", "--keep"],
                    "/",
                )],
                open: vec![],
                tweak: |c| {
                    c.apps.insert(
                        "signal".to_string(),
                        AppConfig {
                            binary: Some("signal-desktop".to_string()),
                            no_spawn: false,
                            strip_args: vec!["--start-in".to_string()],
                        },
                    );
                },
                expect: v(&["signal-desktop", "--keep"]),
            },
            Golden {
                note: "apps: a binary override rescues an empty argv",
                class: "signal",
                chain: vec![(70, "signal", vec![], "/")],
                open: vec![],
                tweak: |c| {
                    c.apps.insert(
                        "signal".to_string(),
                        AppConfig {
                            binary: Some("signal-desktop".to_string()),
                            no_spawn: false,
                            strip_args: vec![],
                        },
                    );
                },
                expect: v(&["signal-desktop"]),
            },
            Golden {
                note: "apps: overrides have no effect on a terminal class",
                class: "foot",
                chain: vec![(10, "foot", vec!["foot"], "/w")],
                open: vec![],
                tweak: |c| {
                    c.apps.insert(
                        "foot".to_string(),
                        AppConfig {
                            binary: Some("footclient".to_string()),
                            no_spawn: false,
                            strip_args: vec!["-D".to_string()],
                        },
                    );
                },
                expect: v(&["foot", "-D", "/w"]),
            },
            // ── a user [terminals] table replaces the built-ins ──
            Golden {
                note: "a replaced terminals table leaves kitty generic",
                class: "kitty",
                chain: vec![(10, "kitty", vec!["kitty", "--single-instance"], "/w")],
                open: vec![],
                tweak: |c| {
                    c.terminals.retain(|k, _| k == "foot");
                },
                expect: v(&["kitty", "--single-instance"]),
            },
        ]
    }

    #[test]
    fn golden_capture_argv_is_unchanged_from_0_1_3() {
        let mut failures = Vec::new();
        for case in golden_cases() {
            let mut proc = MockProcessInfo::default();
            for (i, (pid, comm, argv, cwd)) in case.chain.iter().enumerate() {
                proc.add(*pid, comm, argv, cwd);
                if i > 0 {
                    proc.link(case.chain[i - 1].0, *pid);
                }
            }
            let tui_pid = case.chain.last().unwrap().0;
            if !case.open.is_empty() {
                proc.open(tui_pid, &case.open);
            }
            let mut cfg = config();
            (case.tweak)(&mut cfg);
            let got = relaunch_argv(&client(case.class, case.chain[0].0), &proc, &cfg);
            if got != case.expect {
                failures.push(format!(
                    "{} [{}]\n    expected {:?}\n    got      {:?}",
                    case.note, case.class, case.expect, got
                ));
            }
        }
        assert!(
            failures.is_empty(),
            "capture argv drifted from 0.1.3:\n  {}",
            failures.join("\n  ")
        );
    }

    // ── providers ──

    #[test]
    fn each_launch_records_the_provider_that_captured_it() {
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
        proc.add(10, "foot", &["foot"], "/w");
        proc.add(20, "obsidian", &["obsidian"], "/");
        proc.add(30, "spotify", &["spotify"], "/");
        let hypr = MockHyprctl::new(vec![vec![
            client("foot", 10),
            client("obsidian", 20),
            client("Spotify", 30),
        ]]);
        let session = capture_session("latest", &hypr, &proc, &cfg).unwrap();
        let providers: Vec<_> = session
            .clients
            .iter()
            .map(|c| c.launch.provider.as_deref())
            .collect();
        assert_eq!(
            providers,
            vec![Some("foot"), Some("generic"), Some("Spotify")]
        );
        assert!(!session.clients[2].launch.spawn, "no_spawn still applies");
    }

    #[test]
    fn a_new_form_terminal_is_captured_like_a_builtin_one() {
        let cfg: Config = toml::from_str(
            r#"
[providers.wezterm]
kind = "terminal"
classes = ["org.wezfurlong.wezterm"]
binary = "wezterm"
cwd_flag = "--cwd"
exec_flag = "-e"
"#,
        )
        .unwrap();
        let mut proc = MockProcessInfo::default();
        proc.add(10, "wezterm-gui", &["wezterm-gui"], "/");
        proc.add(11, "nvim", &["nvim", "a.rs"], "/w");
        proc.link(10, 11);
        let argv = relaunch_argv(&client("org.wezfurlong.wezterm", 10), &proc, &cfg).unwrap();
        assert_eq!(argv, vec!["wezterm", "--cwd", "/w", "-e", "nvim", "a.rs"]);
        // The built-ins are still there alongside it.
        proc.add(20, "foot", &["foot"], "/x");
        assert_eq!(
            relaunch_argv(&client("foot", 20), &proc, &cfg).unwrap(),
            vec!["foot", "-D", "/x"]
        );
    }

    #[test]
    fn a_new_form_command_pattern_applies_its_overrides() {
        let cfg: Config = toml::from_str(
            r#"
[providers.jetbrains]
kind = "command"
classes = ["jetbrains-*"]
strip_args = ["-Dtmp.session"]
"#,
        )
        .unwrap();
        let mut proc = MockProcessInfo::default();
        proc.add(
            50,
            "java",
            &["/opt/idea/bin/idea", "-Dtmp.session=/tmp/x", "/w/proj"],
            "/",
        );
        let argv = relaunch_argv(&client("jetbrains-idea", 50), &proc, &cfg).unwrap();
        assert_eq!(argv, vec!["/opt/idea/bin/idea", "/w/proj"]);
    }

    #[test]
    fn monitor_ids_resolve_to_names() {
        let mut proc = MockProcessInfo::default();
        proc.add(40, "foot", &["foot"], "/home/user");
        let hypr = MockHyprctl::new(vec![vec![client("foot", 40)]]);
        let session = capture_session("latest", &hypr, &proc, &config()).unwrap();
        assert_eq!(session.clients[0].monitor, "eDP-1");
    }
}
