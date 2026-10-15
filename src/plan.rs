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
use crate::config::LayoutMode;
use crate::config::MissingMonitor;
use crate::hyprctl::{HyprClient, HyprMonitor};
use crate::layout::{self, ReplayOp};
use crate::lua::{lua_long_str, lua_str};
use crate::matching;
use crate::provider::ProviderSet;
use crate::restore::{
    exec_call, grouped_sets, launch_command, workspace_monitors, workspace_move_call,
    RestoreOptions, RestoreReport,
};
use crate::session::{LayoutNode, Session, SessionClient, Workspace};
use crate::topology::{self, Placement};
use crate::workspace::WorkspaceRef;
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};

/// One thing a restore does. Serialised for `restore --dry-run --json`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum Step {
    /// An external provider's relaunch helper was asked what to start for a
    /// saved window. `reply` is what it printed, verbatim; `rejected` says
    /// why it was not used, in which case the saved argv is launched.
    ExternalRelaunch {
        client: usize,
        class: String,
        provider: String,
        reply: Option<String>,
        rejected: Option<String>,
    },
    /// `--missing-only`: this saved window is already on screen, as the
    /// window at `address`.
    AlreadyOpen {
        client: usize,
        class: String,
        address: String,
    },
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
    /// A workspace whose saved split tree is rebuilt, split by split, before
    /// anything else is launched onto it.
    LayoutBegin {
        workspace: String,
        kind: String,
        windows: usize,
        confidence: f32,
    },
    /// Launch a window of a tree being rebuilt and wait for it to map: the
    /// steps after it focus it to split or size it.
    LayoutSpawn {
        client: usize,
        class: String,
        workspace: String,
        command: String,
        call: String,
    },
    /// Focus a window of a tree being rebuilt, then send the layout `call`.
    LayoutMessage {
        workspace: String,
        focus: usize,
        class: String,
        message: String,
        call: String,
    },
    /// A saved tree that is not rebuilt, and why. Its windows are launched in
    /// reading order instead.
    LayoutSkipped { workspace: String, reason: String },
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
    /// Send a workspace back to the monitor it was on — or, when that
    /// monitor is gone and config says so, to another one.
    MoveWorkspace {
        workspace: String,
        monitor: String,
        call: String,
        /// Why this monitor, whenever it is not simply the same connector.
        reason: Option<String>,
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
    /// Floating windows carried to a different monitor: the position and
    /// size to restore them at instead of the saved ones.
    pub geometry: BTreeMap<usize, Geometry>,
    /// Programs this restore launches: windows plus browser profiles.
    pub launches: usize,
    pub already_open: usize,
    pub groups_unrestored: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Geometry {
    pub at: [i32; 2],
    pub size: [i32; 2],
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

    // Where each saved monitor's workspaces go. Decided first: a floating
    // window carried to another monitor launches with its new geometry.
    let by_identity =
        opts.saved_topology || config.monitors.missing != MissingMonitor::PreserveNumbers;
    let wanted = workspace_monitors(session);
    let placements: BTreeMap<&str, Placement<'_>> = match monitors {
        Some(live_monitors) => wanted
            .values()
            .chain(session.monitors.iter().map(|m| &m.name))
            .map(|saved| {
                let record = session.monitors.iter().find(|m| &m.name == saved);
                (
                    saved.as_str(),
                    topology::place(saved, record, live_monitors, &config.monitors, by_identity),
                )
            })
            .collect(),
        None => BTreeMap::new(),
    };
    let geometry = if by_identity {
        carried_geometry(session, &wanted, &placements)
    } else {
        BTreeMap::new()
    };

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

    // `--missing-only`: which saved windows are already on screen, paired
    // the way the sweep pairs arrivals. A terminal open on workspace 2 is the
    // one saved there, so it is the one saved on workspace 1 that launches.
    let open: HashMap<usize, &str> = if opts.missing_only {
        let saved: Vec<&SessionClient> = session.clients.iter().collect();
        let windows: Vec<&HyprClient> = live.iter().collect();
        matching::pair_all(&saved, &windows)
            .into_iter()
            .zip(live)
            .filter_map(|(pair, window)| pair.map(|(i, _)| (i, window.address.as_str())))
            .collect()
    } else {
        HashMap::new()
    };

    // Saved trees to rebuild, workspace by workspace, before the windows
    // launched in reading order.
    let mode = config.restore.layout;
    let mut rebuilt: HashSet<usize> = HashSet::new();
    for ws in &session.workspaces {
        match layout_replay(session, ws, live, &open, config) {
            Ok(None) => {}
            Err(reason) => steps.push(Step::LayoutSkipped {
                workspace: ws.workspace.selector(),
                reason,
            }),
            Ok(Some((tree, kind))) => {
                let workspace = ws.workspace.selector();
                steps.push(Step::LayoutBegin {
                    workspace: workspace.clone(),
                    kind: kind.to_string(),
                    windows: layout::leaves(tree).len(),
                    confidence: ws.layout_confidence.unwrap_or(0.0),
                });
                for op in layout::replay(tree, kind) {
                    steps.push(match op {
                        ReplayOp::Spawn(index) => {
                            let client = &session.clients[index];
                            rebuilt.insert(index);
                            let command = launch_command(&client.launch.argv, use_uwsm);
                            Step::LayoutSpawn {
                                client: index,
                                class: client.class.clone(),
                                workspace: workspace.clone(),
                                call: exec_call(&command, &client.workspace, client),
                                command,
                            }
                        }
                        ReplayOp::Message { focus, message } => Step::LayoutMessage {
                            workspace: workspace.clone(),
                            focus,
                            class: session.clients[focus].class.clone(),
                            call: format!("hl.dsp.layout({})", lua_str(&message)),
                            message,
                        },
                    });
                }
            }
        }
    }

    let mut previous_tiled: Option<String> = None;
    for index in order {
        let client: &SessionClient = &session.clients[index];
        if rebuilt.contains(&index) {
            continue;
        }
        if let Some(address) = open.get(&index) {
            already_open += 1;
            steps.push(Step::AlreadyOpen {
                client: index,
                class: client.class.clone(),
                address: address.to_string(),
            });
            continue;
        }
        if !client.launch.spawn {
            continue;
        }
        let workspace = client.workspace.selector();
        let tiled = !client.floating;
        let command = launch_command(&client.launch.argv, use_uwsm);
        let placed = with_geometry(client, geometry.get(&index));
        steps.push(Step::Spawn {
            client: index,
            class: client.class.clone(),
            wait_for_previous: tiled
                && mode != LayoutMode::Off
                && previous_tiled.as_deref() == Some(workspace.as_str()),
            call: exec_call(&command, &client.workspace, &placed),
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

    // Everything not already on screen: windows launched above, and those
    // their programs reopen on their own.
    steps.push(Step::Sweep {
        expect: (0..session.clients.len())
            .filter(|i| !open.contains_key(i))
            .collect(),
    });

    // After the windows exist: Hyprland has no workspace to move until
    // something is on it. A session saved docked must still restore on the
    // road, so a missing monitor is noted and skipped.
    if monitors.is_some() {
        for (workspace, saved) in &wanted {
            match placements
                .get(saved.as_str())
                .and_then(|p| p.live().map(|l| (p, l)))
            {
                Some((placement, live)) => steps.push(Step::MoveWorkspace {
                    call: workspace_move_call(workspace, &live.name),
                    workspace: workspace.clone(),
                    monitor: live.name.clone(),
                    reason: placement.reason(saved),
                }),
                None => steps.push(Step::MonitorMissing {
                    workspace: workspace.clone(),
                    monitor: saved.clone(),
                }),
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
        // By connector, as before 0.2.0 — unless monitors are being matched by
        // identity, in which case focus follows the monitor the workspaces
        // went to, and a monitor that went nowhere is not focused at all.
        let name = if by_identity {
            match placements.get(monitor.name.as_str()).and_then(|p| p.live()) {
                Some(live) => live.name.clone(),
                None => continue,
            }
        } else {
            monitor.name.clone()
        };
        steps.push(Step::Focus {
            call: format!("hl.dsp.focus({{ monitor = {} }})", lua_str(&name)),
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
        .filter(|s| {
            matches!(
                s,
                Step::Spawn { .. } | Step::SpawnProfile { .. } | Step::LayoutSpawn { .. }
            )
        })
        .count();
    RestorePlan {
        steps,
        geometry,
        launches,
        already_open,
        groups_unrestored: groups.len(),
    }
}

/// Whether a workspace's saved tree is rebuilt: `Ok(Some)` with the tree and
/// its kind, `Ok(None)` when there is nothing to decide (reading order is
/// configured, or nothing was recorded), `Err` with the reason a recorded
/// tree is passed over.
fn layout_replay<'a>(
    session: &'a Session,
    ws: &'a Workspace,
    live: &[HyprClient],
    open: &HashMap<usize, &str>,
    config: &Config,
) -> Result<Option<(&'a LayoutNode, &'a str)>, String> {
    let mode = config.restore.layout;
    if !matches!(mode, LayoutMode::BestEffort | LayoutMode::Strict) {
        return Ok(None);
    }
    let Some(kind) = ws.layout_kind.as_deref() else {
        return Ok(None);
    };
    let confidence = ws.layout_confidence.unwrap_or(0.0);
    let Some(tree) = &ws.layout else {
        return Err("no split tree could be read from its windows when it was saved".into());
    };
    if ws.workspace.is_special() {
        return Err(
            "a special workspace is an overlay; its windows are not rebuilt as a tree".into(),
        );
    }
    let threshold = config.restore.layout_min_confidence;
    if mode == LayoutMode::BestEffort && confidence < threshold {
        return Err(format!(
            "its tree was read at confidence {confidence:.2}, below the {threshold:.2} \
             layout_min_confidence"
        ));
    }
    let leaves = layout::leaves(tree);
    let tiled: HashSet<usize> = session
        .clients
        .iter()
        .enumerate()
        .filter(|(_, c)| c.workspace.same(&ws.workspace) && !c.floating)
        .map(|(i, _)| i)
        .collect();
    if leaves.len() != tiled.len() || leaves.iter().any(|i| !tiled.contains(i)) {
        return Err("its tree does not cover the windows saved on it".into());
    }
    if let Some(&i) = leaves.iter().find(|&&i| !session.clients[i].launch.spawn) {
        return Err(format!(
            "{} is reopened by its own program, not launched, so it cannot be placed in the tree",
            session.clients[i].class
        ));
    }
    if leaves.iter().any(|i| open.contains_key(i))
        || live.iter().any(|w| w.workspace.same(&ws.workspace))
    {
        return Err("it already has windows open".into());
    }
    Ok(Some((tree, kind)))
}

/// New geometry for floating windows whose workspace is going to a monitor
/// that is not where it was saved.
///
/// Only for snapshots that recorded where each monitor sat: a 0.1.x snapshot
/// has no monitor positions, and fitting from a guessed origin would move
/// windows *off* the screen it is meant to keep them on.
fn carried_geometry(
    session: &Session,
    wanted: &BTreeMap<String, String>,
    placements: &BTreeMap<&str, Placement<'_>>,
) -> BTreeMap<usize, Geometry> {
    let mut out = BTreeMap::new();
    if session.loaded_from.unwrap_or(session.schema_version) < 2 {
        return out;
    }
    for (index, client) in session.clients.iter().enumerate() {
        if !client.floating || client.workspace.is_special() {
            continue;
        }
        let Some(saved) = wanted.get(&client.workspace.selector()) else {
            continue;
        };
        let (Some(from), Some(to)) = (
            session.monitors.iter().find(|m| &m.name == saved),
            placements.get(saved.as_str()).and_then(|p| p.live()),
        ) else {
            continue;
        };
        let (at, size) = topology::fit(client.at, client.size, from, to);
        if at != client.at || size != client.size {
            out.insert(index, Geometry { at, size });
        }
    }
    out
}

/// `client` with its geometry replaced, when the plan moves it.
pub fn with_geometry(client: &SessionClient, geometry: Option<&Geometry>) -> SessionClient {
    let mut placed = client.clone();
    if let Some(g) = geometry {
        placed.at = g.at;
        placed.size = g.size;
    }
    placed
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
            Step::LayoutBegin {
                workspace,
                kind,
                windows,
                confidence,
            } => out.push(format!(
                "[layout] workspace {workspace}: rebuilding a {kind} tree of {windows} windows \
                 (confidence {confidence:.2})"
            )),
            Step::LayoutSpawn {
                class,
                command,
                call,
                ..
            } => {
                out.push(format!("[dry-run] {class} → {command}"));
                out.push(format!("  hyprctl dispatch {call}"));
            }
            Step::LayoutMessage { class, call, .. } => {
                out.push(format!(
                    "  hyprctl dispatch hl.dsp.focus({{ window = <the {class} window> }})"
                ));
                out.push(format!("  hyprctl dispatch {call}"));
            }
            Step::LayoutSkipped { workspace, reason } => out.push(format!(
                "[layout] workspace {workspace}: not rebuilt, {reason}; launched in reading order"
            )),
            Step::ExternalRelaunch {
                class,
                provider,
                reply,
                rejected,
                ..
            } => {
                out.push(format!(
                    "[external] {provider} relaunch for {class}: {}",
                    reply.as_deref().map(str::trim).unwrap_or("(no reply)")
                ));
                if let Some(why) = rejected {
                    out.push(format!("  not used: {why}; the saved argv is launched"));
                }
            }
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
            Step::MoveWorkspace {
                workspace,
                monitor,
                call,
                reason,
            } => {
                if let Some(reason) = reason {
                    out.push(format!("workspace {workspace} → {monitor}: {reason}"));
                }
                out.push(format!("  hyprctl dispatch {call}"));
            }
            Step::Focus { call } => {
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
            initial_title: String::new(),
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
    fn missing_only_treats_the_open_window_as_the_one_saved_where_it_is() {
        let s = session(vec![
            saved("foot", (1, "1"), 0),
            saved("foot", (2, "2"), 0),
            saved("code", (2, "2"), 500),
        ]);
        let opts = RestoreOptions {
            missing_only: true,
            ..Default::default()
        };
        let p = plan(&s, &[live("foot", (2, "2"))], None, &config(), &opts, false);
        assert!(p.steps.contains(&Step::AlreadyOpen {
            client: 1,
            class: "foot".into(),
            address: "0xfoot".into()
        }));
        let launched: Vec<&str> = p
            .steps
            .iter()
            .filter_map(|s| match s {
                Step::Spawn {
                    workspace, class, ..
                } if class == "foot" => Some(workspace.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            launched,
            vec!["1"],
            "the foot saved on 1 is the missing one"
        );
        assert!(
            p.steps.contains(&Step::Sweep { expect: vec![0, 2] }),
            "an open window is not waited for: {:?}",
            p.steps
        );
    }

    fn tree_session(spawn_second: bool) -> Session {
        use crate::layout::tests::{leaf, node};
        let mut second = saved("code", (2, "2"), 900);
        second.launch.spawn = spawn_second;
        let mut s = session(vec![saved("foot", (2, "2"), 0), second]);
        s.workspaces = vec![Workspace {
            workspace: WorkspaceRef::new(2, "2"),
            monitor: "eDP-1".into(),
            layout: Some(node(
                crate::session::Axis::Horizontal,
                0.5,
                leaf(0),
                leaf(1),
            )),
            layout_kind: Some("dwindle".into()),
            layout_confidence: Some(1.0),
        }];
        s
    }

    fn with_layout(mode: LayoutMode) -> Config {
        let mut c = config();
        c.restore.layout = mode;
        c
    }

    #[test]
    fn reading_order_ignores_a_recorded_tree() {
        let p = plan(
            &tree_session(true),
            &[],
            None,
            &config(),
            &RestoreOptions::default(),
            false,
        );
        assert!(!p.steps.iter().any(|s| matches!(
            s,
            Step::LayoutBegin { .. } | Step::LayoutSkipped { .. } | Step::LayoutSpawn { .. }
        )));
        assert_eq!(spawned(&p).len(), 2);
    }

    #[test]
    fn best_effort_rebuilds_the_tree_instead_of_launching_in_reading_order() {
        let p = plan(
            &tree_session(true),
            &[],
            None,
            &with_layout(LayoutMode::BestEffort),
            &RestoreOptions::default(),
            false,
        );
        assert!(
            spawned(&p).is_empty(),
            "no reading-order launch for a rebuilt window"
        );
        let rebuilt: Vec<usize> = p
            .steps
            .iter()
            .filter_map(|s| match s {
                Step::LayoutSpawn { client, .. } => Some(*client),
                _ => None,
            })
            .collect();
        assert_eq!(rebuilt, vec![0, 1]);
        assert_eq!(p.launches, 2);
        assert!(p.steps.contains(&Step::Sweep { expect: vec![0, 1] }));
    }

    #[test]
    fn a_tree_is_not_rebuilt_around_windows_hyprwake_does_not_launch() {
        let p = plan(
            &tree_session(false),
            &[],
            None,
            &with_layout(LayoutMode::Strict),
            &RestoreOptions::default(),
            false,
        );
        assert!(p.steps.iter().any(|s| matches!(s,
            Step::LayoutSkipped { reason, .. } if reason.contains("reopened by its own program"))));
        // nor onto a workspace that already has windows
        let busy = plan(
            &tree_session(true),
            &[live("mpv", (2, "2"))],
            None,
            &with_layout(LayoutMode::Strict),
            &RestoreOptions::default(),
            false,
        );
        assert!(busy.steps.iter().any(|s| matches!(s,
            Step::LayoutSkipped { reason, .. } if reason.contains("already has windows"))));
    }

    #[test]
    fn off_launches_without_waiting_for_tiles() {
        let s = session(vec![
            saved("foot", (1, "1"), 0),
            saved("code", (1, "1"), 900),
        ]);
        let p = plan(
            &s,
            &[],
            None,
            &with_layout(LayoutMode::Off),
            &RestoreOptions::default(),
            false,
        );
        assert_eq!(
            spawned(&p),
            vec![("foot".into(), false), ("code".into(), false)]
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

    // ── monitor topology, driven by recorded `hyprctl monitors -j` ──

    use crate::config::{MissingMonitor, MonitorConfig};
    use crate::topology::tests::{monitors, saved as saved_monitor};

    /// Saved docked: a terminal on the panel's workspace 1, and on the Dell's
    /// workspace 3 an editor and a floating notes window on its right half.
    fn docked_session() -> Session {
        let docked = monitors("docked");
        let mut notes = saved("obsidian", (3, "3"), 1536 + 1280);
        notes.monitor = "DP-1".into();
        notes.floating = true;
        notes.at = [1536 + 1280, 100];
        notes.size = [1200, 800];
        let mut code = saved("code", (3, "3"), 1536);
        code.monitor = "DP-1".into();
        code.focus_history_id = 0;
        let mut s = session(vec![saved("foot", (1, "1"), 0), code, notes]);
        s.monitors = docked.iter().map(saved_monitor).collect();
        s
    }

    fn with_missing(missing: MissingMonitor) -> Config {
        let mut c = config();
        c.monitors = MonitorConfig {
            missing,
            ..Default::default()
        };
        c
    }

    fn moves(p: &RestorePlan) -> Vec<(String, String, Option<String>)> {
        p.steps
            .iter()
            .filter_map(|s| match s {
                Step::MoveWorkspace {
                    workspace,
                    monitor,
                    reason,
                    ..
                } => Some((workspace.clone(), monitor.clone(), reason.clone())),
                _ => None,
            })
            .collect()
    }

    fn spawn_call<'a>(p: &'a RestorePlan, class: &str) -> &'a str {
        p.steps
            .iter()
            .find_map(|s| match s {
                Step::Spawn { class: c, call, .. } if c == class => Some(call.as_str()),
                _ => None,
            })
            .unwrap()
    }

    #[test]
    fn docked_to_undocked_by_default_is_what_0_1_did() {
        let laptop = monitors("laptop");
        let p = plan(
            &docked_session(),
            &[],
            Some(&laptop),
            &config(),
            &RestoreOptions::default(),
            false,
        );
        assert_eq!(moves(&p), vec![("1".into(), "eDP-1".into(), None)]);
        assert!(p.steps.contains(&Step::MonitorMissing {
            workspace: "3".into(),
            monitor: "DP-1".into()
        }));
        assert!(p.geometry.is_empty(), "no fitting unless asked for");
        assert!(spawn_call(&p, "obsidian").contains("move = {2816, 100}"));
    }

    #[test]
    fn docked_to_undocked_with_remap_moves_and_fits_the_lost_monitor() {
        let laptop = monitors("laptop");
        let p = plan(
            &docked_session(),
            &[],
            Some(&laptop),
            &with_missing(MissingMonitor::Remap),
            &RestoreOptions::default(),
            false,
        );
        let m = moves(&p);
        assert_eq!(m[1].0, "3");
        assert_eq!(m[1].1, "eDP-1");
        assert!(m[1].2.as_deref().unwrap().contains("nearest"), "{m:?}");
        // The notes window was on the Dell's right half; it lands on the
        // panel's right half, inside the panel.
        assert_eq!(
            p.geometry.get(&2),
            Some(&Geometry {
                at: [768, 60],
                size: [720, 480]
            })
        );
        assert!(spawn_call(&p, "obsidian").contains("move = {768, 60}, size = {720, 480}"));
        // Focus follows the workspaces: the Dell's focus goes to the panel.
        assert!(p
            .steps
            .iter()
            .any(|s| matches!(s, Step::Focus { call } if call.contains("workspace = '3'"))));
        assert!(!p
            .steps
            .iter()
            .any(|s| matches!(s, Step::Focus { call } if call.contains("'DP-1'"))));
    }

    #[test]
    fn docked_to_undocked_with_collapse_says_so() {
        let laptop = monitors("laptop");
        let p = plan(
            &docked_session(),
            &[],
            Some(&laptop),
            &with_missing(MissingMonitor::Collapse),
            &RestoreOptions::default(),
            false,
        );
        let m = moves(&p);
        assert_eq!(m[1].1, "eDP-1");
        assert!(m[1].2.as_deref().unwrap().contains("collapsed"), "{m:?}");
    }

    #[test]
    fn undocked_to_docked_keeps_everything_where_it_was() {
        let laptop = monitors("laptop");
        let docked = monitors("docked");
        let mut s = session(vec![saved("foot", (1, "1"), 0), saved("code", (2, "2"), 0)]);
        s.monitors = laptop.iter().map(saved_monitor).collect();
        for missing in [
            MissingMonitor::PreserveNumbers,
            MissingMonitor::Remap,
            MissingMonitor::Collapse,
        ] {
            let p = plan(
                &s,
                &[],
                Some(&docked),
                &with_missing(missing),
                &RestoreOptions::default(),
                false,
            );
            let m = moves(&p);
            assert!(
                m.iter().all(|(_, monitor, _)| monitor == "eDP-1"),
                "{missing:?}: {m:?}"
            );
            assert!(!p
                .steps
                .iter()
                .any(|s| matches!(s, Step::MonitorMissing { .. })));
        }
    }

    #[test]
    fn the_same_display_on_another_port_is_followed_and_its_windows_shifted() {
        let elsewhere = monitors("docked-elsewhere");
        let p = plan(
            &docked_session(),
            &[],
            Some(&elsewhere),
            &with_missing(MissingMonitor::Remap),
            &RestoreOptions::default(),
            false,
        );
        let m = moves(&p);
        let dell = m.iter().find(|(ws, _, _)| ws == "3").unwrap();
        assert_eq!(dell.1, "DP-2");
        assert!(dell.2.as_deref().unwrap().contains("serial"), "{m:?}");
        // The Dell is now left of the panel, at x=0: same place on it.
        assert_eq!(p.geometry.get(&2).unwrap().at, [1280, 100]);
    }

    #[test]
    fn a_0_1_snapshot_is_never_fitted() {
        let laptop = monitors("laptop");
        let mut s = docked_session();
        s.loaded_from = Some(1);
        let p = plan(
            &s,
            &[],
            Some(&laptop),
            &with_missing(MissingMonitor::Remap),
            &RestoreOptions::default(),
            false,
        );
        assert!(p.geometry.is_empty());
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
