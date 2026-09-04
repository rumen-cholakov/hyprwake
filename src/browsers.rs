//! Chromium-family profile handling.
//!
//! Every window of a Chromium browser belongs to a single process, so a
//! window's argv says nothing about which profile it shows. The profile list
//! has to come from the browser's own `Local State`, and each profile is
//! reopened explicitly with `--profile-directory`.

use crate::config::BrowserConfig;
use crate::session::BrowserProfile;
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum BrowserError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON parse error: {0}")]
    Json(#[from] serde_json::Error),
}

/// Browsers recognised without any configuration. The key is the Hyprland
/// window class; `Local State` paths are relative to `~/.config`.
pub fn known_browsers() -> HashMap<String, BrowserConfig> {
    let entries = [
        (
            "google-chrome",
            "google-chrome-stable",
            "google-chrome/Local State",
        ),
        ("chromium", "chromium", "chromium/Local State"),
        (
            "brave-browser",
            "brave",
            "BraveSoftware/Brave-Browser/Local State",
        ),
        (
            "microsoft-edge",
            "microsoft-edge-stable",
            "microsoft-edge/Local State",
        ),
        ("vivaldi-stable", "vivaldi-stable", "vivaldi/Local State"),
    ];
    entries
        .into_iter()
        .map(|(class, binary, state)| {
            (
                class.to_string(),
                BrowserConfig {
                    binary: binary.to_string(),
                    local_state: state.to_string(),
                    profile_workspaces: HashMap::new(),
                    default_workspace: None,
                },
            )
        })
        .collect()
}

pub fn local_state_path(config: &BrowserConfig) -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("~/.config"))
        .join(&config.local_state)
}

/// Profiles the browser knows about, in `Local State` order.
pub fn read_profiles(
    class: &str,
    config: &BrowserConfig,
) -> Result<Vec<BrowserProfile>, BrowserError> {
    let path = local_state_path(config);
    if !path.exists() {
        return Ok(vec![]);
    }
    let content = std::fs::read_to_string(path)?;
    Ok(parse_profiles(class, &content)?)
}

pub fn parse_profiles(class: &str, json: &str) -> Result<Vec<BrowserProfile>, serde_json::Error> {
    let value: Value = serde_json::from_str(json)?;
    Ok(value
        .get("profile")
        .and_then(|p| p.get("info_cache"))
        .and_then(|c| c.as_object())
        .map(|cache| {
            cache
                .iter()
                .map(|(dir, info)| BrowserProfile {
                    class: class.to_string(),
                    directory: dir.clone(),
                    name: info
                        .get("name")
                        .and_then(|n| n.as_str())
                        .filter(|s| !s.is_empty())
                        .unwrap_or(dir)
                        .to_string(),
                    workspace: String::new(),
                })
                .collect()
        })
        .unwrap_or_default())
}

/// Keep only profiles the user mapped to a workspace, and stamp that
/// workspace onto each. Without a mapping the browser is restored as an
/// ordinary window instead, which is the right default for a single profile.
pub fn assign_workspaces(
    profiles: Vec<BrowserProfile>,
    config: &BrowserConfig,
) -> Vec<BrowserProfile> {
    if config.profile_workspaces.is_empty() {
        return vec![];
    }
    profiles
        .into_iter()
        .filter_map(|mut p| {
            let ws = config
                .profile_workspaces
                .get(&p.directory)
                .or(config.default_workspace.as_ref())?;
            p.workspace = ws.clone();
            Some(p)
        })
        .collect()
}

pub fn profile_argv(config: &BrowserConfig, profile: &BrowserProfile) -> Vec<String> {
    vec![
        config.binary.clone(),
        format!("--profile-directory={}", profile.directory),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOCAL_STATE: &str = r#"{
      "profile": {
        "info_cache": {
          "Default":   { "name": "Rumen" },
          "Profile 1": { "name": "Work" },
          "Profile 2": { "name": "" }
        }
      }
    }"#;

    fn config_with(mappings: &[(&str, &str)]) -> BrowserConfig {
        BrowserConfig {
            binary: "google-chrome-stable".to_string(),
            local_state: "google-chrome/Local State".to_string(),
            profile_workspaces: mappings
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            default_workspace: None,
        }
    }

    #[test]
    fn parses_profile_names() {
        let mut profiles = parse_profiles("google-chrome", LOCAL_STATE).unwrap();
        profiles.sort_by(|a, b| a.directory.cmp(&b.directory));
        assert_eq!(profiles.len(), 3);
        assert_eq!(profiles[0].directory, "Default");
        assert_eq!(profiles[0].name, "Rumen");
        assert_eq!(profiles[0].class, "google-chrome");
    }

    #[test]
    fn unnamed_profile_falls_back_to_its_directory() {
        let profiles = parse_profiles("google-chrome", LOCAL_STATE).unwrap();
        let p2 = profiles
            .iter()
            .find(|p| p.directory == "Profile 2")
            .unwrap();
        assert_eq!(p2.name, "Profile 2");
    }

    #[test]
    fn missing_profile_section_yields_nothing() {
        assert!(parse_profiles("chromium", "{}").unwrap().is_empty());
    }

    #[test]
    fn without_mappings_no_profile_windows_are_produced() {
        let profiles = parse_profiles("google-chrome", LOCAL_STATE).unwrap();
        assert!(assign_workspaces(profiles, &config_with(&[])).is_empty());
    }

    #[test]
    fn only_mapped_profiles_survive_and_carry_their_workspace() {
        let profiles = parse_profiles("google-chrome", LOCAL_STATE).unwrap();
        let assigned = assign_workspaces(profiles, &config_with(&[("Profile 1", "6")]));
        assert_eq!(assigned.len(), 1);
        assert_eq!(assigned[0].directory, "Profile 1");
        assert_eq!(assigned[0].workspace, "6");
    }

    #[test]
    fn default_workspace_covers_unmapped_profiles() {
        let mut config = config_with(&[("Default", "2")]);
        config.default_workspace = Some("9".to_string());
        let profiles = parse_profiles("google-chrome", LOCAL_STATE).unwrap();
        let mut assigned = assign_workspaces(profiles, &config);
        assigned.sort_by(|a, b| a.directory.cmp(&b.directory));
        assert_eq!(assigned.len(), 3);
        assert_eq!(assigned[0].workspace, "2");
        assert_eq!(assigned[1].workspace, "9");
    }

    #[test]
    fn profile_argv_names_the_directory() {
        let profiles = parse_profiles("google-chrome", LOCAL_STATE).unwrap();
        let p = profiles
            .iter()
            .find(|p| p.directory == "Profile 1")
            .unwrap();
        assert_eq!(
            profile_argv(&config_with(&[]), p),
            vec!["google-chrome-stable", "--profile-directory=Profile 1"]
        );
    }

    #[test]
    fn chrome_is_known_out_of_the_box() {
        let known = known_browsers();
        assert_eq!(
            known.get("google-chrome").unwrap().binary,
            "google-chrome-stable"
        );
        assert!(known.contains_key("brave-browser"));
    }
}
