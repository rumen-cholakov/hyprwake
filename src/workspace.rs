//! Workspace identity.
//!
//! A workspace cannot be reduced to its numeric id: named workspaces and
//! special (scratchpad / "magic") workspaces both carry meaning only in their
//! name, and special workspaces have synthetic negative ids that differ
//! between sessions. Every comparison and every dispatcher target therefore
//! goes through [`WorkspaceRef::selector`].

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceRef {
    pub id: i32,
    #[serde(default)]
    pub name: String,
}

impl WorkspaceRef {
    pub fn new(id: i32, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
        }
    }

    /// The string Hyprland's dispatchers accept as a workspace target.
    ///
    /// * unnamed special workspace -> `special:special`
    /// * named special workspace   -> `special:<name>` (already in `name`)
    /// * plain numbered workspace  -> `<id>`
    /// * named workspace           -> `name:<name>`
    pub fn selector(&self) -> String {
        if self.name == "special" {
            return "special:special".to_string();
        }
        if self.name.starts_with("special:") {
            return self.name.clone();
        }
        if self.name == self.id.to_string() || self.name.is_empty() {
            return self.id.to_string();
        }
        format!("name:{}", self.name)
    }

    pub fn is_special(&self) -> bool {
        self.name == "special" || self.name.starts_with("special:")
    }

    /// Two workspace references denote the same workspace when their
    /// dispatcher selectors agree. Ids alone are not reliable for special
    /// workspaces, which is what makes scratchpad restore work.
    pub fn same(&self, other: &Self) -> bool {
        self.selector() == other.selector()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbered_workspace_uses_id() {
        assert_eq!(WorkspaceRef::new(3, "3").selector(), "3");
    }

    #[test]
    fn named_workspace_uses_name_prefix() {
        assert_eq!(WorkspaceRef::new(7, "code").selector(), "name:code");
    }

    #[test]
    fn unnamed_special_workspace_is_qualified() {
        assert_eq!(
            WorkspaceRef::new(-99, "special").selector(),
            "special:special"
        );
    }

    #[test]
    fn named_special_workspace_passes_through() {
        assert_eq!(
            WorkspaceRef::new(-98, "special:magic").selector(),
            "special:magic"
        );
    }

    #[test]
    fn empty_name_falls_back_to_id() {
        assert_eq!(WorkspaceRef::new(2, "").selector(), "2");
    }

    #[test]
    fn special_workspaces_match_by_name_not_id() {
        // Ids for special workspaces are assigned per session and drift;
        // the name is the stable identity.
        let saved = WorkspaceRef::new(-99, "special:magic");
        let now = WorkspaceRef::new(-42, "special:magic");
        assert!(saved.same(&now));
    }

    #[test]
    fn different_workspaces_do_not_match() {
        assert!(!WorkspaceRef::new(1, "1").same(&WorkspaceRef::new(2, "2")));
    }

    #[test]
    fn is_special_detects_both_forms() {
        assert!(WorkspaceRef::new(-99, "special").is_special());
        assert!(WorkspaceRef::new(-98, "special:term").is_special());
        assert!(!WorkspaceRef::new(1, "1").is_special());
    }
}
