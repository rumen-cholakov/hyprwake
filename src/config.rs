use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub general: GeneralConfig,
    #[serde(default)]
    pub filters: FilterConfig,
    #[serde(default)]
    pub apps: HashMap<String, AppConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneralConfig {
    #[serde(default = "default_session_name")]
    pub default_session: String,
    #[serde(default = "default_restore_delay")]
    pub restore_delay_ms: u64,
    #[serde(default = "default_detect_timeout")]
    pub window_detect_timeout_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilterConfig {
    #[serde(default = "default_ignore_classes")]
    pub ignore_classes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub binary: Option<String>,
    pub capture_cwd: Option<bool>,
    pub capture_last_command: Option<bool>,
    pub hint_template: Option<String>,
}

fn default_session_name() -> String {
    "latest".to_string()
}

fn default_restore_delay() -> u64 {
    500
}

fn default_detect_timeout() -> u64 {
    5000
}

fn default_ignore_classes() -> Vec<String> {
    vec![
        "waybar",
        "wofi",
        "mako",
        "polkit",
        "nm-applet",
        "xdg-desktop-portal",
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

impl Default for Config {
    fn default() -> Self {
        Self {
            general: GeneralConfig::default(),
            filters: FilterConfig::default(),
            apps: HashMap::new(),
        }
    }
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            default_session: default_session_name(),
            restore_delay_ms: default_restore_delay(),
            window_detect_timeout_ms: default_detect_timeout(),
        }
    }
}

impl Default for FilterConfig {
    fn default() -> Self {
        Self {
            ignore_classes: default_ignore_classes(),
        }
    }
}

pub fn load_config() -> Config {
    let path = config_path();
    if path.exists() {
        let content = std::fs::read_to_string(&path).unwrap_or_default();
        toml::from_str(&content).unwrap_or_default()
    } else {
        Config::default()
    }
}

pub fn config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("~/.config"))
        .join("hyprflow")
        .join("config.toml")
}

pub fn sessions_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("~/.local/share"))
        .join("hyprflow")
        .join("sessions")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();

        assert_eq!(config.general.default_session, "latest");
        assert_eq!(config.general.restore_delay_ms, 500);
        assert_eq!(config.general.window_detect_timeout_ms, 5000);
        assert!(
            config.filters.ignore_classes.contains(&"waybar".to_string()),
            "ignore_classes should contain 'waybar'"
        );
        assert!(config.apps.is_empty());
    }

    #[test]
    fn test_config_from_toml() {
        let toml_str = r#"
[general]
default_session = "work"
restore_delay_ms = 1000
window_detect_timeout_ms = 8000

[filters]
ignore_classes = ["waybar", "dunst"]

[apps.kitty]
binary = "/usr/bin/kitty"
capture_cwd = true
capture_last_command = false
hint_template = "{cwd}"
"#;

        let config: Config = toml::from_str(toml_str).expect("should parse valid TOML");

        assert_eq!(config.general.default_session, "work");
        assert_eq!(config.general.restore_delay_ms, 1000);
        assert_eq!(config.general.window_detect_timeout_ms, 8000);
        assert_eq!(config.filters.ignore_classes, vec!["waybar", "dunst"]);

        let kitty = config.apps.get("kitty").expect("apps.kitty should be present");
        assert_eq!(kitty.binary.as_deref(), Some("/usr/bin/kitty"));
        assert_eq!(kitty.capture_cwd, Some(true));
        assert_eq!(kitty.capture_last_command, Some(false));
        assert_eq!(kitty.hint_template.as_deref(), Some("{cwd}"));
    }

    #[test]
    fn test_empty_toml_uses_defaults() {
        let config: Config = toml::from_str("").expect("empty TOML should parse successfully");

        assert_eq!(config.general.default_session, "latest");
        assert_eq!(config.general.restore_delay_ms, 500);
        assert_eq!(config.general.window_detect_timeout_ms, 5000);
        assert!(
            config.filters.ignore_classes.contains(&"waybar".to_string()),
            "ignore_classes should contain 'waybar' by default"
        );
        assert!(
            config.filters.ignore_classes.contains(&"wofi".to_string()),
            "ignore_classes should contain 'wofi' by default"
        );
        assert!(config.apps.is_empty());
    }
}
