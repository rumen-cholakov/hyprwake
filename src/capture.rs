use crate::config::{AppConfig, Config};
use crate::hyprctl::{HyprctlClient, HyprctlError};
use crate::process::{ProcessInfoProvider, ProcessError};
use crate::session::{LaunchInfo, Monitor, Session, SessionClient};
use chrono::Utc;

// ── Error ──────────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error("hyprctl error: {0}")]
    Hyprctl(#[from] HyprctlError),
    #[error("process error: {0}")]
    Process(#[from] ProcessError),
}

// ── Public API ─────────────────────────────────────────────────────────────

/// Capture the current Hyprland session state into a [`Session`].
///
/// All windows whose class appears in `config.filters.ignore_classes` are
/// excluded from the returned session.
pub fn capture_session(
    name: &str,
    hyprctl: &dyn HyprctlClient,
    process_info: &dyn ProcessInfoProvider,
    config: &Config,
) -> Result<Session, CaptureError> {
    let raw_clients = hyprctl.get_clients()?;
    let raw_monitors = hyprctl.get_monitors()?;
    let version = hyprctl
        .get_hyprland_version()
        .unwrap_or_else(|_| "unknown".to_string());

    // Build an index from monitor position to monitor name so that
    // HyprClient.monitor (an i32 index) can be resolved to a human-readable
    // name such as "DP-1".
    let monitor_names: Vec<String> = raw_monitors.iter().map(|m| m.name.clone()).collect();

    let monitors: Vec<Monitor> = raw_monitors
        .iter()
        .map(|m| Monitor {
            name: m.name.clone(),
            width: m.width,
            height: m.height,
            transform: m.transform,
        })
        .collect();

    let clients: Vec<SessionClient> = raw_clients
        .iter()
        .filter(|c| !config.filters.ignore_classes.contains(&c.class))
        .map(|c| build_session_client(c, &monitor_names, process_info, config))
        .collect();

    Ok(Session {
        name: name.to_string(),
        created_at: Utc::now(),
        hyprland_version: version,
        monitors,
        clients,
    })
}

// ── Private helpers ────────────────────────────────────────────────────────

fn build_session_client(
    client: &crate::hyprctl::HyprClient,
    monitor_names: &[String],
    process_info: &dyn ProcessInfoProvider,
    config: &Config,
) -> SessionClient {
    let monitor_name = monitor_names
        .get(client.monitor as usize)
        .cloned()
        .unwrap_or_else(|| format!("monitor-{}", client.monitor));

    let app_config = config.apps.get(&client.class);
    let launch = build_launch_info(client, app_config, process_info);

    SessionClient {
        class: client.class.clone(),
        title: client.title.clone(),
        workspace: client.workspace.id,
        monitor: monitor_name,
        at: client.at,
        size: client.size,
        floating: client.floating,
        fullscreen: client.fullscreen,
        focus_history_id: client.focus_history_id,
        launch,
    }
}

fn build_launch_info(
    client: &crate::hyprctl::HyprClient,
    app_config: Option<&AppConfig>,
    process_info: &dyn ProcessInfoProvider,
) -> LaunchInfo {
    let binary = app_config
        .and_then(|a| a.binary.clone())
        .unwrap_or_else(|| client.class.clone());

    let capture_cwd = app_config.and_then(|a| a.capture_cwd).unwrap_or(false);
    let capture_cmd = app_config
        .and_then(|a| a.capture_last_command)
        .unwrap_or(false);

    let mut args: Vec<String> = Vec::new();
    let mut hint: Option<String> = None;

    if capture_cwd || capture_cmd {
        if let Ok(children) = process_info.get_children(client.pid) {
            // Find the actual shell child, skipping helper processes like
            // kitty's "kitten __atexit__" which has CWD=/home but is not the
            // interactive shell.
            const SKIP_COMMANDS: &[&str] = &["kitten", "/usr/bin/kitten"];
            if let Some(shell) = children
                .iter()
                .filter(|c| !c.cwd.as_os_str().is_empty())
                .find(|c| !SKIP_COMMANDS.iter().any(|s| c.cmdline.starts_with(s)))
            {
                if capture_cwd {
                    args.push("--directory".to_string());
                    args.push(shell.cwd.to_string_lossy().to_string());
                }

                if capture_cmd {
                    // Prefer a grandchild process (the command running inside the shell).
                    if let Ok(grandchildren) = process_info.get_children(shell.pid) {
                        if let Some(cmd) =
                            grandchildren.iter().find(|gc| !gc.cmdline.is_empty())
                        {
                            hint = Some(cmd.cmdline.clone());
                        }
                    }

                    // Fall back to the shell's own cmdline if it is not a plain shell.
                    if hint.is_none() && !shell.cmdline.is_empty() {
                        const PLAIN_SHELLS: &[&str] = &["zsh", "bash", "fish", "sh"];
                        if !PLAIN_SHELLS.contains(&shell.cmdline.as_str()) {
                            hint = Some(shell.cmdline.clone());
                        }
                    }
                }
            }
        }
    }

    // Render hint through the app-level template when one is configured.
    if let (Some(h), Some(ac)) = (&hint, app_config) {
        if let Some(template) = &ac.hint_template {
            let cwd_str = args
                .iter()
                .skip_while(|s| s.as_str() != "--directory")
                .nth(1)
                .map(|s| s.as_str())
                .unwrap_or("");
            hint = Some(
                template
                    .replace("{last_command}", h)
                    .replace("{cwd}", cwd_str),
            );
        }
    }

    LaunchInfo {
        command: binary,
        args,
        hint,
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AppConfig, Config, FilterConfig, GeneralConfig};
    use crate::hyprctl::{
        HyprClient as RawClient, HyprMonitor as RawMonitor, HyprWorkspace as RawWorkspace,
        HyprctlClient, HyprctlError,
    };
    use crate::process::{ChildProcess, ProcessError, ProcessInfoProvider};
    use std::collections::HashMap;
    use std::path::PathBuf;

    // ── Mock: HyprctlClient ──────────────────────────────────────────────

    struct MockHyprctl {
        clients: Vec<RawClient>,
        monitors: Vec<RawMonitor>,
    }

    impl HyprctlClient for MockHyprctl {
        fn get_clients(&self) -> Result<Vec<RawClient>, HyprctlError> {
            Ok(self.clients.clone())
        }
        fn get_monitors(&self) -> Result<Vec<RawMonitor>, HyprctlError> {
            Ok(self.monitors.clone())
        }
        fn dispatch(&self, _: &str) -> Result<(), HyprctlError> {
            Ok(())
        }
        fn get_hyprland_version(&self) -> Result<String, HyprctlError> {
            Ok("0.54.1".to_string())
        }
    }

    // ── Mock: ProcessInfoProvider ────────────────────────────────────────

    struct MockProcess {
        cwds: HashMap<u32, PathBuf>,
        children: HashMap<u32, Vec<ChildProcess>>,
    }

    impl ProcessInfoProvider for MockProcess {
        fn get_cwd(&self, pid: u32) -> Result<PathBuf, ProcessError> {
            self.cwds
                .get(&pid)
                .cloned()
                .ok_or(ProcessError::NotFound(pid))
        }
        fn get_children(&self, pid: u32) -> Result<Vec<ChildProcess>, ProcessError> {
            Ok(self.children.get(&pid).cloned().unwrap_or_default())
        }
    }

    // ── Helpers ──────────────────────────────────────────────────────────

    fn make_hypr_client(class: &str, pid: u32) -> RawClient {
        RawClient {
            address: "0xdeadbeef".to_string(),
            class: class.to_string(),
            title: format!("{class} window"),
            workspace: RawWorkspace {
                id: 1,
                name: "1".to_string(),
            },
            monitor: 0,
            at: [0, 0],
            size: [800, 600],
            floating: false,
            fullscreen: 0,
            focus_history_id: 0,
            pid,
        }
    }

    fn make_monitor(name: &str) -> RawMonitor {
        RawMonitor {
            name: name.to_string(),
            width: 1920,
            height: 1080,
            transform: 0,
        }
    }

    fn empty_process() -> MockProcess {
        MockProcess {
            cwds: HashMap::new(),
            children: HashMap::new(),
        }
    }

    // ── Test 1: filter ignored classes ───────────────────────────────────

    #[test]
    fn test_capture_filters_ignored_classes() {
        let hyprctl = MockHyprctl {
            clients: vec![
                make_hypr_client("kitty", 1001),
                make_hypr_client("waybar", 1002),
                make_hypr_client("brave-browser", 1003),
            ],
            monitors: vec![make_monitor("DP-1")],
        };

        let config = Config {
            general: GeneralConfig::default(),
            filters: FilterConfig {
                ignore_classes: vec!["waybar".to_string()],
            },
            apps: HashMap::new(),
        };

        let session =
            capture_session("test", &hyprctl, &empty_process(), &config).expect("capture failed");

        assert_eq!(session.clients.len(), 2, "waybar must be excluded");
        let classes: Vec<&str> = session.clients.iter().map(|c| c.class.as_str()).collect();
        assert!(classes.contains(&"kitty"), "kitty must be present");
        assert!(
            classes.contains(&"brave-browser"),
            "brave-browser must be present"
        );
        assert!(!classes.contains(&"waybar"), "waybar must be absent");
    }

    // ── Test 2: kitty with CWD capture ───────────────────────────────────

    #[test]
    fn test_capture_builds_kitty_launch_with_cwd() {
        const KITTY_PID: u32 = 2001;
        const SHELL_PID: u32 = 2002;

        let hyprctl = MockHyprctl {
            clients: vec![make_hypr_client("kitty", KITTY_PID)],
            monitors: vec![make_monitor("DP-1")],
        };

        let mut app_configs = HashMap::new();
        app_configs.insert(
            "kitty".to_string(),
            AppConfig {
                binary: None,
                capture_cwd: Some(true),
                capture_last_command: None,
                hint_template: None,
            },
        );

        let config = Config {
            general: GeneralConfig::default(),
            filters: FilterConfig {
                ignore_classes: vec![],
            },
            apps: app_configs,
        };

        let mut children: HashMap<u32, Vec<ChildProcess>> = HashMap::new();
        children.insert(
            KITTY_PID,
            vec![ChildProcess {
                pid: SHELL_PID,
                cwd: PathBuf::from("/home/user/project"),
                cmdline: "zsh".to_string(),
            }],
        );

        let process = MockProcess {
            cwds: HashMap::new(),
            children,
        };

        let session =
            capture_session("test", &hyprctl, &process, &config).expect("capture failed");

        assert_eq!(session.clients.len(), 1);
        let launch = &session.clients[0].launch;
        assert_eq!(launch.command, "kitty");
        assert_eq!(
            launch.args,
            vec!["--directory", "/home/user/project"],
            "args must contain --directory <cwd>"
        );
        assert!(
            launch.hint.is_none(),
            "no hint expected when capture_last_command is off"
        );
    }

    // ── Test 3: generic app — binary override, no CWD ────────────────────

    #[test]
    fn test_capture_builds_generic_app_launch() {
        const BRAVE_PID: u32 = 3001;

        let hyprctl = MockHyprctl {
            clients: vec![make_hypr_client("brave-browser", BRAVE_PID)],
            monitors: vec![make_monitor("HDMI-A-1")],
        };

        let mut app_configs = HashMap::new();
        app_configs.insert(
            "brave-browser".to_string(),
            AppConfig {
                binary: Some("brave".to_string()),
                capture_cwd: None,
                capture_last_command: None,
                hint_template: None,
            },
        );

        let config = Config {
            general: GeneralConfig::default(),
            filters: FilterConfig {
                ignore_classes: vec![],
            },
            apps: app_configs,
        };

        let session =
            capture_session("test", &hyprctl, &empty_process(), &config).expect("capture failed");

        assert_eq!(session.clients.len(), 1);
        let launch = &session.clients[0].launch;
        assert_eq!(launch.command, "brave", "binary override must be applied");
        assert!(
            launch.args.is_empty(),
            "no args expected without CWD capture"
        );
        assert!(launch.hint.is_none());
    }

    // ── Additional: monitor name resolution ───────────────────────────────

    #[test]
    fn test_capture_resolves_monitor_name_from_index() {
        let hyprctl = MockHyprctl {
            clients: vec![make_hypr_client("kitty", 4001)],
            monitors: vec![make_monitor("DP-2")],
        };

        let config = Config {
            general: GeneralConfig::default(),
            filters: FilterConfig {
                ignore_classes: vec![],
            },
            apps: HashMap::new(),
        };

        let session =
            capture_session("test", &hyprctl, &empty_process(), &config).expect("capture failed");

        assert_eq!(session.clients[0].monitor, "DP-2");
    }

    // ── Additional: version propagated ───────────────────────────────────

    #[test]
    fn test_capture_propagates_hyprland_version() {
        let hyprctl = MockHyprctl {
            clients: vec![],
            monitors: vec![],
        };
        let config = Config::default();

        let session =
            capture_session("test", &hyprctl, &empty_process(), &config).expect("capture failed");

        assert_eq!(session.hyprland_version, "0.54.1");
    }
}
