use crate::session::BraveProfile;
use serde_json::Value;
use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum BraveError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON parse error: {0}")]
    Json(#[from] serde_json::Error),
}

/// Default path to Brave's Local State file on Linux.
pub fn local_state_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("~/.config"))
        .join("BraveSoftware/Brave-Browser/Local State")
}

/// Read and parse profiles from the Local State file.
pub fn read_profiles() -> Result<Vec<BraveProfile>, BraveError> {
    let path = local_state_path();
    if !path.exists() {
        return Ok(vec![]);
    }
    let content = std::fs::read_to_string(path)?;
    Ok(parse_profiles_from_local_state(&content)?)
}

/// Parse profile info from Local State JSON content.
pub fn parse_profiles_from_local_state(
    json_str: &str,
) -> Result<Vec<BraveProfile>, serde_json::Error> {
    let value: Value = serde_json::from_str(json_str)?;
    let profiles = value
        .get("profile")
        .and_then(|p| p.get("info_cache"))
        .and_then(|c| c.as_object())
        .map(|cache| {
            cache
                .iter()
                .map(|(dir, info)| BraveProfile {
                    directory: dir.clone(),
                    name: info
                        .get("name")
                        .and_then(|n| n.as_str())
                        .filter(|s| !s.is_empty())
                        .unwrap_or(dir)
                        .to_string(),
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(profiles)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_local_state_profiles() {
        let json = r#"{
            "profile": {
                "info_cache": {
                    "Default": {"name": "Credifit"},
                    "Profile 1": {"name": "LinkPJ"},
                    "Profile 2": {"name": "ABRH Bahia"}
                }
            }
        }"#;
        let profiles = parse_profiles_from_local_state(json).unwrap();
        assert_eq!(profiles.len(), 3);
        assert!(profiles
            .iter()
            .any(|p| p.directory == "Default" && p.name == "Credifit"));
        assert!(profiles
            .iter()
            .any(|p| p.directory == "Profile 1" && p.name == "LinkPJ"));
    }

    #[test]
    fn test_parse_local_state_empty() {
        let json = r#"{"profile": {"info_cache": {}}}"#;
        let profiles = parse_profiles_from_local_state(json).unwrap();
        assert!(profiles.is_empty());
    }

    #[test]
    fn test_parse_local_state_empty_name_falls_back_to_directory() {
        let json = r#"{"profile": {"info_cache": {"Default": {"name": ""}}}}"#;
        let profiles = parse_profiles_from_local_state(json).unwrap();
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].name, "Default", "empty name should fall back to directory");
    }

    #[test]
    fn test_parse_local_state_missing_field() {
        let json = r#"{"other": "data"}"#;
        let profiles = parse_profiles_from_local_state(json).unwrap();
        assert!(profiles.is_empty());
    }
}
