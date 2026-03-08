use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// === Hyprflow session structs (what we save to disk) ===

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub hyprland_version: String,
    pub monitors: Vec<Monitor>,
    pub clients: Vec<SessionClient>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Monitor {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub transform: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionClient {
    pub class: String,
    pub title: String,
    pub workspace: i32,
    pub monitor: String,
    pub at: [i32; 2],
    pub size: [i32; 2],
    pub floating: bool,
    pub fullscreen: u8,
    pub focus_history_id: i32,
    pub launch: LaunchInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LaunchInfo {
    pub command: String,
    pub args: Vec<String>,
    pub hint: Option<String>,
}

// === Raw hyprctl JSON structs (what hyprctl returns) ===

#[derive(Debug, Clone, Deserialize)]
pub struct HyprClient {
    pub address: String,
    pub class: String,
    pub title: String,
    pub workspace: HyprWorkspace,
    pub monitor: i32,
    pub at: [i32; 2],
    pub size: [i32; 2],
    pub floating: bool,
    pub fullscreen: u8,
    #[serde(rename = "focusHistoryID")]
    pub focus_history_id: i32,
    pub pid: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HyprWorkspace {
    pub id: i32,
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HyprMonitor {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub transform: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_roundtrip() {
        let session = Session {
            name: "work".to_string(),
            created_at: DateTime::parse_from_rfc3339("2026-03-08T10:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
            hyprland_version: "0.47.0".to_string(),
            monitors: vec![Monitor {
                name: "DP-4".to_string(),
                width: 2560,
                height: 1440,
                transform: 0,
            }],
            clients: vec![SessionClient {
                class: "kitty".to_string(),
                title: "Claude Code".to_string(),
                workspace: 4,
                monitor: "DP-4".to_string(),
                at: [12, 50],
                size: [842, 1378],
                floating: false,
                fullscreen: 0,
                focus_history_id: 3,
                launch: LaunchInfo {
                    command: "kitty".to_string(),
                    args: vec![],
                    hint: None,
                },
            }],
        };

        let json = serde_json::to_string(&session).expect("serialization failed");
        let restored: Session = serde_json::from_str(&json).expect("deserialization failed");

        assert_eq!(restored.name, session.name);
        assert_eq!(restored.hyprland_version, session.hyprland_version);
        assert_eq!(restored.created_at, session.created_at);

        assert_eq!(restored.monitors.len(), 1);
        let mon = &restored.monitors[0];
        assert_eq!(mon.name, "DP-4");
        assert_eq!(mon.width, 2560);
        assert_eq!(mon.height, 1440);
        assert_eq!(mon.transform, 0);

        assert_eq!(restored.clients.len(), 1);
        let client = &restored.clients[0];
        assert_eq!(client.class, "kitty");
        assert_eq!(client.title, "Claude Code");
        assert_eq!(client.workspace, 4);
        assert_eq!(client.monitor, "DP-4");
        assert_eq!(client.at, [12, 50]);
        assert_eq!(client.size, [842, 1378]);
        assert!(!client.floating);
        assert_eq!(client.fullscreen, 0);
        assert_eq!(client.focus_history_id, 3);
        assert_eq!(client.launch.command, "kitty");
        assert!(client.launch.args.is_empty());
        assert!(client.launch.hint.is_none());
    }

    #[test]
    fn test_parse_hyprctl_clients_fixture() {
        let raw = include_str!("../tests/fixtures/sample_clients.json");
        let clients: Vec<HyprClient> =
            serde_json::from_str(raw).expect("fixture parse failed");

        assert_eq!(clients.len(), 3);

        // First client: kitty
        let kitty = &clients[0];
        assert_eq!(kitty.address, "0x55c46f7e1350");
        assert_eq!(kitty.class, "kitty");
        assert_eq!(kitty.title, "Claude Code");
        assert_eq!(kitty.workspace.id, 4);
        assert_eq!(kitty.workspace.name, "4");
        assert_eq!(kitty.monitor, 0);
        assert_eq!(kitty.at, [12, 50]);
        assert_eq!(kitty.size, [842, 1378]);
        assert!(!kitty.floating);
        assert_eq!(kitty.fullscreen, 0);
        assert_eq!(kitty.focus_history_id, 3);
        assert_eq!(kitty.pid, 9537);

        // Second client: brave-browser
        let brave = &clients[1];
        assert_eq!(brave.class, "brave-browser");
        assert_eq!(brave.workspace.id, 1);
        assert_eq!(brave.focus_history_id, 1);

        // Third client: obsidian
        let obsidian = &clients[2];
        assert_eq!(obsidian.class, "obsidian");
        assert_eq!(obsidian.title, "smart notes - Obsidian");
        assert_eq!(obsidian.workspace.id, 3);
        assert_eq!(obsidian.focus_history_id, 2);
        assert_eq!(obsidian.pid, 5000);
    }
}
