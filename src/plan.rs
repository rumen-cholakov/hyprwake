//! What a restore will do, decided before anything is done.
//!
//! [`plan`] is pure: it reads the saved session and a snapshot of the live
//! desktop and returns the steps, with no compositor and no clock. [`render`]
//! turns a plan into the dry-run text, and `restore::execute` carries one out,
//! owning every dispatch, sleep and poll. Dry-run and live restore walk the
//! same description, so a new kind of step — layout replay — extends the plan
//! instead of forking restore a third time.
//!
//! The sweep stays reactive and belongs to the executor: single-instance
//! programs produce windows no plan can name in advance. The plan says which
//! saved windows it expects; the executor pairs whatever arrives.

use crate::browsers::profile_argv;
use crate::config::Config;
use crate::hyprctl::{HyprClient, HyprMonitor};
use crate::lua::{lua_long_str, lua_str};
use crate::provider::ProviderSet;
use crate::restore::{
    exec_call, grouped_sets, launch_command, workspace_monitors, workspace_move_call,
    RestoreOptions, RestoreReport,
};
use crate::session::{Session, SessionClient};
use crate::workspace::WorkspaceRef;
use serde::Serialize;
use std::collections::HashMap;

/// One thing a restore does. Serialised for `restore --dry-run --json`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum Step {
    /// `--missing-only`: this saved window's class is already on screen.
    AlreadyOpen { client: usize, class: String },
    /// Launch a saved window's program, with rules that place it as it maps.
    Spawn {
        client: usize,
        class: String,
        workspace: String,
        /// Tiled windows are launched one at a time per workspace: dwindle
        /// tiles in the order windows *map*, so a fast terminal launched
        /// second would otherwise overtake a slow browser launched first.
        tiled: bool,
        /// Wait for the previous tiled window on this workspace to map first.
        wait_for_previous: bool,
        command: String,
        call: String,
    },
    /// Launch one browser profile in its own window.
    SpawnProfile {
        class: String,
        name: String,
        workspace: String,
        command: String,
        call: String,
    },
    /// Pair windows that arrive with these saved entries, and place the ones
    /// the launch rules could not reach.
    Sweep { expect: Vec<usize> },
    /// Send a workspace back to the monitor it was on.
    MoveWorkspace {
        workspace: String,
        monitor: String,
        call: String,
    },
    /// The monitor a workspace was on is not attached; leave it be.
    MonitorMissing { workspace: String, monitor: String },
    /// Members of a window group come back as separate windows.
    GroupUnrestored { group: u32, classes: Vec<String> },
    /// Bring a monitor or a workspace forward.
    Focus { call: String },
    /// Focus whichever window the saved focused client turned into.
    FocusWindow { client: usize },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RestorePlan {
    pub steps: Vec<Step>,
    /// Programs this restore launches: windows plus browser profiles.
    pub launches: usize,
    pub already_open: usize,
    pub groups_unrestored: usize,
}

/// Decide the restore.
///
/// `monitors` are the monitors attached now, or `None` when they could not be
/// read, in which case workspaces are left where they land. `use_uwsm` is
/// passed in rather than probed, which keeps this pure.
pub fn plan(
    session: &Session,
    live: &[HyprClient],
    monitors: Option<&[HyprMonitor]>,
    config: &Config,
    opts: &RestoreOptions,
    use_uwsm: bool,
) -> RestorePlan {
    let mut steps = Vec::new();
    let mut already_open = 0;

    let wanted = workspace_monitors(session);

    // Tiled windows first, in their old reading order, so the layout
    // re-tiles roughly as it was; floating windows are positioned explicitly.
    let mut order: Vec<usize> = (0..session.clients.len()).collect();
    order.sort_by(|&a, &b| {
        let (a, b) = (&session.clients[a], &session.clients[b]);
        a.floating
            .cmp(&b.floating)
            .then(a.workspace.selector().cmp(&b.workspace.selector()))
            .then(a.at[1].cmp(&b.at[1]))
            .then(a.at[0].cmp(&b.at[0]))
    });

    // What is already on screen, by class, so a merge can tell "this window
    // is back" from "this window is still missing".
    let mut already: HashMap<&str, usize> = HashMap::new();
    if opts.missing_only {
        for window in live {
            *already.entry(window.class.as_str()).or_default() += 1;
        }
    }

    let mut previous_tiled: Option<String> = None;
    for index in order {
        let client: &SessionClient = &session.clients[index];
        if !client.launch.spawn {
            continue;
        }
        if let Some(count) = already.get_mut(client.class.as_str()) {
            if *count > 0 {
                *count -= 1;
                already_open += 1;
                steps.push(Step::AlreadyOpen {
                    client: index,
                    class: client.class.clone(),
                });
                continue;
            }
        }
        let workspace = client.workspace.selector();
        let tiled = !client.floating;
        let command = launch_command(&client.launch.argv, use_uwsm);
        steps.push(Step::Spawn {
            client: index,
            class: client.class.clone(),
            wait_for_previous: tiled && previous_tiled.as_deref() == Some(workspace.as_str()),
            call: exec_call(&command, &client.workspace, client),
            command,
            workspace: workspace.clone(),
            tiled,
        });
        if tiled {
            previous_tiled = Some(workspace);
        }
    }

    let providers = ProviderSet::from_config(config);
    for profile in &session.browser_profiles {
        let Some(browser) = providers.browser_for(&profile.class) else {
            continue;
        };
        let command = launch_command(&profile_argv(&browser, profile), use_uwsm);
        let workspace = WorkspaceRef::new(
            profile.workspace.parse().unwrap_or(0),
            profile.workspace.clone(),
        );
        steps.push(Step::SpawnProfile {
            class: profile.class.clone(),
            name: profile.name.clone(),
            workspace: profile.workspace.clone(),
            call: format!(
                "hl.dsp.exec_cmd({}, {{ workspace = {} }})",
                lua_long_str(&command),
                lua_str(&format!("{} silent", workspace.selector()))
            ),
            command,
        });
    }

    steps.push(Step::Sweep {
        expect: (0..session.clients.len()).collect(),
    });

    // After the windows exist: Hyprland has no workspace to move until
    // something is on it. A session saved docked must still restore on the
    // road, so a missing monitor is noted and skipped.
    if let Some(attached) = monitors {
        for (workspace, monitor) in &wanted {
            if attached.iter().any(|m| &m.name == monitor) {
                steps.push(Step::MoveWorkspace {
                    call: workspace_move_call(workspace, monitor),
                    workspace: workspace.clone(),
                    monitor: monitor.clone(),
                });
            } else {
                steps.push(Step::MonitorMissing {
                    workspace: workspace.clone(),
                    monitor: monitor.clone(),
                });
            }
        }
    }

    let groups = grouped_sets(session);
    for (group, members) in &groups {
        steps.push(Step::GroupUnrestored {
            group: *group,
            classes: members
                .iter()
                .map(|i| session.clients[*i].class.clone())
                .collect(),
        });
    }

    // Last, after every window is in place; anything earlier would be undone
    // by the placements that follow.
    for monitor in &session.monitors {
        let Some(workspace) = &monitor.active_workspace else {
            continue;
        };
        // A special workspace is an overlay, not a resting state.
        if workspace.is_special() {
            continue;
        }
        steps.push(Step::Focus {
            call: format!("hl.dsp.focus({{ monitor = {} }})", lua_str(&monitor.name)),
        });
        steps.push(Step::Focus {
            call: format!(
                "hl.dsp.focus({{ workspace = {} }})",
                lua_str(&workspace.selector())
            ),
        });
    }
    // Focusing the window last also brings its monitor and workspace forward.
    if let Some(client) = session.focused_client() {
        steps.push(Step::FocusWindow { client });
    }

    let launches = steps
        .iter()
        .filter(|s| matches!(s, Step::Spawn { .. } | Step::SpawnProfile { .. }))
        .count();
    RestorePlan {
        steps,
        launches,
        already_open,
        groups_unrestored: groups.len(),
    }
}

impl Step {
    /// What the executor reports for a monitor it will not use.
    pub fn monitor_missing_note(workspace: &str, monitor: &str) -> String {
        format!("monitor {monitor} is not attached; leaving workspace {workspace}")
    }

    pub fn group_note(group: u32, classes: &[String]) -> String {
        format!(
            "group {group} ({}) restored as separate windows: Hyprland has no \
             dispatcher to regroup them",
            classes.join(", ")
        )
    }
}

/// The dry run: what a live restore would dispatch, as text.
///
/// Nothing is sent and nothing is waited for, so the sweep and the window
/// focus that depends on it have nothing to show.
pub fn render(plan: &RestorePlan, opts: &RestoreOptions) -> RestoreReport {
    let mut report = RestoreReport {
        spawned: plan.launches,
        already_open: plan.already_open,
        groups_unrestored: plan.groups_unrestored,
        ..Default::default()
    };
    let out = &mut report.details;
    for step in &plan.steps {
        match step {
            Step::AlreadyOpen { class, .. } => {
                if opts.verbose {
                    out.push(format!("already open: {class}"));
                }
            }
            Step::Spawn {
                class,
                command,
                call,
                ..
            } => {
                out.push(format!("[dry-run] {class} → {command}"));
                out.push(format!("  hyprctl dispatch {call}"));
            }
            Step::SpawnProfile {
                class,
                name,
                workspace,
                call,
                ..
            } => {
                out.push(format!(
                    "[dry-run] {class} profile \"{name}\" → ws {workspace}"
                ));
                out.push(format!("  hyprctl dispatch {call}"));
            }
            Step::MoveWorkspace { call, .. } | Step::Focus { call } => {
                out.push(format!("  hyprctl dispatch {call}"));
            }
            Step::MonitorMissing { workspace, monitor } => {
                if opts.verbose {
                    out.push(Step::monitor_missing_note(workspace, monitor));
                }
            }
            Step::GroupUnrestored { group, classes } => {
                out.push(Step::group_note(*group, classes));
            }
            Step::Sweep { .. } | Step::FocusWindow { .. } => {}
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{BrowserProfile, LaunchInfo, Monitor};

    // No HyprctlClient anywhere in here: a plan is a function of the saved
    // session and a snapshot of the desktop, and is tested as one.

    fn saved(class: &str, ws: (i32, &str), x: i32) -> SessionClient {
        SessionClient {
            class: class.to_string(),
            initial_class: class.to_string(),
            title: String::new(),
            workspace: WorkspaceRef::new(ws.0, ws.1),
            monitor: "eDP-1".to_string(),
            at: [x, 30],
            size: [800, 600],
            floating: false,
            pinned: false,
            fullscreen: 0,
            focus_history_id: -1,
            group: None,
            identity: Default::default(),
            launch: LaunchInfo {
                argv: vec![class.to_string()],
                spawn: true,
                provider: None,
                provider_state: None,
            },
        }
    }

    fn live(class: &str, ws: (i32, &str)) -> HyprClient {
        HyprClient {
            address: format!("0x{class}"),
            class: class.to_string(),
            initial_class: class.to_string(),
            title: String::new(),
            workspace: WorkspaceRef::new(ws.0, ws.1),
            monitor: 0,
            at: [0, 0],
            size: [0, 0],
            floating: false,
            pinned: false,
            fullscreen: 0,
            focus_history_id: 0,
            pid: 1,
            mapped: true,
            grouped: vec![],
        }
    }

    fn session(clients: Vec<SessionClient>) -> Session {
        Session {
            schema_version: crate::session::SCHEMA_VERSION,
            created_by: String::new(),
            loaded_from: None,
            name: "t".to_string(),
            created_at: chrono::Utc::now(),
            hyprland_version: String::new(),
            monitors: vec![],
            workspaces: vec![],
            clients,
            browser_profiles: vec![],
        }
    }

    fn config() -> Config {
        toml::from_str("").unwrap()
    }

    fn attached(names: &[&str]) -> Vec<HyprMonitor> {
        names
            .iter()
            .enumerate()
            .map(|(i, name)| HyprMonitor {
                id: i as i32,
                name: name.to_string(),
                width: 1920,
                height: 1080,
                transform: 0,
                active_workspace: None,
                focused: i == 0,
                make: String::new(),
                model: String::new(),
                serial: String::new(),
                scale: 1.0,
                x: 1920 * i as i32,
                y: 0,
            })
            .collect()
    }

    fn spawned(plan: &RestorePlan) -> Vec<(String, bool)> {
        plan.steps
            .iter()
            .filter_map(|s| match s {
                Step::Spawn {
                    class,
                    wait_for_previous,
                    ..
                } => Some((class.clone(), *wait_for_previous)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn tiled_windows_launch_left_to_right_and_wait_for_each_other() {
        let mut float = saved("obsidian", (1, "1"), 0);
        float.floating = true;
        let s = session(vec![
            saved("foot", (1, "1"), 771),
            float,
            saved("code", (1, "1"), 4),
            saved("kitty", (2, "2"), 4),
        ]);
        let p = plan(&s, &[], None, &config(), &RestoreOptions::default(), false);
        assert_eq!(
            spawned(&p),
            vec![
                ("code".into(), false),
                ("foot".into(), true),      // same workspace, after code
                ("kitty".into(), false),    // a new workspace does not wait
                ("obsidian".into(), false), // floating: placed exactly, never waits
            ]
        );
        assert_eq!(p.launches, 4);
    }

    #[test]
    fn a_window_its_process_reopens_is_expected_but_not_launched() {
        let mut second = saved("chrome", (1, "1"), 900);
        second.launch.spawn = false;
        let s = session(vec![saved("chrome", (1, "1"), 0), second]);
        let p = plan(&s, &[], None, &config(), &RestoreOptions::default(), false);
        assert_eq!(spawned(&p).len(), 1);
        assert!(p.steps.contains(&Step::Sweep { expect: vec![0, 1] }));
    }

    #[test]
    fn missing_only_skips_what_is_already_open_one_for_one() {
        let s = session(vec![
            saved("foot", (1, "1"), 0),
            saved("foot", (1, "1"), 500),
            saved("code", (2, "2"), 0),
        ]);
        let opts = RestoreOptions {
            missing_only: true,
            ..Default::default()
        };
        let p = plan(&s, &[live("foot", (3, "3"))], None, &config(), &opts, false);
        assert_eq!(p.already_open, 1, "one foot is open, so one is launched");
        assert_eq!(
            spawned(&p).iter().map(|s| s.0.as_str()).collect::<Vec<_>>(),
            vec!["foot", "code"]
        );
    }

    #[test]
    fn workspaces_go_back_to_attached_monitors_only() {
        let mut far = saved("mpv", (10, "10"), 0);
        far.monitor = "HDMI-A-1".to_string();
        let s = session(vec![saved("foot", (1, "1"), 0), far]);
        let p = plan(
            &s,
            &[],
            Some(&attached(&["eDP-1"])),
            &config(),
            &RestoreOptions::default(),
            false,
        );
        assert!(p.steps.iter().any(|s| matches!(s,
            Step::MoveWorkspace { workspace, monitor, .. } if workspace == "1" && monitor == "eDP-1")));
        assert!(p.steps.contains(&Step::MonitorMissing {
            workspace: "10".into(),
            monitor: "HDMI-A-1".into()
        }));
        let unknown = plan(&s, &[], None, &config(), &RestoreOptions::default(), false);
        assert!(
            !unknown
                .steps
                .iter()
                .any(|s| matches!(s, Step::MoveWorkspace { .. } | Step::MonitorMissing { .. })),
            "unreadable monitors: leave workspaces where they land"
        );
    }

    #[test]
    fn focus_comes_last_and_skips_special_workspaces() {
        let mut focused = saved("code", (1, "1"), 0);
        focused.focus_history_id = 0;
        let mut s = session(vec![saved("foot", (1, "1"), 500), focused]);
        let monitor = |name: &str, ws: WorkspaceRef| Monitor {
            name: name.into(),
            width: 1,
            height: 1,
            transform: 0,
            active_workspace: Some(ws),
            focused: false,
            make: String::new(),
            model: String::new(),
            serial: String::new(),
            scale: 1.0,
            position: [0, 0],
        };
        s.monitors = vec![
            monitor("eDP-1", WorkspaceRef::new(1, "1")),
            monitor("DP-2", WorkspaceRef::new(-99, "special:magic")),
        ];
        let p = plan(&s, &[], None, &config(), &RestoreOptions::default(), false);
        let tail: Vec<&Step> = p.steps.iter().rev().take(3).collect();
        assert_eq!(tail[0], &Step::FocusWindow { client: 1 });
        assert!(matches!(tail[1], Step::Focus { call } if call.contains("workspace = '1'")));
        assert!(matches!(tail[2], Step::Focus { call } if call.contains("monitor = 'eDP-1'")));
        assert!(!p
            .steps
            .iter()
            .any(|s| matches!(s, Step::Focus { call } if call.contains("DP-2"))));
    }

    #[test]
    fn a_profile_is_launched_only_when_a_browser_provider_claims_its_class() {
        let mut s = session(vec![]);
        s.browser_profiles = vec![BrowserProfile {
            class: "google-chrome".into(),
            directory: "Profile 1".into(),
            name: "Work".into(),
            workspace: "6".into(),
        }];
        let none = plan(&s, &[], None, &config(), &RestoreOptions::default(), false);
        assert_eq!(none.launches, 0);

        let with: Config = toml::from_str(
            "[providers.chrome]\nkind = \"chromium\"\nclasses = [\"google-chrome\"]\n\
             binary = \"google-chrome-stable\"\nlocal_state = \"google-chrome/Local State\"\n\
             restore_last_session = true\n",
        )
        .unwrap();
        let p = plan(&s, &[], None, &with, &RestoreOptions::default(), true);
        let Some(Step::SpawnProfile { command, call, .. }) = p.steps.first() else {
            panic!("{:?}", p.steps);
        };
        assert_eq!(
            command,
            "uwsm-app -- google-chrome-stable '--profile-directory=Profile 1' --restore-last-session"
        );
        assert!(call.contains("workspace = '6 silent'"));
    }

    #[test]
    fn the_dry_run_renders_the_plan_without_sweep_or_window_focus() {
        let mut focused = saved("foot", (1, "1"), 0);
        focused.focus_history_id = 0;
        let s = session(vec![focused]);
        let p = plan(&s, &[], None, &config(), &RestoreOptions::default(), false);
        let report = render(&p, &RestoreOptions::default());
        assert_eq!(
            report.details,
            vec![
                "[dry-run] foot → foot",
                "  hyprctl dispatch hl.dsp.exec_cmd([[foot]], { workspace = '1 silent' })",
            ]
        );
        assert_eq!(report.spawned, 1);
    }

    #[test]
    fn a_plan_serialises_as_tagged_actions() {
        let s = session(vec![saved("foot", (1, "1"), 0)]);
        let p = plan(&s, &[], None, &config(), &RestoreOptions::default(), false);
        let json = serde_json::to_value(&p).unwrap();
        assert_eq!(json["steps"][0]["action"], "spawn");
        assert_eq!(json["steps"][0]["class"], "foot");
        assert_eq!(json["steps"][1]["action"], "sweep");
        assert_eq!(json["launches"], 1);
    }
}
