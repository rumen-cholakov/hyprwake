//! A privacy-preserving comparison between a saved session and live windows.

use crate::config::Config;
use crate::hyprctl::HyprClient;
use crate::layout::{self, LayoutOptions, Rect};
use crate::matching;
use crate::session::{Session, SessionClient};
use std::collections::{BTreeMap, HashMap};

#[derive(Debug, PartialEq, Eq)]
pub struct SessionDiff {
    pub missing: Vec<WindowCount>,
    pub unexpected: Vec<WindowCount>,
    /// Saved windows that are open, but on another workspace: what a restore
    /// would move rather than launch.
    pub moved: Vec<WindowMove>,
    /// Workspaces whose windows are all there but no longer tiled the way
    /// the saved tree says.
    pub layout: Vec<LayoutDrift>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct LayoutDrift {
    pub workspace: String,
    pub what: &'static str,
}

#[derive(Debug, PartialEq, Eq)]
pub struct WindowCount {
    pub class: String,
    pub workspace: String,
    pub count: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub struct WindowMove {
    pub class: String,
    pub from: String,
    pub to: String,
    pub count: usize,
}

/// Pair saved and live windows the way restore does, then report what is
/// left over. Output is counts by class and workspace only: titles take part
/// in pairing, as they do in restore, but never appear in the result, and
/// command lines and paths are not looked at.
///
/// With `layout`, each workspace that has a saved split tree is also compared
/// with the tree its windows are in now.
pub fn compare(
    saved: &Session,
    live: &[HyprClient],
    config: &Config,
    layout: Option<&LayoutOptions>,
) -> SessionDiff {
    let live: Vec<&HyprClient> = live
        .iter()
        .filter(|c| c.is_restorable() && !config.is_ignored(&c.class))
        .collect();
    let entries: Vec<&SessionClient> = saved.clients.iter().collect();
    let pairs = matching::pair_all(&entries, &live);

    let mut used = vec![false; entries.len()];
    let mut now: HashMap<usize, &HyprClient> = HashMap::new();
    let mut unexpected: BTreeMap<(String, String), usize> = BTreeMap::new();
    let mut moved: BTreeMap<(String, String, String), usize> = BTreeMap::new();
    for (window, pair) in live.iter().zip(pairs) {
        match pair {
            Some((i, _)) => {
                used[i] = true;
                now.insert(i, window);
                let entry = entries[i];
                if !entry.workspace.same(&window.workspace) {
                    *moved
                        .entry((
                            entry.class.clone(),
                            entry.workspace.selector(),
                            window.workspace.selector(),
                        ))
                        .or_default() += 1;
                }
            }
            None => {
                *unexpected
                    .entry((window.class.clone(), window.workspace.selector()))
                    .or_default() += 1;
            }
        }
    }
    let mut missing: BTreeMap<(String, String), usize> = BTreeMap::new();
    for (entry, _) in entries.iter().zip(&used).filter(|(_, used)| !**used) {
        *missing
            .entry((entry.class.clone(), entry.workspace.selector()))
            .or_default() += 1;
    }

    SessionDiff {
        missing: counts(missing),
        unexpected: counts(unexpected),
        layout: layout
            .map(|opts| layout_drift(saved, &now, opts))
            .unwrap_or_default(),
        moved: moved
            .into_iter()
            .map(|((class, from, to), count)| WindowMove {
                class,
                from,
                to,
                count,
            })
            .collect(),
    }
}

/// Workspaces whose saved tree no longer describes their windows.
///
/// Only where every window of the tree is open, tiled, on that workspace:
/// anything else is already reported as missing or moved, and a tree over
/// half its windows says nothing.
fn layout_drift(
    saved: &Session,
    now: &HashMap<usize, &HyprClient>,
    opts: &LayoutOptions,
) -> Vec<LayoutDrift> {
    let mut out = Vec::new();
    for ws in &saved.workspaces {
        let Some(tree) = &ws.layout else {
            continue;
        };
        let rects: Option<Vec<(usize, Rect)>> = layout::leaves(tree)
            .into_iter()
            .map(|i| {
                now.get(&i)
                    .filter(|w| w.workspace.same(&ws.workspace) && !w.floating)
                    .map(|w| (i, Rect::new(w.at, w.size)))
            })
            .collect();
        let Some(rects) = rects else {
            continue;
        };
        let what = match layout::infer(&rects, opts) {
            None => "its windows no longer tile it",
            Some(current) if layout::same(tree, &current.tree, 0.05) => continue,
            Some(current) if layout::same(tree, &current.tree, 1.0) => "split ratios differ",
            Some(_) => "arrangement differs",
        };
        out.push(LayoutDrift {
            workspace: ws.workspace.selector(),
            what,
        });
    }
    out
}

fn counts(map: BTreeMap<(String, String), usize>) -> Vec<WindowCount> {
    map.into_iter()
        .map(|((class, workspace), count)| WindowCount {
            class,
            workspace,
            count,
        })
        .collect()
}

impl SessionDiff {
    pub fn is_empty(&self) -> bool {
        self.missing.is_empty()
            && self.unexpected.is_empty()
            && self.moved.is_empty()
            && self.layout.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hyprctl::HyprClient;
    use crate::session::{LaunchInfo, SessionClient};
    use crate::workspace::WorkspaceRef;
    use chrono::Utc;

    fn session_client(class: &str, workspace: i32) -> SessionClient {
        SessionClient {
            class: class.to_string(),
            initial_class: class.to_string(),
            title: "private title".to_string(),
            workspace: WorkspaceRef::new(workspace, workspace.to_string()),
            monitor: String::new(),
            at: [0, 0],
            size: [0, 0],
            floating: false,
            pinned: false,
            fullscreen: 0,
            focus_history_id: 0,
            group: None,
            group_position: None,
            identity: Default::default(),
            launch: LaunchInfo {
                argv: vec![],
                spawn: true,
                provider: None,
                provider_state: None,
            },
        }
    }

    fn live_client(class: &str, workspace: i32) -> HyprClient {
        HyprClient {
            address: class.to_string(),
            class: class.to_string(),
            initial_class: class.to_string(),
            title: "other private title".to_string(),
            initial_title: String::new(),
            workspace: WorkspaceRef::new(workspace, workspace.to_string()),
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

    #[test]
    fn reports_missing_and_unexpected_windows_without_titles() {
        let saved = Session {
            schema_version: crate::session::SCHEMA_VERSION,
            created_by: env!("CARGO_PKG_VERSION").to_string(),
            loaded_from: None,
            workspaces: vec![],
            name: "latest".to_string(),
            created_at: Utc::now(),
            hyprland_version: String::new(),
            monitors: vec![],
            clients: vec![session_client("foot", 1), session_client("firefox", 2)],
            browser_profiles: vec![],
        };
        let config: Config = toml::from_str("").unwrap();
        let result = compare(
            &saved,
            &[live_client("foot", 1), live_client("kitty", 3)],
            &config,
            None,
        );

        assert_eq!(
            result.missing,
            vec![WindowCount {
                class: "firefox".to_string(),
                workspace: "2".to_string(),
                count: 1
            }]
        );
        assert_eq!(
            result.unexpected,
            vec![WindowCount {
                class: "kitty".to_string(),
                workspace: "3".to_string(),
                count: 1
            }]
        );
    }

    fn session_of(clients: Vec<SessionClient>) -> Session {
        Session {
            schema_version: crate::session::SCHEMA_VERSION,
            created_by: String::new(),
            loaded_from: None,
            workspaces: vec![],
            name: "latest".to_string(),
            created_at: Utc::now(),
            hyprland_version: String::new(),
            monitors: vec![],
            clients,
            browser_profiles: vec![],
        }
    }

    #[test]
    fn a_window_on_the_wrong_workspace_is_moved_not_missing() {
        // Restore would pair this terminal and move it, not launch another.
        let config: Config = toml::from_str("").unwrap();
        let result = compare(
            &session_of(vec![session_client("foot", 1), session_client("foot", 2)]),
            &[live_client("foot", 2), live_client("foot", 5)],
            &config,
            None,
        );
        assert!(result.missing.is_empty(), "{result:?}");
        assert!(result.unexpected.is_empty(), "{result:?}");
        assert_eq!(
            result.moved,
            vec![WindowMove {
                class: "foot".to_string(),
                from: "1".to_string(),
                to: "5".to_string(),
                count: 1
            }]
        );
    }

    #[test]
    fn a_renamed_window_is_recognised_as_restore_would() {
        let config: Config = toml::from_str("").unwrap();
        let mut renamed = live_client("nautilus", 4);
        renamed.initial_class = "org.gnome.Nautilus".to_string();
        let result = compare(
            &session_of(vec![session_client("org.gnome.Nautilus", 4)]),
            &[renamed],
            &config,
            None,
        );
        assert!(result.is_empty(), "{result:?}");
    }

    #[test]
    fn a_workspace_tiled_differently_from_its_saved_tree_is_reported() {
        use crate::layout::tests::{leaf, node};
        use crate::session::{Axis, Workspace};
        let config: Config = toml::from_str("").unwrap();
        let mut a = session_client("foot", 1);
        let mut b = session_client("code", 1);
        (a.at, a.size) = ([21, 21], [933, 1038]);
        (b.at, b.size) = ([966, 21], [933, 1038]);
        let mut saved = session_of(vec![a, b]);
        saved.workspaces = vec![Workspace {
            workspace: WorkspaceRef::new(1, "1"),
            monitor: String::new(),
            layout: Some(node(Axis::Horizontal, 0.5, leaf(0), leaf(1))),
            layout_kind: Some("dwindle".into()),
            layout_confidence: Some(1.0),
        }];
        let opts = LayoutOptions::default();
        let tiled = |c: &str, at: [i32; 2], size: [i32; 2]| {
            let mut w = live_client(c, 1);
            (w.at, w.size) = (at, size);
            w
        };
        let same = [
            tiled("foot", [21, 21], [933, 1038]),
            tiled("code", [966, 21], [933, 1038]),
        ];
        assert!(compare(&saved, &same, &config, Some(&opts)).is_empty());

        let wider = [
            tiled("foot", [21, 21], [1300, 1038]),
            tiled("code", [1333, 21], [566, 1038]),
        ];
        let d = compare(&saved, &wider, &config, Some(&opts));
        assert_eq!(
            d.layout,
            vec![LayoutDrift {
                workspace: "1".into(),
                what: "split ratios differ"
            }]
        );

        let swapped = [
            tiled("code", [21, 21], [933, 1038]),
            tiled("foot", [966, 21], [933, 1038]),
        ];
        let d = compare(&saved, &swapped, &config, Some(&opts));
        assert_eq!(d.layout[0].what, "arrangement differs");
        assert!(
            compare(&saved, &swapped, &config, None).is_empty(),
            "without layout options there is no layout comparison"
        );
    }
}
