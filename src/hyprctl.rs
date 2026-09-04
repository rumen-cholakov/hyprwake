//! Thin wrapper around the `hyprctl` binary.

use crate::workspace::WorkspaceRef;
use serde::{Deserialize, Serialize};
use std::process::Command;

// ── Hyprland data types ────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HyprClient {
    pub address: String,
    pub class: String,
    #[serde(default, rename = "initialClass")]
    pub initial_class: String,
    #[serde(default)]
    pub title: String,
    pub workspace: WorkspaceRef,
    #[serde(default)]
    pub monitor: i32,
    pub at: [i32; 2],
    pub size: [i32; 2],
    #[serde(default)]
    pub floating: bool,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub fullscreen: u8,
    #[serde(default, rename = "focusHistoryID")]
    pub focus_history_id: i32,
    #[serde(default)]
    pub pid: i32,
    #[serde(default = "default_true")]
    pub mapped: bool,
}

fn default_true() -> bool {
    true
}

impl HyprClient {
    /// A window worth saving: actually on screen, owned by a live process,
    /// and identifiable by class. Unmapped or class-less surfaces are helper
    /// windows (tray bridges, tooltips) that reappear on their own.
    pub fn is_restorable(&self) -> bool {
        self.mapped && self.pid > 0 && !self.class.is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HyprMonitor {
    pub id: i32,
    pub name: String,
    pub width: u32,
    pub height: u32,
    #[serde(default)]
    pub transform: u32,
}

// ── Error type ─────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum HyprctlError {
    #[error("hyprctl command failed: {0}")]
    CommandFailed(String),
    #[error("dispatch rejected: {0}")]
    DispatchRejected(String),
    #[error("JSON parse error: {0}")]
    ParseError(#[from] serde_json::Error),
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
}

// ── Trait ──────────────────────────────────────────────────────────────────

pub trait HyprctlClient {
    fn get_clients(&self) -> Result<Vec<HyprClient>, HyprctlError>;
    fn get_monitors(&self) -> Result<Vec<HyprMonitor>, HyprctlError>;
    /// Send one dispatcher call. `call` is passed to hyprctl verbatim as a
    /// single argument, so Lua expressions survive intact.
    fn dispatch(&self, call: &str) -> Result<(), HyprctlError>;
    fn get_hyprland_version(&self) -> Result<String, HyprctlError>;
}

// ── Real implementation ────────────────────────────────────────────────────

pub struct RealHyprctl;

impl RealHyprctl {
    fn json<T: for<'de> Deserialize<'de>>(&self, what: &str) -> Result<T, HyprctlError> {
        let output = Command::new("hyprctl").args(["-j", what]).output()?;
        if !output.status.success() {
            return Err(HyprctlError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ));
        }
        Ok(serde_json::from_slice(&output.stdout)?)
    }
}

impl HyprctlClient for RealHyprctl {
    fn get_clients(&self) -> Result<Vec<HyprClient>, HyprctlError> {
        self.json("clients")
    }

    fn get_monitors(&self) -> Result<Vec<HyprMonitor>, HyprctlError> {
        self.json("monitors")
    }

    fn dispatch(&self, call: &str) -> Result<(), HyprctlError> {
        // Note: the call must stay a single argv entry. Splitting it on
        // whitespace would tear Lua table literals apart.
        let output = Command::new("hyprctl").arg("dispatch").arg(call).output()?;

        // hyprctl exits 0 even when the compositor rejects a dispatch; the
        // verdict is in the payload, which is "ok" on success and an
        // "error:"/"warning:" line otherwise.
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let payload = if stdout.is_empty() { stderr } else { stdout };

        if !output.status.success() || !payload.to_lowercase().starts_with("ok") {
            return Err(HyprctlError::DispatchRejected(format!(
                "{payload} :: {call}"
            )));
        }
        Ok(())
    }

    fn get_hyprland_version(&self) -> Result<String, HyprctlError> {
        let output = Command::new("hyprctl").arg("version").output()?;
        if !output.status.success() {
            return Err(HyprctlError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ));
        }
        // "Hyprland 0.56.2 built from branch ..."
        let text = String::from_utf8_lossy(&output.stdout);
        Ok(text
            .split_whitespace()
            .nth(1)
            .unwrap_or("unknown")
            .to_string())
    }
}

// ── Test double ────────────────────────────────────────────────────────────

#[cfg(any(test, feature = "testing"))]
pub mod mock {
    use super::*;
    use std::cell::RefCell;

    /// Records dispatches and replays scripted `get_clients` responses, one
    /// per call, so a whole restore run can be driven without a compositor.
    pub struct MockHyprctl {
        pub dispatched: RefCell<Vec<String>>,
        pub client_frames: RefCell<Vec<Vec<HyprClient>>>,
        pub monitors: Vec<HyprMonitor>,
        pub reject: Option<String>,
    }

    impl Default for MockHyprctl {
        fn default() -> Self {
            Self::new(vec![])
        }
    }

    impl MockHyprctl {
        pub fn new(frames: Vec<Vec<HyprClient>>) -> Self {
            Self {
                dispatched: RefCell::new(Vec::new()),
                client_frames: RefCell::new(frames),
                monitors: vec![HyprMonitor {
                    id: 0,
                    name: "eDP-1".to_string(),
                    width: 1920,
                    height: 1080,
                    transform: 0,
                }],
                reject: None,
            }
        }

        pub fn calls(&self) -> Vec<String> {
            self.dispatched.borrow().clone()
        }
    }

    impl HyprctlClient for MockHyprctl {
        fn get_clients(&self) -> Result<Vec<HyprClient>, HyprctlError> {
            let mut frames = self.client_frames.borrow_mut();
            if frames.is_empty() {
                return Ok(vec![]);
            }
            // Keep replaying the last frame once the script runs out.
            if frames.len() == 1 {
                Ok(frames[0].clone())
            } else {
                Ok(frames.remove(0))
            }
        }

        fn get_monitors(&self) -> Result<Vec<HyprMonitor>, HyprctlError> {
            Ok(self.monitors.clone())
        }

        fn dispatch(&self, call: &str) -> Result<(), HyprctlError> {
            self.dispatched.borrow_mut().push(call.to_string());
            match &self.reject {
                Some(msg) => Err(HyprctlError::DispatchRejected(msg.clone())),
                None => Ok(()),
            }
        }

        fn get_hyprland_version(&self) -> Result<String, HyprctlError> {
            Ok("0.56.2-mock".to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::mock::MockHyprctl;
    use super::*;

    fn client(class: &str, ws: i32) -> HyprClient {
        HyprClient {
            address: format!("0x{class}{ws}"),
            class: class.to_string(),
            initial_class: class.to_string(),
            title: String::new(),
            workspace: WorkspaceRef::new(ws, ws.to_string()),
            monitor: 0,
            at: [0, 0],
            size: [800, 600],
            floating: false,
            pinned: false,
            fullscreen: 0,
            focus_history_id: 0,
            pid: 100,
            mapped: true,
        }
    }

    #[test]
    fn mock_records_dispatch_calls_verbatim() {
        let mock = MockHyprctl::default();
        mock.dispatch("hl.dsp.exec_cmd([[foot]], { workspace = '2 silent' })")
            .unwrap();
        assert_eq!(
            mock.calls(),
            vec!["hl.dsp.exec_cmd([[foot]], { workspace = '2 silent' })"]
        );
    }

    #[test]
    fn mock_replays_frames_then_repeats_the_last() {
        let mock = MockHyprctl::new(vec![vec![], vec![client("foot", 1)]]);
        assert_eq!(mock.get_clients().unwrap().len(), 0);
        assert_eq!(mock.get_clients().unwrap().len(), 1);
        assert_eq!(mock.get_clients().unwrap().len(), 1);
    }

    #[test]
    fn unmapped_windows_are_not_restorable() {
        let mut c = client("foot", 1);
        c.mapped = false;
        assert!(!c.is_restorable());
    }

    #[test]
    fn classless_windows_are_not_restorable() {
        let mut c = client("foot", 1);
        c.class = String::new();
        assert!(!c.is_restorable());
    }

    #[test]
    fn windows_without_a_pid_are_not_restorable() {
        let mut c = client("foot", 1);
        c.pid = 0;
        assert!(!c.is_restorable());
    }

    #[test]
    fn ordinary_window_is_restorable() {
        assert!(client("foot", 1).is_restorable());
    }

    #[test]
    fn client_json_without_optional_fields_parses() {
        // Older hyprctl builds omit pinned/initialClass; defaults must hold.
        let json = r#"{"address":"0x1","class":"foot","workspace":{"id":1,"name":"1"},
                       "at":[0,0],"size":[100,100],"pid":42}"#;
        let c: HyprClient = serde_json::from_str(json).unwrap();
        assert!(!c.pinned);
        assert!(c.mapped);
        assert_eq!(c.class, "foot");
    }
}
