//! Bringing a saved session back.
//!
//! Windows are not launched and then chased. Each program is started through
//! the compositor's own `exec_cmd` dispatcher carrying window rules, so
//! Hyprland places the window the moment it maps — on the right workspace,
//! silently, without stealing focus.
//!
//! Rules only reach windows the compositor can attribute to the process it
//! spawned. Single-instance programs (Electron apps, browsers, Obsidian)
//! hand the request to an existing process and the new window arrives
//! unattributed, so a sweep afterwards pairs stray windows with their saved
//! entry by class and places them explicitly.

use crate::config::Config;
use crate::hyprctl::{HyprctlClient, HyprctlError};
use crate::logging::log;
use crate::lua::{lua_long_str, lua_str, shell_join};
pub use crate::plan::RestorePlan;
use crate::plan::Step;
use crate::session::{Session, SessionClient};
use crate::workspace::WorkspaceRef;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::thread::sleep;
use std::time::{Duration, Instant};

#[derive(Debug, thiserror::Error)]
pub enum RestoreError {
    #[error("hyprctl error: {0}")]
    Hyprctl(#[from] HyprctlError),
    #[error("desktop already has {0} windows open; refusing to restore over it (use --force)")]
    DesktopPopulated(usize),
}

#[derive(Debug, Default, Clone)]
pub struct RestoreOptions {
    pub dry_run: bool,
    pub verbose: bool,
    /// Restore even though windows are already open.
    pub force: bool,
    /// Launch only what is not already open, and leave existing windows
    /// where they are.
    pub missing_only: bool,
}

#[derive(Debug, Default)]
pub struct RestoreReport {
    pub spawned: usize,
    pub placed: usize,
    pub failed: usize,
    pub unmatched: usize,
    pub monitors_placed: usize,
    /// Groups whose members were restored but not regrouped.
    pub groups_unrestored: usize,
    /// Whether the view and focus were put back.
    pub focus_restored: bool,
    /// Windows the session wanted that were already open.
    pub already_open: usize,
    pub details: Vec<String>,
}

pub fn restore_session(
    session: &Session,
    hyprctl: &dyn HyprctlClient,
    config: &Config,
    opts: &RestoreOptions,
) -> Result<RestoreReport, RestoreError> {
    let initial = live_windows(hyprctl, config)?;
    // Merging is the answer to a populated desktop, so it does not trip the
    // guard that exists to stop one being duplicated.
    if !opts.force
        && !opts.missing_only
        && !opts.dry_run
        && initial.len() > config.general.abort_restore_above
    {
        log(format!(
            "restore aborted: desktop already has {} windows",
            initial.len()
        ));
        return Err(RestoreError::DesktopPopulated(initial.len()));
    }

    let use_uwsm = config.use_uwsm();
    log(format!(
        "restore: {} saved windows, launching {}{}",
        session.clients.len(),
        session.spawn_count() + session.browser_profiles.len(),
        if use_uwsm { " via uwsm-app" } else { "" }
    ));

    let plan = plan_against(session, &initial, hyprctl, config, opts, use_uwsm)?;
    if opts.dry_run {
        return Ok(crate::plan::render(&plan, opts));
    }
    let report = execute(&plan, session, hyprctl, config, opts, initial);
    log(format!(
        "restore done: spawned={} placed={} failed={} unmatched={}",
        report.spawned, report.placed, report.failed, report.unmatched
    ));
    Ok(report)
}

/// The plan a restore would carry out right now, for `restore --dry-run
/// --json`. Reads the desktop; changes nothing.
pub fn plan_restore(
    session: &Session,
    hyprctl: &dyn HyprctlClient,
    config: &Config,
    opts: &RestoreOptions,
) -> Result<RestorePlan, RestoreError> {
    let initial = live_windows(hyprctl, config)?;
    plan_against(session, &initial, hyprctl, config, opts, config.use_uwsm())
}

fn plan_against(
    session: &Session,
    initial: &[crate::hyprctl::HyprClient],
    hyprctl: &dyn HyprctlClient,
    config: &Config,
    opts: &RestoreOptions,
    use_uwsm: bool,
) -> Result<RestorePlan, RestoreError> {
    let monitors = match hyprctl.get_monitors() {
        Ok(monitors) => Some(monitors),
        Err(e) => {
            log(format!("monitors: query failed: {e}"));
            None
        }
    };
    Ok(crate::plan::plan(
        session,
        initial,
        monitors.as_deref(),
        config,
        opts,
        use_uwsm,
    ))
}

/// Carry a plan out. Every dispatch, sleep and poll of a restore is here.
fn execute(
    plan: &RestorePlan,
    session: &Session,
    hyprctl: &dyn HyprctlClient,
    config: &Config,
    opts: &RestoreOptions,
    initial: Vec<crate::hyprctl::HyprClient>,
) -> RestoreReport {
    let mut report = RestoreReport {
        already_open: plan.already_open,
        groups_unrestored: plan.groups_unrestored,
        ..Default::default()
    };

    // Windows already on each workspace plus those this restore has launched
    // there, and the workspace of the last tiled launch that succeeded: the
    // tiled-order wait is judged on what happened, not on what was planned,
    // so a failed launch never becomes a wait for a window that will not come.
    let mut on_workspace: HashMap<String, usize> = HashMap::new();
    for window in &initial {
        *on_workspace.entry(window.workspace.selector()).or_default() += 1;
    }
    let mut last_tiled: Option<String> = None;
    let mut initial = Some(initial);
    let mut paired: HashMap<usize, String> = HashMap::new();

    for step in &plan.steps {
        match step {
            Step::AlreadyOpen { class, .. } => {
                if opts.verbose {
                    report.details.push(format!("already open: {class}"));
                }
            }
            Step::Spawn {
                class,
                workspace,
                tiled,
                wait_for_previous,
                call,
                ..
            } => {
                if *tiled
                    && *wait_for_previous
                    && config.general.tiled_map_timeout_ms > 0
                    && last_tiled.as_deref() == Some(workspace.as_str())
                {
                    let want = on_workspace.get(workspace).copied().unwrap_or(0);
                    if !wait_for_map(hyprctl, config, workspace, want) {
                        log(format!(
                            "tiled order: a window on {workspace} did not map within {}ms; \
                             launching the next anyway, so this pair may come back swapped",
                            config.general.tiled_map_timeout_ms
                        ));
                        report.details.push(format!(
                            "order not guaranteed on workspace {workspace}: previous window did not map in time"
                        ));
                    }
                }
                match hyprctl.dispatch(call) {
                    Ok(()) => {
                        report.spawned += 1;
                        if *tiled {
                            *on_workspace.entry(workspace.clone()).or_default() += 1;
                            last_tiled = Some(workspace.clone());
                        }
                        if opts.verbose {
                            report.details.push(format!("spawn {class} → {workspace}"));
                        }
                    }
                    Err(e) => {
                        report.failed += 1;
                        report.details.push(format!("FAIL spawn {class}: {e}"));
                        log(format!("spawn failed for {class}: {e}"));
                    }
                }
                sleep(Duration::from_millis(config.general.spawn_stagger_ms));
            }
            Step::SpawnProfile { name, call, .. } => {
                match hyprctl.dispatch(call) {
                    Ok(()) => report.spawned += 1,
                    Err(e) => {
                        report.failed += 1;
                        report.details.push(format!("FAIL profile {name}: {e}"));
                    }
                }
                sleep(Duration::from_millis(config.general.spawn_stagger_ms));
            }
            Step::Sweep { expect } => {
                let seen = initial.take().unwrap_or_default();
                paired = sweep(session, expect, hyprctl, config, opts, seen, &mut report);
            }
            Step::MoveWorkspace {
                workspace,
                monitor,
                call,
            } => match hyprctl.dispatch(call) {
                Ok(()) => {
                    report.monitors_placed += 1;
                    if opts.verbose {
                        report
                            .details
                            .push(format!("workspace {workspace} → {monitor}"));
                    }
                }
                Err(e) => log(format!("workspace {workspace} → {monitor} failed: {e}")),
            },
            Step::MonitorMissing { workspace, monitor } => {
                let msg = Step::monitor_missing_note(workspace, monitor);
                log(msg.clone());
                if opts.verbose {
                    report.details.push(msg);
                }
            }
            Step::GroupUnrestored { group, classes } => {
                let msg = Step::group_note(*group, classes);
                log(msg.clone());
                if opts.verbose {
                    report.details.push(msg);
                }
            }
            Step::Focus { call } => focus(hyprctl, call, &mut report),
            Step::FocusWindow { client } => {
                if let Some(address) = paired.get(client) {
                    let call = format!(
                        "hl.dsp.focus({{ window = {} }})",
                        lua_str(&format!("address:{address}"))
                    );
                    focus(hyprctl, &call, &mut report);
                }
            }
        }
    }
    report
}

fn focus(hyprctl: &dyn HyprctlClient, call: &str, report: &mut RestoreReport) {
    match hyprctl.dispatch(call) {
        Ok(()) => report.focus_restored = true,
        Err(e) => log(format!("focus: {e}")),
    }
}

/// Pair windows that appeared after the spawns with their saved entries and
/// place the ones the exec rules could not reach.
fn sweep(
    session: &Session,
    expect: &[usize],
    hyprctl: &dyn HyprctlClient,
    config: &Config,
    opts: &RestoreOptions,
    initial: Vec<crate::hyprctl::HyprClient>,
    report: &mut RestoreReport,
) -> HashMap<usize, String> {
    // Which live window each saved client turned into, so later steps can
    // address them -- restoring focus needs to name a window.
    let mut paired: HashMap<usize, String> = HashMap::new();
    let index_of: HashMap<*const SessionClient, usize> = session
        .clients
        .iter()
        .enumerate()
        .map(|(i, c)| (c as *const SessionClient, i))
        .collect();
    let mut pending: Vec<&SessionClient> = expect.iter().map(|&i| &session.clients[i]).collect();
    let mut seen: HashSet<String> = initial.into_iter().map(|c| c.address).collect();

    let deadline = Instant::now() + Duration::from_secs(config.general.sweep_timeout_secs);
    let poll = Duration::from_millis(config.general.sweep_poll_ms);

    while !pending.is_empty() && Instant::now() < deadline {
        sleep(poll);
        let now = match live_windows(hyprctl, config) {
            Ok(w) => w,
            Err(e) => {
                log(format!("sweep: query failed: {e}"));
                continue;
            }
        };

        for window in now {
            if seen.contains(&window.address) {
                continue;
            }
            // Prefer a saved window of the same class that already sits where
            // this one is: that pairing needs no correction.
            let Some(index) = pick_match(&pending, &window) else {
                continue;
            };
            let saved = pending.remove(index);
            seen.insert(window.address.clone());
            if let Some(i) = index_of.get(&(saved as *const SessionClient)) {
                paired.insert(*i, window.address.clone());
            }

            if !needs_placement(saved, &window) {
                continue;
            }
            let calls = fix_calls(saved, &window.address);
            let mut ok = true;
            for call in &calls {
                if let Err(e) = hyprctl.dispatch(call) {
                    ok = false;
                    log(format!("place failed for {}: {e}", saved.class));
                    if opts.verbose {
                        report
                            .details
                            .push(format!("FAIL place {}: {e}", saved.class));
                    }
                }
            }
            if ok {
                report.placed += 1;
                if opts.verbose {
                    report.details.push(format!(
                        "place {} → {}",
                        saved.class,
                        saved.workspace.selector()
                    ));
                }
            } else {
                report.failed += 1;
            }
        }
    }

    report.unmatched = pending.len();
    paired
}

/// Index into `pending` of the best saved entry for `window`.
///
/// Matching is by window class, preferring an entry that is already on this
/// window's workspace. Failing that, the class each window announced when it
/// first mapped is tried: applications that rename themselves afterwards --
/// and some XWayland windows -- would otherwise never be paired, and would
/// be reported as never having appeared while sitting on screen.
fn pick_match(pending: &[&SessionClient], window: &crate::hyprctl::HyprClient) -> Option<usize> {
    let by_class = |saved: &SessionClient| saved.class == window.class;
    let by_initial = |saved: &SessionClient| {
        !window.initial_class.is_empty()
            && (saved.initial_class == window.initial_class
                || saved.class == window.initial_class
                || (!saved.initial_class.is_empty() && saved.initial_class == window.class))
    };

    for matches in [&by_class as &dyn Fn(&SessionClient) -> bool, &by_initial] {
        let mut fallback = None;
        for (i, saved) in pending.iter().enumerate() {
            if !matches(saved) {
                continue;
            }
            if saved.workspace.same(&window.workspace) {
                return Some(i);
            }
            fallback.get_or_insert(i);
        }
        if fallback.is_some() {
            return fallback;
        }
    }
    None
}

/// Whether a window that arrived on its own still needs correcting.
fn needs_placement(saved: &SessionClient, window: &crate::hyprctl::HyprClient) -> bool {
    let misplaced = !saved.workspace.same(&window.workspace);
    let geometry_off =
        saved.floating && (!window.floating || window.at != saved.at || window.size != saved.size);
    misplaced || geometry_off || saved.fullscreen > 0 || saved.pinned
}

/// The window groups recorded in a session, as sets of client indices.
///
/// Hyprland 0.56 offers no way to put a window into an existing group by
/// address: `into_group` acts on the focused window and takes a direction,
/// and the `group` spawn rule joins whatever is focused, which a silent
/// restore never is. Members therefore come back on the right workspace, in
/// their old order, but ungrouped -- and the restore says so rather than
/// leaving the user to notice.
pub fn grouped_sets(session: &Session) -> BTreeMap<u32, Vec<usize>> {
    let mut sets: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
    for (index, client) in session.clients.iter().enumerate() {
        if let Some(group) = client.group {
            sets.entry(group).or_default().push(index);
        }
    }
    sets.retain(|_, members| members.len() > 1);
    sets
}

/// Which monitor each workspace belonged to, decided by majority of the
/// windows saved on it.
///
/// A workspace lives on one monitor, but a session can disagree with itself
/// if a window moved while the snapshot was being taken, so the common answer
/// wins. Special workspaces are excluded: they are overlays on whichever
/// monitor summons them, not residents of one.
pub fn workspace_monitors(session: &Session) -> BTreeMap<String, String> {
    let mut votes: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
    for client in &session.clients {
        if client.workspace.is_special() || client.monitor.is_empty() {
            continue;
        }
        *votes
            .entry(client.workspace.selector())
            .or_default()
            .entry(client.monitor.clone())
            .or_default() += 1;
    }
    votes
        .into_iter()
        .filter_map(|(workspace, counts)| {
            counts
                .into_iter()
                // Ties break on the monitor name so the result is stable.
                .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)))
                .map(|(monitor, _)| (workspace, monitor))
        })
        .collect()
}

pub fn workspace_move_call(workspace: &str, monitor: &str) -> String {
    format!(
        "hl.dsp.workspace.move({{ workspace = {}, monitor = {} }})",
        lua_str(workspace),
        lua_str(monitor)
    )
}

// ── Call builders (pure, unit-tested) ──────────────────────────────────────

/// The shell command that starts this window's program.
pub fn launch_command(argv: &[String], use_uwsm: bool) -> String {
    let joined = shell_join(argv);
    if use_uwsm {
        // uwsm-app puts the program in its own systemd scope, matching how a
        // uwsm-managed session starts applications in the first place.
        format!("uwsm-app -- {joined}")
    } else {
        joined
    }
}

/// `exec_cmd` with the window rules that place the window as it maps.
pub fn exec_call(command: &str, workspace: &WorkspaceRef, client: &SessionClient) -> String {
    let mut rules = vec![format!(
        "workspace = {}",
        lua_str(&format!("{} silent", workspace.selector()))
    )];
    if client.floating {
        rules.push("float = true".to_string());
        rules.push(format!("move = {{{}, {}}}", client.at[0], client.at[1]));
        rules.push(format!("size = {{{}, {}}}", client.size[0], client.size[1]));
    }
    if client.pinned {
        rules.push("pin = true".to_string());
    }
    format!(
        "hl.dsp.exec_cmd({}, {{ {} }})",
        lua_long_str(command),
        rules.join(", ")
    )
}

/// Dispatcher calls that move an existing window to its saved state.
pub fn fix_calls(client: &SessionClient, address: &str) -> Vec<String> {
    let target = lua_str(&format!("address:{address}"));
    let mut calls = vec![format!(
        "hl.dsp.window.move({{ workspace = {}, follow = false, window = {} }})",
        lua_str(&client.workspace.selector()),
        target
    )];
    if client.floating {
        calls.push(format!(
            "hl.dsp.window.float({{ action = 'on', window = {target} }})"
        ));
        // Size before position, and position last. A window that has just
        // been floated is still settling, and Hyprland re-centres it as the
        // resize completes; moving afterwards is what makes the placement
        // stick. `relative = false` pins the coordinates as absolute rather
        // than as an offset from wherever the window currently sits.
        calls.push(format!(
            "hl.dsp.window.resize({{ x = {}, y = {}, window = {} }})",
            client.size[0], client.size[1], target
        ));
        calls.push(format!(
            "hl.dsp.window.move({{ x = {}, y = {}, relative = false, window = {} }})",
            client.at[0], client.at[1], target
        ));
    }
    if client.pinned {
        calls.push(format!(
            "hl.dsp.window.pin({{ action = 'on', window = {target} }})"
        ));
    }
    if client.fullscreen > 0 {
        // Bit 1 is real fullscreen; bit 0 alone is the maximized state.
        let mode = if client.fullscreen & 2 != 0 {
            "fullscreen"
        } else {
            "maximized"
        };
        calls.push(format!(
            "hl.dsp.window.fullscreen({{ mode = '{mode}', action = 'set', window = {target} }})"
        ));
    }
    calls
}

/// How often `wait_for_map` looks. Short, because the thing being waited on
/// is usually a terminal that maps in a few milliseconds.
const MAP_POLL_MS: u64 = 25;

/// Wait until `workspace` holds at least `want` restorable windows.
///
/// Returns false on timeout, which the caller reports rather than treats as
/// failure: a program that never maps must not hold up everything after it.
fn wait_for_map(
    hyprctl: &dyn HyprctlClient,
    config: &Config,
    workspace: &str,
    want: usize,
) -> bool {
    let deadline = Instant::now() + Duration::from_millis(config.general.tiled_map_timeout_ms);
    loop {
        let here = live_windows(hyprctl, config)
            .map(|ws| {
                ws.iter()
                    .filter(|w| w.workspace.selector() == workspace)
                    .count()
            })
            .unwrap_or(0);
        if here >= want {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        sleep(Duration::from_millis(MAP_POLL_MS));
    }
}

fn live_windows(
    hyprctl: &dyn HyprctlClient,
    config: &Config,
) -> Result<Vec<crate::hyprctl::HyprClient>, RestoreError> {
    Ok(hyprctl
        .get_clients()?
        .into_iter()
        .filter(|c| c.is_restorable() && !config.is_ignored(&c.class))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hyprctl::mock::MockHyprctl;
    use crate::hyprctl::HyprClient;
    use crate::session::{BrowserProfile, LaunchInfo};
    use chrono::Utc;

    fn saved(class: &str, ws: (i32, &str)) -> SessionClient {
        SessionClient {
            class: class.to_string(),
            initial_class: class.to_string(),
            title: String::new(),
            workspace: WorkspaceRef::new(ws.0, ws.1),
            monitor: "eDP-1".to_string(),
            at: [10, 20],
            size: [800, 600],
            floating: false,
            pinned: false,
            fullscreen: 0,
            focus_history_id: 0,
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

    fn live_renamed(class: &str, initial: &str, ws: (i32, &str), addr: &str) -> HyprClient {
        let mut c = live(class, ws, addr);
        c.initial_class = initial.to_string();
        c
    }

    fn live(class: &str, ws: (i32, &str), addr: &str) -> HyprClient {
        HyprClient {
            address: addr.to_string(),
            class: class.to_string(),
            initial_class: class.to_string(),
            title: String::new(),
            workspace: WorkspaceRef::new(ws.0, ws.1),
            monitor: 0,
            at: [10, 20],
            size: [800, 600],
            floating: false,
            pinned: false,
            fullscreen: 0,
            focus_history_id: 0,
            pid: 1,
            mapped: true,
            grouped: vec![],
        }
    }

    fn session_of(clients: Vec<SessionClient>) -> Session {
        Session {
            name: "latest".to_string(),
            schema_version: crate::session::SCHEMA_VERSION,
            created_by: env!("CARGO_PKG_VERSION").to_string(),
            loaded_from: None,
            workspaces: vec![],
            created_at: Utc::now(),
            hyprland_version: "0.56.2".to_string(),
            monitors: vec![],
            clients,
            browser_profiles: vec![],
        }
    }

    fn fast_config() -> Config {
        let mut c: Config = toml::from_str("").unwrap();
        c.general.spawn_stagger_ms = 0;
        c.general.sweep_poll_ms = 0;
        c.general.sweep_timeout_secs = 0;
        c.general.tiled_map_timeout_ms = 0;
        c.launch.use_uwsm = Some(false);
        c
    }

    // ── tiled order ──
    //
    // Dwindle tiles in map order. These drive a compositor where each program
    // takes a given number of polls to map, and record the interleaving of
    // launches and polls so the order can be asserted, not inferred.

    use std::cell::RefCell;

    /// A compositor where each program maps `delay` polls after it launches.
    struct SlowMaps {
        delays: HashMap<String, usize>,
        launched: RefCell<Vec<(String, WorkspaceRef, usize)>>,
        polls: RefCell<usize>,
        log: RefCell<Vec<String>>,
    }

    impl SlowMaps {
        fn new(delays: &[(&str, usize)]) -> Self {
            Self {
                delays: delays.iter().map(|(c, d)| (c.to_string(), *d)).collect(),
                launched: RefCell::new(vec![]),
                polls: RefCell::new(0),
                log: RefCell::new(vec![]),
            }
        }
        fn launches(&self) -> Vec<String> {
            self.log
                .borrow()
                .iter()
                .filter(|l| l.starts_with("launch"))
                .cloned()
                .collect()
        }
    }

    impl HyprctlClient for SlowMaps {
        fn get_clients(&self) -> Result<Vec<HyprClient>, crate::hyprctl::HyprctlError> {
            *self.polls.borrow_mut() += 1;
            let now = *self.polls.borrow();
            let mapped: Vec<HyprClient> = self
                .launched
                .borrow()
                .iter()
                .enumerate()
                .filter(|(_, (_, _, at))| now >= *at)
                .map(|(i, (class, ws, _))| live(class, (ws.id, &ws.name), &format!("0x{i}")))
                .collect();
            self.log
                .borrow_mut()
                .push(format!("poll sees {}", mapped.len()));
            Ok(mapped)
        }
        fn get_monitors(
            &self,
        ) -> Result<Vec<crate::hyprctl::HyprMonitor>, crate::hyprctl::HyprctlError> {
            Ok(vec![])
        }
        fn dispatch(&self, call: &str) -> Result<(), crate::hyprctl::HyprctlError> {
            let Some(class) = self
                .delays
                .keys()
                .find(|c| call.contains(&format!("[[{c}]]")))
            else {
                return Ok(());
            };
            let ws = if call.contains("'5 silent'") {
                WorkspaceRef::new(5, "5")
            } else {
                WorkspaceRef::new(6, "6")
            };
            let at = *self.polls.borrow() + self.delays[class];
            self.launched.borrow_mut().push((class.clone(), ws, at));
            self.log.borrow_mut().push(format!("launch {class}"));
            Ok(())
        }
        fn get_hyprland_version(&self) -> Result<String, crate::hyprctl::HyprctlError> {
            Ok("0.56.2".into())
        }
    }

    fn at(mut c: SessionClient, x: i32) -> SessionClient {
        c.at = [x, 30];
        c
    }

    fn ordered_config() -> Config {
        let mut c = fast_config();
        c.general.tiled_map_timeout_ms = 2_000;
        c
    }

    #[test]
    fn a_slow_window_launched_first_maps_before_the_next_one_launches() {
        // The bug: VS Code left, terminal right; after reboot, swapped. The
        // terminal was launched second but mapped first.
        let hypr = SlowMaps::new(&[("code", 5), ("foot", 0)]);
        let session = session_of(vec![
            at(saved("foot", (5, "5")), 771),
            at(saved("code", (5, "5")), 4),
        ]);
        restore_session(
            &session,
            &hypr,
            &ordered_config(),
            &RestoreOptions::default(),
        )
        .unwrap();

        let log = hypr.log.borrow().clone();
        let code = log
            .iter()
            .position(|l| l == "launch code")
            .expect("code launched");
        let foot = log
            .iter()
            .position(|l| l == "launch foot")
            .expect("foot launched");
        assert!(code < foot, "left window must launch first: {log:?}");
        assert!(
            log[code..foot].iter().any(|l| l == "poll sees 1"),
            "foot launched before code had mapped: {log:?}"
        );
    }

    #[test]
    fn without_waiting_the_race_is_real() {
        // The control. With no wait the fast window is launched while the slow
        // one is still unmapped — which is exactly the swap. If this ever
        // fails, the test above is not testing anything.
        let hypr = SlowMaps::new(&[("code", 5), ("foot", 0)]);
        let session = session_of(vec![
            at(saved("foot", (5, "5")), 771),
            at(saved("code", (5, "5")), 4),
        ]);
        restore_session(&session, &hypr, &fast_config(), &RestoreOptions::default()).unwrap();

        let log = hypr.log.borrow().clone();
        let code = log.iter().position(|l| l == "launch code").unwrap();
        let foot = log.iter().position(|l| l == "launch foot").unwrap();
        assert!(
            !log[code..foot].iter().any(|l| l == "poll sees 1"),
            "expected foot to launch before code mapped: {log:?}"
        );
    }

    #[test]
    fn different_workspaces_do_not_wait_on_each_other() {
        // Waiting is per workspace. A slow browser on 6 must not delay a
        // terminal on 5 — a restore should still take seconds, not minutes.
        let hypr = SlowMaps::new(&[("google-chrome", 1_000_000), ("foot", 0)]);
        let session = session_of(vec![
            at(saved("google-chrome", (6, "6")), 4),
            at(saved("foot", (5, "5")), 4),
        ]);
        let started = Instant::now();
        restore_session(
            &session,
            &hypr,
            &ordered_config(),
            &RestoreOptions::default(),
        )
        .unwrap();
        assert_eq!(hypr.launches().len(), 2);
        assert!(
            started.elapsed() < Duration::from_millis(1_500),
            "waited across workspaces"
        );
    }

    #[test]
    fn a_window_that_never_maps_does_not_block_the_restore() {
        // A program that crashes before mapping must cost one timeout, not the
        // rest of the session — and the report says the order is not assured.
        let hypr = SlowMaps::new(&[("code", 1_000_000), ("foot", 0)]);
        let mut cfg = fast_config();
        cfg.general.tiled_map_timeout_ms = 100;
        let session = session_of(vec![
            at(saved("foot", (5, "5")), 771),
            at(saved("code", (5, "5")), 4),
        ]);
        let report = restore_session(&session, &hypr, &cfg, &RestoreOptions::default()).unwrap();
        assert_eq!(hypr.launches(), vec!["launch code", "launch foot"]);
        assert!(
            report
                .details
                .iter()
                .any(|d| d.contains("order not guaranteed")),
            "{:?}",
            report.details
        );
    }

    // ── golden: the restore sequence as 0.1.3 + tiled-order emits it ──
    //
    // Recorded from the implementation before the plan/executor split, so the
    // split can be checked against what restore actually did rather than
    // against a reading of it: every dispatch in order, every report field and
    // every detail line, live and dry-run. Regenerate only on a deliberate
    // behaviour change, with HYPRWAKE_RECORD_GOLDEN=1, and say why.

    fn golden_config(sweep_secs: u64, tiled_ms: u64) -> Config {
        let mut c: Config = toml::from_str(
            r#"
[browsers.google-chrome]
binary = "google-chrome-stable"
local_state = "google-chrome/Local State"
profile_workspaces = { "Profile 1" = "3" }
"#,
        )
        .unwrap();
        c.general.spawn_stagger_ms = 0;
        c.general.sweep_poll_ms = 0;
        c.general.sweep_timeout_secs = sweep_secs;
        c.general.tiled_map_timeout_ms = tiled_ms;
        c.launch.use_uwsm = Some(false);
        c
    }

    fn monitor(name: &str, ws: (i32, &str), focused: bool) -> crate::session::Monitor {
        crate::session::Monitor {
            name: name.to_string(),
            width: 1920,
            height: 1080,
            transform: 0,
            active_workspace: Some(WorkspaceRef::new(ws.0, ws.1)),
            focused,
            make: String::new(),
            model: String::new(),
            serial: String::new(),
            scale: 1.0,
            position: [0, 0],
        }
    }

    /// Two tiled windows sharing a group, a pinned floating window, a
    /// profile-driven browser, a fullscreen window saved on a monitor that is
    /// no longer attached, and a scratchpad window.
    fn mixed_session() -> Session {
        let mut foot = at(saved("foot", (1, "1")), 771);
        foot.focus_history_id = 1;
        foot.group = Some(0);
        let mut code = at(saved("code", (1, "1")), 4);
        code.focus_history_id = 0;
        code.group = Some(0);
        let mut obsidian = saved("obsidian", (2, "2"));
        obsidian.floating = true;
        obsidian.pinned = true;
        obsidian.at = [100, 100];
        obsidian.size = [900, 700];
        obsidian.focus_history_id = 2;
        let mut chrome = saved("google-chrome", (3, "3"));
        chrome.launch.spawn = false;
        chrome.focus_history_id = 3;
        let mut mpv = saved("mpv", (10, "10"));
        mpv.fullscreen = 2;
        mpv.monitor = "HDMI-A-1".to_string();
        mpv.focus_history_id = 4;
        let mut scratch = saved("foot", (-99, "special:magic"));
        scratch.focus_history_id = 5;
        let mut s = session_of(vec![foot, code, obsidian, chrome, mpv, scratch]);
        s.monitors = vec![
            monitor("eDP-1", (1, "1"), true),
            monitor("HDMI-A-1", (10, "10"), false),
        ];
        s.browser_profiles = vec![BrowserProfile {
            class: "google-chrome".to_string(),
            directory: "Profile 1".to_string(),
            name: "Work".to_string(),
            workspace: "3".to_string(),
        }];
        s
    }

    fn mixed_arrivals() -> Vec<HyprClient> {
        let mut arrivals = vec![
            live("foot", (1, "1"), "0xa"),
            live("code", (9, "9"), "0xb"), // landed on the wrong workspace
            live("obsidian", (2, "2"), "0xc"), // tiled, but saved floating
            live("google-chrome", (3, "3"), "0xd"),
            live("mpv", (10, "10"), "0xe"), // saved fullscreen
            live("foot", (-99, "special:magic"), "0xf"),
        ];
        arrivals[0].at = [771, 30];
        arrivals
    }

    struct Case {
        name: &'static str,
        session: Session,
        frames: Vec<Vec<HyprClient>>,
        config: Config,
        opts: RestoreOptions,
        reject: bool,
    }

    fn golden_restore_cases() -> Vec<Case> {
        let verbose = RestoreOptions {
            verbose: true,
            ..Default::default()
        };
        vec![
            Case {
                name: "mixed-verbose",
                session: mixed_session(),
                frames: vec![vec![], mixed_arrivals()],
                config: golden_config(1, 0),
                opts: verbose.clone(),
                reject: false,
            },
            Case {
                name: "mixed-quiet",
                session: mixed_session(),
                frames: vec![vec![], mixed_arrivals()],
                config: golden_config(1, 0),
                opts: RestoreOptions::default(),
                reject: false,
            },
            Case {
                name: "missing-only",
                session: session_of(vec![
                    at(saved("foot", (1, "1")), 4),
                    at(saved("code", (1, "1")), 771),
                    saved("obsidian", (2, "2")),
                ]),
                frames: vec![
                    vec![live("foot", (1, "1"), "0x1")],
                    vec![
                        live("foot", (1, "1"), "0x1"),
                        live("code", (1, "1"), "0x2"),
                        live("obsidian", (2, "2"), "0x3"),
                    ],
                ],
                config: golden_config(1, 0),
                opts: RestoreOptions {
                    missing_only: true,
                    verbose: true,
                    ..Default::default()
                },
                reject: false,
            },
            Case {
                name: "rejected",
                session: session_of(vec![saved("foot", (1, "1")), saved("code", (2, "2"))]),
                frames: vec![vec![]],
                config: golden_config(0, 0),
                opts: verbose.clone(),
                reject: true,
            },
            Case {
                name: "tiled-wait",
                session: session_of(vec![
                    at(saved("foot", (5, "5")), 771),
                    at(saved("code", (5, "5")), 4),
                ]),
                frames: vec![
                    vec![],
                    vec![live("code", (5, "5"), "0xb")],
                    vec![live("code", (5, "5"), "0xb"), live("foot", (5, "5"), "0xa")],
                ],
                config: golden_config(1, 1_000),
                opts: verbose,
                reject: false,
            },
        ]
    }

    fn record(case: &Case, dry_run: bool) -> serde_json::Value {
        let mut hypr = MockHyprctl::new(case.frames.clone());
        if case.reject {
            hypr.reject = Some("nope".to_string());
        }
        let opts = RestoreOptions {
            dry_run,
            ..case.opts.clone()
        };
        let r = restore_session(&case.session, &hypr, &case.config, &opts).unwrap();
        serde_json::json!({
            "dispatched": hypr.calls(),
            "spawned": r.spawned,
            "placed": r.placed,
            "failed": r.failed,
            "unmatched": r.unmatched,
            "monitors_placed": r.monitors_placed,
            "groups_unrestored": r.groups_unrestored,
            "focus_restored": r.focus_restored,
            "already_open": r.already_open,
            "details": r.details,
        })
    }

    #[test]
    fn golden_restore_sequence_is_unchanged() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/restore-golden.json"
        );
        let mut got = serde_json::Map::new();
        for case in golden_restore_cases() {
            got.insert(
                case.name.to_string(),
                serde_json::json!({ "live": record(&case, false), "dry_run": record(&case, true) }),
            );
        }
        let got = serde_json::Value::Object(got);
        if std::env::var_os("HYPRWAKE_RECORD_GOLDEN").is_some() {
            std::fs::write(path, serde_json::to_string_pretty(&got).unwrap() + "\n").unwrap();
            return;
        }
        let want: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).expect("golden fixture")).unwrap();
        for (name, expected) in want.as_object().unwrap() {
            assert_eq!(
                &got[name], expected,
                "restore sequence drifted for case {name}"
            );
        }
        assert_eq!(got, want, "golden cases added or removed");
    }

    // ── command building ──

    #[test]
    fn launch_command_quotes_the_argv() {
        let argv = vec!["foot".to_string(), "-D".to_string(), "/my dir".to_string()];
        assert_eq!(launch_command(&argv, false), "foot -D '/my dir'");
    }

    #[test]
    fn uwsm_wraps_the_command() {
        let argv = vec!["foot".to_string()];
        assert_eq!(launch_command(&argv, true), "uwsm-app -- foot");
    }

    #[test]
    fn exec_call_places_tiled_windows_silently() {
        let c = saved("foot", (2, "2"));
        let call = exec_call("foot", &c.workspace, &c);
        assert_eq!(
            call,
            "hl.dsp.exec_cmd([[foot]], { workspace = '2 silent' })"
        );
    }

    #[test]
    fn exec_call_carries_geometry_for_floating_windows() {
        let mut c = saved("foot", (1, "1"));
        c.floating = true;
        let call = exec_call("foot", &c.workspace, &c);
        assert!(call.contains("float = true"));
        assert!(call.contains("move = {10, 20}"));
        assert!(call.contains("size = {800, 600}"));
    }

    #[test]
    fn exec_call_pins_pinned_windows() {
        let mut c = saved("foot", (1, "1"));
        c.pinned = true;
        assert!(exec_call("foot", &c.workspace, &c).contains("pin = true"));
    }

    #[test]
    fn exec_call_targets_scratchpads_by_name() {
        let c = saved("foot", (-99, "special:magic"));
        assert!(exec_call("foot", &c.workspace, &c).contains("'special:magic silent'"));
    }

    #[test]
    fn fix_calls_place_then_float_then_size_then_position() {
        let mut c = saved("foot", (3, "3"));
        c.floating = true;
        c.pinned = true;
        let calls = fix_calls(&c, "0xabc");
        assert!(calls[0].contains("window.move") && calls[0].contains("workspace = '3'"));
        assert!(calls[1].contains("window.float"));
        // Size first, position last: a freshly floated window is re-centred
        // by the resize, so moving before it would be undone.
        assert!(calls[2].contains("window.resize") && calls[2].contains("x = 800, y = 600"));
        assert!(calls[3].contains("window.move") && calls[3].contains("x = 10, y = 20"));
        assert!(calls[4].contains("window.pin"));
        assert!(calls.iter().all(|c| c.contains("address:0xabc")));
    }

    #[test]
    fn positioning_is_absolute_not_relative() {
        // Without relative = false the coordinates are an offset from the
        // window's current position, which lands it somewhere else entirely.
        let mut c = saved("foot", (1, "1"));
        c.floating = true;
        let calls = fix_calls(&c, "0x1");
        let move_call = calls.iter().find(|c| c.contains("x = 10, y = 20")).unwrap();
        assert!(move_call.contains("relative = false"));
    }

    #[test]
    fn fullscreen_bit_selects_the_mode() {
        let mut c = saved("foot", (1, "1"));
        c.fullscreen = 1;
        assert!(fix_calls(&c, "0x1").last().unwrap().contains("'maximized'"));
        c.fullscreen = 2;
        assert!(fix_calls(&c, "0x1")
            .last()
            .unwrap()
            .contains("'fullscreen'"));
    }

    #[test]
    fn tiled_windows_get_no_geometry_calls() {
        let calls = fix_calls(&saved("foot", (1, "1")), "0x1");
        assert_eq!(calls.len(), 1);
    }

    // ── matching ──

    #[test]
    fn match_prefers_the_window_already_in_place() {
        let a = saved("foot", (1, "1"));
        let b = saved("foot", (5, "5"));
        let pending = vec![&a, &b];
        let window = live("foot", (5, "5"), "0x1");
        assert_eq!(pick_match(&pending, &window), Some(1));
    }

    #[test]
    fn match_falls_back_to_any_window_of_the_class() {
        let a = saved("foot", (1, "1"));
        let pending = vec![&a];
        assert_eq!(
            pick_match(&pending, &live("foot", (9, "9"), "0x1")),
            Some(0)
        );
    }

    #[test]
    fn a_window_that_renamed_itself_still_matches() {
        // Saved as "obsidian"; it maps under that name and then renames.
        let a = saved("obsidian", (2, "2"));
        let pending = vec![&a];
        let window = live_renamed("obsidian-v2", "obsidian", (2, "2"), "0x1");
        assert_eq!(pick_match(&pending, &window), Some(0));
    }

    #[test]
    fn an_exact_class_match_wins_over_an_initial_class_one() {
        let exact = saved("code", (1, "1"));
        let renamed = saved("obsidian", (1, "1"));
        let pending = vec![&renamed, &exact];
        let window = live_renamed("code", "obsidian", (1, "1"), "0x1");
        assert_eq!(
            pick_match(&pending, &window),
            Some(1),
            "the window's current class is the stronger signal"
        );
    }

    #[test]
    fn windows_of_another_class_never_match() {
        let a = saved("foot", (1, "1"));
        let pending = vec![&a];
        assert_eq!(pick_match(&pending, &live("kitty", (1, "1"), "0x1")), None);
    }

    #[test]
    fn a_correctly_placed_window_needs_no_work() {
        assert!(!needs_placement(
            &saved("foot", (1, "1")),
            &live("foot", (1, "1"), "0x1")
        ));
    }

    #[test]
    fn a_window_on_the_wrong_workspace_needs_placement() {
        assert!(needs_placement(
            &saved("foot", (1, "1")),
            &live("foot", (4, "4"), "0x1")
        ));
    }

    #[test]
    fn a_floating_window_that_came_back_tiled_needs_placement() {
        let mut s = saved("foot", (1, "1"));
        s.floating = true;
        assert!(needs_placement(&s, &live("foot", (1, "1"), "0x1")));
    }

    #[test]
    fn pinned_and_fullscreen_states_are_always_reapplied() {
        let mut s = saved("foot", (1, "1"));
        s.pinned = true;
        assert!(needs_placement(&s, &live("foot", (1, "1"), "0x1")));
        s.pinned = false;
        s.fullscreen = 2;
        assert!(needs_placement(&s, &live("foot", (1, "1"), "0x1")));
    }

    // ── end to end against the mock compositor ──

    // ── merging into a live desktop ──

    fn merging() -> RestoreOptions {
        RestoreOptions {
            missing_only: true,
            ..Default::default()
        }
    }

    #[test]
    fn merging_launches_only_what_is_missing() {
        let session = session_of(vec![
            saved("foot", (1, "1")),
            saved("foot", (2, "2")),
            saved("google-chrome", (3, "3")),
        ]);
        // One foot is already up; the other foot and Chrome are not.
        let hypr = MockHyprctl::new(vec![vec![live("foot", (1, "1"), "0xlive")]]);
        let report = restore_session(&session, &hypr, &fast_config(), &merging()).unwrap();

        assert_eq!(report.already_open, 1);
        assert_eq!(report.spawned, 2);
        let spawns: Vec<_> = hypr
            .calls()
            .into_iter()
            .filter(|c| c.contains("exec_cmd"))
            .collect();
        assert_eq!(spawns.len(), 2);
        assert!(spawns.iter().any(|c| c.contains("[[google-chrome]]")));
    }

    #[test]
    fn merging_does_not_disturb_windows_that_are_already_open() {
        let session = session_of(vec![saved("foot", (1, "1"))]);
        // The live window sits on the wrong workspace by the session's
        // reckoning, but the user put it there.
        let hypr = MockHyprctl::new(vec![vec![live("foot", (7, "7"), "0xlive")]]);
        restore_session(&session, &hypr, &fast_config(), &merging()).unwrap();
        assert!(
            !hypr.calls().iter().any(|c| c.contains("window.move")),
            "an already-open window is left where the user has it"
        );
    }

    #[test]
    fn merging_is_allowed_on_a_populated_desktop() {
        let session = session_of(vec![saved("code", (1, "1"))]);
        let busy: Vec<HyprClient> = (0..6)
            .map(|i| live("foot", (1, "1"), &format!("0x{i}")))
            .collect();
        let hypr = MockHyprctl::new(vec![busy]);
        let report = restore_session(&session, &hypr, &fast_config(), &merging()).unwrap();
        assert_eq!(report.spawned, 1, "merging is the answer to a busy desktop");
    }

    #[test]
    fn a_second_window_of_a_class_is_still_launched() {
        // Two terminals were saved and one is open: the other is missing.
        let session = session_of(vec![saved("foot", (1, "1")), saved("foot", (2, "2"))]);
        let hypr = MockHyprctl::new(vec![vec![live("foot", (1, "1"), "0xlive")]]);
        let report = restore_session(&session, &hypr, &fast_config(), &merging()).unwrap();
        assert_eq!(report.already_open, 1);
        assert_eq!(report.spawned, 1);
    }

    // ── focus and the active view ──

    #[test]
    fn the_focused_client_is_the_lowest_focus_id() {
        let mut a = saved("foot", (1, "1"));
        let mut b = saved("code", (2, "2"));
        a.focus_history_id = 3;
        b.focus_history_id = 0;
        let session = session_of(vec![a, b]);
        assert_eq!(session.focused_client(), Some(1));
    }

    #[test]
    fn restore_puts_each_monitor_back_on_its_workspace() {
        let mut session = session_of(vec![saved("foot", (3, "3"))]);
        session.monitors = vec![crate::session::Monitor {
            name: "eDP-1".into(),
            width: 1920,
            height: 1080,
            transform: 0,
            active_workspace: Some(WorkspaceRef::new(3, "3")),
            focused: true,
            make: String::new(),
            model: String::new(),
            serial: String::new(),
            scale: 1.0,
            position: [0, 0],
        }];
        let hypr = MockHyprctl::new(vec![vec![]]);
        let report =
            restore_session(&session, &hypr, &fast_config(), &RestoreOptions::default()).unwrap();
        assert!(report.focus_restored);
        let calls = hypr.calls();
        assert!(calls
            .iter()
            .any(|c| c.contains("focus({ monitor = 'eDP-1' })")));
        assert!(calls
            .iter()
            .any(|c| c.contains("focus({ workspace = '3' })")));
    }

    #[test]
    fn a_scratchpad_is_not_restored_as_the_resting_view() {
        // A special workspace is an overlay; coming back with it open is not
        // where the user left off.
        let mut session = session_of(vec![saved("foot", (1, "1"))]);
        session.monitors = vec![crate::session::Monitor {
            name: "eDP-1".into(),
            width: 1920,
            height: 1080,
            transform: 0,
            active_workspace: Some(WorkspaceRef::new(-99, "special:magic")),
            focused: true,
            make: String::new(),
            model: String::new(),
            serial: String::new(),
            scale: 1.0,
            position: [0, 0],
        }];
        let hypr = MockHyprctl::new(vec![vec![]]);
        restore_session(&session, &hypr, &fast_config(), &RestoreOptions::default()).unwrap();
        assert!(!hypr.calls().iter().any(|c| c.contains("special:magic")));
    }

    #[test]
    fn the_focused_window_is_focused_last() {
        let mut config = fast_config();
        config.general.sweep_timeout_secs = 1;
        let mut a = saved("foot", (1, "1"));
        a.focus_history_id = 0;
        let session = session_of(vec![a]);
        // The window appears during the sweep, which is what lets focus name it.
        let hypr = MockHyprctl::new(vec![vec![], vec![live("foot", (1, "1"), "0xfocus")]]);
        restore_session(&session, &hypr, &config, &RestoreOptions::default()).unwrap();
        let calls = hypr.calls();
        let focus = calls.last().expect("a call was made");
        assert!(
            focus.contains("focus({ window = 'address:0xfocus' })"),
            "focus must come last, after every placement: {focus}"
        );
    }

    #[test]
    fn a_window_that_never_appeared_cannot_be_focused() {
        let mut a = saved("foot", (1, "1"));
        a.focus_history_id = 0;
        let session = session_of(vec![a]);
        let hypr = MockHyprctl::new(vec![vec![]]); // nothing ever maps
        restore_session(&session, &hypr, &fast_config(), &RestoreOptions::default()).unwrap();
        assert!(!hypr.calls().iter().any(|c| c.contains("focus({ window")));
    }

    // ── groups ──

    #[test]
    fn grouped_clients_are_collected_into_sets() {
        let mut a = saved("foot", (1, "1"));
        let mut b = saved("foot", (1, "1"));
        let c = saved("code", (2, "2"));
        a.group = Some(0);
        b.group = Some(0);
        let session = session_of(vec![a, b, c]);
        let sets = grouped_sets(&session);
        assert_eq!(sets.len(), 1);
        assert_eq!(sets.get(&0).unwrap(), &vec![0, 1]);
    }

    #[test]
    fn a_group_that_lost_all_but_one_member_is_not_a_group() {
        let mut a = saved("foot", (1, "1"));
        a.group = Some(3);
        assert!(grouped_sets(&session_of(vec![a])).is_empty());
    }

    #[test]
    fn restore_reports_groups_it_cannot_reassemble() {
        let mut a = saved("foot", (1, "1"));
        let mut b = saved("foot", (1, "1"));
        a.group = Some(0);
        b.group = Some(0);
        let session = session_of(vec![a, b]);
        let hypr = MockHyprctl::new(vec![vec![]]);
        let opts = RestoreOptions {
            verbose: true,
            ..Default::default()
        };
        let report = restore_session(&session, &hypr, &fast_config(), &opts).unwrap();
        assert_eq!(report.groups_unrestored, 1);
        assert!(
            report.details.iter().any(|d| d.contains("group 0")),
            "the user should be told the windows came back ungrouped"
        );
    }

    // ── multi-monitor ──

    fn on_monitor(class: &str, ws: (i32, &str), monitor: &str) -> SessionClient {
        let mut c = saved(class, ws);
        c.monitor = monitor.to_string();
        c
    }

    #[test]
    fn each_workspace_maps_to_its_monitor() {
        let session = session_of(vec![
            on_monitor("foot", (1, "1"), "eDP-1"),
            on_monitor("code", (2, "2"), "DP-1"),
            on_monitor("chrome", (2, "2"), "DP-1"),
        ]);
        let map = workspace_monitors(&session);
        assert_eq!(map.get("1").unwrap(), "eDP-1");
        assert_eq!(map.get("2").unwrap(), "DP-1");
    }

    #[test]
    fn a_disagreeing_workspace_takes_the_majority_monitor() {
        // A window can move mid-snapshot; the common answer should win.
        let session = session_of(vec![
            on_monitor("a", (3, "3"), "DP-1"),
            on_monitor("b", (3, "3"), "DP-1"),
            on_monitor("c", (3, "3"), "eDP-1"),
        ]);
        assert_eq!(workspace_monitors(&session).get("3").unwrap(), "DP-1");
    }

    #[test]
    fn special_workspaces_are_not_bound_to_a_monitor() {
        // A scratchpad appears on whichever monitor summons it.
        let session = session_of(vec![on_monitor("foot", (-99, "special:magic"), "DP-1")]);
        assert!(workspace_monitors(&session).is_empty());
    }

    #[test]
    fn workspaces_without_a_recorded_monitor_are_skipped() {
        let session = session_of(vec![on_monitor("foot", (1, "1"), "")]);
        assert!(workspace_monitors(&session).is_empty());
    }

    #[test]
    fn the_workspace_move_call_targets_both_by_name() {
        assert_eq!(
            workspace_move_call("2", "DP-1"),
            "hl.dsp.workspace.move({ workspace = '2', monitor = 'DP-1' })"
        );
    }

    #[test]
    fn restore_sends_workspaces_back_to_their_monitors() {
        let session = session_of(vec![
            on_monitor("foot", (1, "1"), "eDP-1"),
            on_monitor("code", (2, "2"), "eDP-1"),
        ]);
        let hypr = MockHyprctl::new(vec![vec![]]); // mock reports monitor eDP-1
        let report =
            restore_session(&session, &hypr, &fast_config(), &RestoreOptions::default()).unwrap();
        assert_eq!(report.monitors_placed, 2);
        let moves: Vec<_> = hypr
            .calls()
            .into_iter()
            .filter(|c| c.contains("workspace.move"))
            .collect();
        assert_eq!(moves.len(), 2);
        assert!(moves.iter().any(|c| c.contains("workspace = '1'")));
    }

    #[test]
    fn a_detached_monitor_is_left_alone() {
        // The session was saved docked; this restore is on the road.
        let session = session_of(vec![on_monitor("foot", (1, "1"), "DP-9-not-attached")]);
        let hypr = MockHyprctl::new(vec![vec![]]);
        let report =
            restore_session(&session, &hypr, &fast_config(), &RestoreOptions::default()).unwrap();
        assert_eq!(report.monitors_placed, 0);
        assert!(
            !hypr.calls().iter().any(|c| c.contains("workspace.move")),
            "nothing should be moved to a monitor that is not there"
        );
    }

    #[test]
    fn restore_spawns_every_window_once() {
        let session = session_of(vec![
            saved("foot", (1, "1")),
            saved("google-chrome", (2, "2")),
        ]);
        let hypr = MockHyprctl::new(vec![vec![]]);
        let report =
            restore_session(&session, &hypr, &fast_config(), &RestoreOptions::default()).unwrap();
        assert_eq!(report.spawned, 2);
        // Only the spawns: a restore also issues placement and monitor calls.
        let spawns: Vec<_> = hypr
            .calls()
            .into_iter()
            .filter(|c| c.contains("exec_cmd"))
            .collect();
        assert_eq!(spawns.len(), 2);
        assert!(spawns[0].contains("[[foot]]"));
        assert!(spawns[1].contains("[[google-chrome]]"));
    }

    #[test]
    fn restore_skips_windows_marked_no_spawn() {
        let mut second = saved("google-chrome", (2, "2"));
        second.launch.spawn = false;
        let session = session_of(vec![saved("google-chrome", (1, "1")), second]);
        let hypr = MockHyprctl::new(vec![vec![]]);
        let report =
            restore_session(&session, &hypr, &fast_config(), &RestoreOptions::default()).unwrap();
        assert_eq!(report.spawned, 1);
    }

    #[test]
    fn restore_refuses_to_run_over_a_populated_desktop() {
        let session = session_of(vec![saved("foot", (1, "1"))]);
        let busy: Vec<HyprClient> = (0..5)
            .map(|i| live("foot", (1, "1"), &format!("0x{i}")))
            .collect();
        let hypr = MockHyprctl::new(vec![busy]);
        let err = restore_session(&session, &hypr, &fast_config(), &RestoreOptions::default())
            .unwrap_err();
        assert!(matches!(err, RestoreError::DesktopPopulated(5)));
        assert!(
            hypr.calls().is_empty(),
            "nothing may be launched after the refusal"
        );
    }

    #[test]
    fn force_overrides_the_populated_desktop_check() {
        let session = session_of(vec![saved("foot", (1, "1"))]);
        let busy: Vec<HyprClient> = (0..5)
            .map(|i| live("foot", (1, "1"), &format!("0x{i}")))
            .collect();
        let hypr = MockHyprctl::new(vec![busy]);
        let opts = RestoreOptions {
            force: true,
            ..Default::default()
        };
        assert!(restore_session(&session, &hypr, &fast_config(), &opts).is_ok());
    }

    #[test]
    fn dry_run_dispatches_nothing_but_reports_everything() {
        let session = session_of(vec![saved("foot", (1, "1"))]);
        let hypr = MockHyprctl::new(vec![vec![]]);
        let opts = RestoreOptions {
            dry_run: true,
            ..Default::default()
        };
        let report = restore_session(&session, &hypr, &fast_config(), &opts).unwrap();
        assert!(hypr.calls().is_empty());
        assert_eq!(report.spawned, 1);
        assert!(report
            .details
            .iter()
            .any(|d| d.contains("hyprctl dispatch")));
    }

    #[test]
    fn a_failed_spawn_is_counted_and_reported() {
        let session = session_of(vec![saved("foot", (1, "1"))]);
        let mut hypr = MockHyprctl::new(vec![vec![]]);
        hypr.reject = Some("unknown dispatcher".to_string());
        let report =
            restore_session(&session, &hypr, &fast_config(), &RestoreOptions::default()).unwrap();
        assert_eq!(report.failed, 1);
        assert_eq!(report.spawned, 0);
        assert!(report.details[0].starts_with("FAIL spawn foot"));
    }

    #[test]
    fn the_sweep_places_a_window_that_arrived_on_the_wrong_workspace() {
        let mut config = fast_config();
        config.general.sweep_timeout_secs = 1;

        // The window shows up on workspace 1 although it was saved on 5:
        // the single-instance case the exec rules cannot reach.
        let session = session_of(vec![saved("google-chrome", (5, "5"))]);
        let hypr = MockHyprctl::new(vec![
            vec![],                                         // pre-spawn snapshot
            vec![live("google-chrome", (1, "1"), "0xnew")], // sweep sees it
        ]);
        let report = restore_session(&session, &hypr, &config, &RestoreOptions::default()).unwrap();

        assert_eq!(report.placed, 1);
        assert_eq!(report.unmatched, 0);
        let move_call = hypr
            .calls()
            .into_iter()
            .find(|c| c.contains("window.move"))
            .expect("the stray window must be moved");
        assert!(move_call.contains("workspace = '5'"));
        assert!(move_call.contains("address:0xnew"));
    }

    #[test]
    fn the_sweep_leaves_correctly_placed_windows_alone() {
        let mut config = fast_config();
        config.general.sweep_timeout_secs = 1;
        let session = session_of(vec![saved("foot", (3, "3"))]);
        let hypr = MockHyprctl::new(vec![vec![], vec![live("foot", (3, "3"), "0xnew")]]);
        let report = restore_session(&session, &hypr, &config, &RestoreOptions::default()).unwrap();
        assert_eq!(report.placed, 0);
        assert_eq!(report.unmatched, 0);
        assert!(!hypr.calls().iter().any(|c| c.contains("window.move")));
    }

    #[test]
    fn windows_that_never_appear_are_reported_as_unmatched() {
        let session = session_of(vec![saved("foot", (1, "1"))]);
        let hypr = MockHyprctl::new(vec![vec![]]);
        let report =
            restore_session(&session, &hypr, &fast_config(), &RestoreOptions::default()).unwrap();
        assert_eq!(report.unmatched, 1);
    }
}
