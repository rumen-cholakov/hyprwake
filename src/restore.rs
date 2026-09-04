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
use crate::session::{BrowserProfile, Session, SessionClient};
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
    let mut report = RestoreReport::default();

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

    // Tiled windows first, in their old reading order, so the layout re-tiles
    // roughly as it was; floating windows are positioned explicitly anyway.
    let mut order: Vec<&SessionClient> = session.clients.iter().collect();
    order.sort_by(|a, b| {
        a.floating
            .cmp(&b.floating)
            .then(a.workspace.selector().cmp(&b.workspace.selector()))
            .then(a.at[1].cmp(&b.at[1]))
            .then(a.at[0].cmp(&b.at[0]))
    });

    // What is already on screen, by class, so a merge can tell "this window
    // is back" from "this window is still missing".
    let mut already: HashMap<String, usize> = HashMap::new();
    if opts.missing_only {
        for window in &initial {
            *already.entry(window.class.clone()).or_default() += 1;
        }
    }

    for client in &order {
        if !client.launch.spawn {
            continue;
        }
        if opts.missing_only {
            if let Some(count) = already.get_mut(&client.class) {
                if *count > 0 {
                    *count -= 1;
                    report.already_open += 1;
                    if opts.verbose {
                        report
                            .details
                            .push(format!("already open: {}", client.class));
                    }
                    continue;
                }
            }
        }
        let command = launch_command(&client.launch.argv, use_uwsm);
        let call = exec_call(&command, &client.workspace, client);
        if opts.dry_run {
            report
                .details
                .push(format!("[dry-run] {} → {}", client.class, command));
            report.details.push(format!("  hyprctl dispatch {call}"));
            report.spawned += 1;
            continue;
        }
        match hyprctl.dispatch(&call) {
            Ok(()) => {
                report.spawned += 1;
                if opts.verbose {
                    report.details.push(format!(
                        "spawn {} → {}",
                        client.class,
                        client.workspace.selector()
                    ));
                }
            }
            Err(e) => {
                report.failed += 1;
                report
                    .details
                    .push(format!("FAIL spawn {}: {e}", client.class));
                log(format!("spawn failed for {}: {e}", client.class));
            }
        }
        sleep(Duration::from_millis(config.general.spawn_stagger_ms));
    }

    for profile in &session.browser_profiles {
        restore_browser_profile(profile, hyprctl, config, opts, use_uwsm, &mut report);
    }

    if opts.dry_run {
        restore_workspace_monitors(session, hyprctl, opts, &mut report);
        report_groups(session, opts, &mut report);
        restore_focus(session, &HashMap::new(), hyprctl, opts, &mut report);
        return Ok(report);
    }

    let paired = sweep(session, hyprctl, config, opts, initial, &mut report)?;
    restore_workspace_monitors(session, hyprctl, opts, &mut report);
    report_groups(session, opts, &mut report);
    restore_focus(session, &paired, hyprctl, opts, &mut report);
    log(format!(
        "restore done: spawned={} placed={} failed={} unmatched={}",
        report.spawned, report.placed, report.failed, report.unmatched
    ));
    Ok(report)
}

fn restore_browser_profile(
    profile: &BrowserProfile,
    hyprctl: &dyn HyprctlClient,
    config: &Config,
    opts: &RestoreOptions,
    use_uwsm: bool,
    report: &mut RestoreReport,
) {
    let Some(browser) = config.browsers.get(&profile.class) else {
        return;
    };
    let argv = crate::browsers::profile_argv(browser, profile);
    let command = launch_command(&argv, use_uwsm);
    let workspace = WorkspaceRef::new(
        profile.workspace.parse().unwrap_or(0),
        profile.workspace.clone(),
    );
    let call = format!(
        "hl.dsp.exec_cmd({}, {{ workspace = {} }})",
        lua_long_str(&command),
        lua_str(&format!("{} silent", workspace.selector()))
    );

    if opts.dry_run {
        report.details.push(format!(
            "[dry-run] {} profile \"{}\" → ws {}",
            profile.class, profile.name, profile.workspace
        ));
        report.details.push(format!("  hyprctl dispatch {call}"));
        report.spawned += 1;
        return;
    }

    match hyprctl.dispatch(&call) {
        Ok(()) => report.spawned += 1,
        Err(e) => {
            report.failed += 1;
            report
                .details
                .push(format!("FAIL profile {}: {e}", profile.name));
        }
    }
    sleep(Duration::from_millis(config.general.spawn_stagger_ms));
}

/// Pair windows that appeared after the spawns with their saved entries and
/// place the ones the exec rules could not reach.
fn sweep(
    session: &Session,
    hyprctl: &dyn HyprctlClient,
    config: &Config,
    opts: &RestoreOptions,
    initial: Vec<crate::hyprctl::HyprClient>,
    report: &mut RestoreReport,
) -> Result<HashMap<usize, String>, RestoreError> {
    // Which live window each saved client turned into, so later steps can
    // address them -- restoring focus needs to name a window.
    let mut paired: HashMap<usize, String> = HashMap::new();
    let index_of: HashMap<*const SessionClient, usize> = session
        .clients
        .iter()
        .enumerate()
        .map(|(i, c)| (c as *const SessionClient, i))
        .collect();
    let mut pending: Vec<&SessionClient> = session.clients.iter().collect();
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
    Ok(paired)
}

/// Put the desktop back the way it looked: each monitor on its workspace,
/// and the focus where it was.
///
/// Runs last, after every window is in place. Anything earlier would be
/// undone by the placements that follow.
fn restore_focus(
    session: &Session,
    paired: &HashMap<usize, String>,
    hyprctl: &dyn HyprctlClient,
    opts: &RestoreOptions,
    report: &mut RestoreReport,
) {
    let mut calls: Vec<String> = Vec::new();

    for monitor in &session.monitors {
        let Some(workspace) = &monitor.active_workspace else {
            continue;
        };
        // A special workspace is an overlay, not a resting state.
        if workspace.is_special() {
            continue;
        }
        calls.push(format!(
            "hl.dsp.focus({{ monitor = {} }})",
            lua_str(&monitor.name)
        ));
        calls.push(format!(
            "hl.dsp.focus({{ workspace = {} }})",
            lua_str(&workspace.selector())
        ));
    }

    // Focusing the window last also brings its monitor and workspace forward,
    // so the session ends where the user left it.
    if let Some(address) = session.focused_client().and_then(|i| paired.get(&i)) {
        calls.push(format!(
            "hl.dsp.focus({{ window = {} }})",
            lua_str(&format!("address:{address}"))
        ));
    }

    for call in calls {
        if opts.dry_run {
            report.details.push(format!("  hyprctl dispatch {call}"));
            continue;
        }
        match hyprctl.dispatch(&call) {
            Ok(()) => report.focus_restored = true,
            Err(e) => log(format!("focus: {e}")),
        }
    }
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

/// Send each workspace back to the monitor it was on.
///
/// Runs after the windows exist: Hyprland has no workspace to move until
/// something is on it. Monitors that are no longer attached are skipped --
/// a session saved on a docked laptop must still restore on the road.
fn restore_workspace_monitors(
    session: &Session,
    hyprctl: &dyn HyprctlClient,
    opts: &RestoreOptions,
    report: &mut RestoreReport,
) {
    let wanted = workspace_monitors(session);
    if wanted.is_empty() {
        return;
    }
    let attached: HashSet<String> = match hyprctl.get_monitors() {
        Ok(monitors) => monitors.into_iter().map(|m| m.name).collect(),
        Err(e) => {
            log(format!("monitors: query failed: {e}"));
            return;
        }
    };

    for (workspace, monitor) in wanted {
        if !attached.contains(&monitor) {
            let msg = format!("monitor {monitor} is not attached; leaving workspace {workspace}");
            log(msg.clone());
            if opts.verbose {
                report.details.push(msg);
            }
            continue;
        }
        let call = workspace_move_call(&workspace, &monitor);
        if opts.dry_run {
            report.details.push(format!("  hyprctl dispatch {call}"));
            continue;
        }
        match hyprctl.dispatch(&call) {
            Ok(()) => {
                report.monitors_placed += 1;
                if opts.verbose {
                    report
                        .details
                        .push(format!("workspace {workspace} → {monitor}"));
                }
            }
            Err(e) => log(format!("workspace {workspace} → {monitor} failed: {e}")),
        }
    }
}

fn report_groups(session: &Session, opts: &RestoreOptions, report: &mut RestoreReport) {
    let sets = grouped_sets(session);
    if sets.is_empty() {
        return;
    }
    report.groups_unrestored = sets.len();
    for (group, members) in &sets {
        let classes: Vec<&str> = members
            .iter()
            .map(|i| session.clients[*i].class.as_str())
            .collect();
        let msg = format!(
            "group {group} ({}) restored as separate windows: Hyprland has no \
             dispatcher to regroup them",
            classes.join(", ")
        );
        log(msg.clone());
        if opts.verbose || opts.dry_run {
            report.details.push(msg);
        }
    }
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
    use crate::session::LaunchInfo;
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
            launch: LaunchInfo {
                argv: vec![class.to_string()],
                spawn: true,
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
        c.launch.use_uwsm = Some(false);
        c
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
