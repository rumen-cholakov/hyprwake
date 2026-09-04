//! A privacy-preserving comparison between a saved session and live windows.

use crate::config::Config;
use crate::hyprctl::HyprClient;
use crate::session::Session;
use std::collections::BTreeMap;

#[derive(Debug, PartialEq, Eq)]
pub struct SessionDiff {
    pub missing: Vec<WindowCount>,
    pub unexpected: Vec<WindowCount>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct WindowCount {
    pub class: String,
    pub workspace: String,
    pub count: usize,
}

/// Compare counts by application class and workspace. Titles, command lines,
/// and paths are deliberately not part of the identity or the output.
pub fn compare(saved: &Session, live: &[HyprClient], config: &Config) -> SessionDiff {
    let mut wanted = BTreeMap::new();
    for client in &saved.clients {
        *wanted
            .entry((client.class.clone(), client.workspace.selector()))
            .or_insert(0usize) += 1;
    }

    let mut present = BTreeMap::new();
    for client in live {
        if client.is_restorable() && !config.is_ignored(&client.class) {
            *present
                .entry((client.class.clone(), client.workspace.selector()))
                .or_insert(0usize) += 1;
        }
    }

    SessionDiff {
        missing: difference(&wanted, &present),
        unexpected: difference(&present, &wanted),
    }
}

fn difference(
    left: &BTreeMap<(String, String), usize>,
    right: &BTreeMap<(String, String), usize>,
) -> Vec<WindowCount> {
    left.iter()
        .filter_map(|((class, workspace), count)| {
            let remainder =
                count.saturating_sub(*right.get(&(class.clone(), workspace.clone())).unwrap_or(&0));
            (remainder > 0).then(|| WindowCount {
                class: class.clone(),
                workspace: workspace.clone(),
                count: remainder,
            })
        })
        .collect()
}

impl SessionDiff {
    pub fn is_empty(&self) -> bool {
        self.missing.is_empty() && self.unexpected.is_empty()
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
            launch: LaunchInfo {
                argv: vec![],
                spawn: true,
            },
        }
    }

    fn live_client(class: &str, workspace: i32) -> HyprClient {
        HyprClient {
            address: class.to_string(),
            class: class.to_string(),
            initial_class: class.to_string(),
            title: "other private title".to_string(),
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
}
