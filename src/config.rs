//! Configuration: `~/.config/hyprwake/config.toml`.
//!
//! Everything has a working default, so the file is optional. `hyprwake
//! config --init` writes one seeded from what is actually installed on the
//! machine.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub general: GeneralConfig,
    #[serde(default)]
    pub launch: LaunchConfig,
    #[serde(default)]
    pub filters: FilterConfig,
    #[serde(default = "default_terminals")]
    pub terminals: HashMap<String, TerminalConfig>,
    #[serde(default)]
    pub tui: TuiConfig,
    #[serde(default)]
    pub apps: HashMap<String, AppConfig>,
    #[serde(default)]
    pub browsers: HashMap<String, BrowserConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneralConfig {
    #[serde(default = "default_session_name")]
    pub default_session: String,
    /// Autosaves kept before the oldest are pruned.
    #[serde(default = "default_autosave_retain")]
    pub autosave_retain: usize,
    /// Seconds between saves in `hyprwake daemon --poll`.
    #[serde(default = "default_save_interval")]
    pub save_interval_secs: u64,
    /// Quiet period after a window event before an event-driven save fires.
    #[serde(default = "default_debounce")]
    pub debounce_ms: u64,
    /// Pause between spawns so the compositor can settle.
    #[serde(default = "default_stagger")]
    pub spawn_stagger_ms: u64,
    /// How long the sweep waits for late-arriving windows.
    #[serde(default = "default_sweep_timeout")]
    pub sweep_timeout_secs: u64,
    /// Interval between sweep polls.
    #[serde(default = "default_sweep_poll")]
    pub sweep_poll_ms: u64,
    /// Restore aborts when more than this many windows are already open, so
    /// a stray invocation cannot duplicate a live desktop.
    #[serde(default = "default_abort_above")]
    pub abort_restore_above: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LaunchConfig {
    /// Launch through `uwsm-app` so restored apps land in their own systemd
    /// scopes, as a uwsm-managed session (Omarchy) starts them. `None`
    /// auto-detects by looking for the binary.
    #[serde(default)]
    pub use_uwsm: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilterConfig {
    #[serde(default = "default_ignore_classes")]
    pub ignore_classes: Vec<String>,
}

/// How to reopen a terminal at a directory, and optionally running a program.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalConfig {
    pub binary: String,
    /// Flag introducing the working directory. A trailing `=` means the
    /// value is joined to the flag (`--working-directory=/path`) rather than
    /// passed as a separate argument.
    pub cwd_flag: String,
    /// Flag introducing a command to run, where the terminal needs one.
    #[serde(default)]
    pub exec_flag: Option<String>,
    /// Extra arguments every relaunch needs — an `--app-id` that pins the
    /// window class, for instance, without which the sweep cannot pair the
    /// window with its saved entry.
    #[serde(default)]
    pub extra_args: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TuiConfig {
    /// Programs worth reopening inside a restored terminal.
    #[serde(default = "default_tui_programs")]
    pub programs: Vec<String>,
    #[serde(default = "default_shells")]
    pub shells: Vec<String>,
    /// Programs that can reopen the session they were in, keyed by program
    /// name.
    #[serde(default = "default_resume_rules")]
    pub resume: HashMap<String, ResumeConfig>,
}

/// How to recover a program's session and ask it to resume.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResumeConfig {
    /// Path pattern matched against the program's open files. One segment
    /// carries `{id}`; `*` matches within a segment.
    pub fd_glob: String,
    /// Arguments appended when an id is recovered; `{id}` is substituted.
    pub args: Vec<String>,
    /// Arguments used when no id could be recovered.
    #[serde(default)]
    pub fallback: Vec<String>,
    /// Flags to drop from the captured argv first, so a session started with
    /// its own resume flag is not asked to resume twice. A non-flag token
    /// following one of these is dropped with it.
    #[serde(default)]
    pub strip_flags: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AppConfig {
    /// Override the binary; by default the saved argv is replayed as-is.
    pub binary: Option<String>,
    /// Never relaunch this class, but still place its window if it appears.
    #[serde(default)]
    pub no_spawn: bool,
    /// Arguments to drop from the captured argv (session-scoped temp paths).
    #[serde(default)]
    pub strip_args: Vec<String>,
}

/// Chromium-family browsers, whose windows all belong to one process, so
/// profiles have to be read from the browser's own state file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserConfig {
    pub binary: String,
    /// Path to `Local State`, relative to the config dir.
    pub local_state: String,
    /// Profile directory -> workspace. Only mapped profiles are restored.
    #[serde(default)]
    pub profile_workspaces: HashMap<String, String>,
    #[serde(default)]
    pub default_workspace: Option<String>,
}

// ── Defaults ───────────────────────────────────────────────────────────────

fn default_session_name() -> String {
    "latest".to_string()
}
fn default_autosave_retain() -> usize {
    5
}
fn default_save_interval() -> u64 {
    60
}
fn default_debounce() -> u64 {
    3000
}
fn default_stagger() -> u64 {
    300
}
fn default_sweep_timeout() -> u64 {
    20
}
fn default_sweep_poll() -> u64 {
    1000
}
fn default_abort_above() -> usize {
    3
}

fn default_ignore_classes() -> Vec<String> {
    [
        // bars, launchers, notification daemons and portals are all started
        // by the session itself and must never be duplicated
        "org.quickshell",
        "quickshell",
        "waybar",
        "wofi",
        "rofi",
        "walker",
        "mako",
        "dunst",
        "swaync",
        "xembedsniproxy",
        "polkit-gnome-authentication-agent-1",
        "hyprlock",
        "xdg-desktop-portal",
        "xdg-desktop-portal-gtk",
        "xdg-desktop-portal-hyprland",
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

pub fn default_terminals() -> HashMap<String, TerminalConfig> {
    let mut t = HashMap::new();
    t.insert(
        "foot".to_string(),
        TerminalConfig {
            binary: "foot".to_string(),
            cwd_flag: "-D".to_string(),
            exec_flag: None,
            extra_args: vec![],
        },
    );
    // Omarchy's terminal is foot under its own app-id; relaunching without
    // the app-id would change the window class and break sweep pairing.
    t.insert(
        "org.omarchy.terminal".to_string(),
        TerminalConfig {
            binary: "foot".to_string(),
            cwd_flag: "-D".to_string(),
            exec_flag: None,
            extra_args: vec!["--app-id=org.omarchy.terminal".to_string()],
        },
    );
    t.insert(
        "kitty".to_string(),
        TerminalConfig {
            binary: "kitty".to_string(),
            cwd_flag: "-d".to_string(),
            exec_flag: None,
            extra_args: vec![],
        },
    );
    t.insert(
        "Alacritty".to_string(),
        TerminalConfig {
            binary: "alacritty".to_string(),
            cwd_flag: "--working-directory".to_string(),
            exec_flag: Some("-e".to_string()),
            extra_args: vec![],
        },
    );
    t.insert(
        "com.mitchellh.ghostty".to_string(),
        TerminalConfig {
            binary: "ghostty".to_string(),
            cwd_flag: "--working-directory=".to_string(),
            exec_flag: Some("-e".to_string()),
            extra_args: vec![],
        },
    );
    t
}

fn default_tui_programs() -> Vec<String> {
    [
        "nvim",
        "vim",
        "vi",
        "hx",
        "helix",
        "emacs",
        "yazi",
        "ranger",
        "lf",
        "nnn",
        "joshuto",
        "mc",
        "btop",
        "htop",
        "top",
        "bottom",
        "btm",
        "ncdu",
        "duf",
        "lazygit",
        "lazydocker",
        "gitui",
        "tig",
        "k9s",
        "aerc",
        "neomutt",
        "mutt",
        "irssi",
        "weechat",
        "cmus",
        "ncmpcpp",
        "herdr",
        "claude",
        "codex",
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

/// Claude Code keeps a per-session directory open under /tmp whose name is
/// the session id, so the exact conversation can be reopened. `--continue`
/// is the fallback: it reopens the most recent conversation in the directory,
/// which is right only when there was one.
fn default_resume_rules() -> HashMap<String, ResumeConfig> {
    let mut rules = HashMap::new();
    rules.insert(
        "claude".to_string(),
        ResumeConfig {
            fd_glob: "/tmp/claude-*/*/{id}/*".to_string(),
            args: vec!["--resume".to_string(), "{id}".to_string()],
            fallback: vec!["--continue".to_string()],
            strip_flags: vec![
                "--resume".to_string(),
                "--continue".to_string(),
                "-c".to_string(),
            ],
        },
    );
    rules
}

fn default_shells() -> Vec<String> {
    ["bash", "zsh", "fish", "sh", "dash", "nu", "elvish", "xonsh"]
        .into_iter()
        .map(String::from)
        .collect()
}

impl Default for Config {
    /// Mirrors what serde produces for an empty file. Deriving `Default`
    /// here would hand back an empty terminals table, quietly disabling all
    /// terminal handling whenever no config file exists.
    fn default() -> Self {
        Self {
            general: GeneralConfig::default(),
            launch: LaunchConfig::default(),
            filters: FilterConfig::default(),
            terminals: default_terminals(),
            tui: TuiConfig::default(),
            apps: HashMap::new(),
            browsers: HashMap::new(),
        }
    }
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            default_session: default_session_name(),
            autosave_retain: default_autosave_retain(),
            save_interval_secs: default_save_interval(),
            debounce_ms: default_debounce(),
            spawn_stagger_ms: default_stagger(),
            sweep_timeout_secs: default_sweep_timeout(),
            sweep_poll_ms: default_sweep_poll(),
            abort_restore_above: default_abort_above(),
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

impl Default for TuiConfig {
    fn default() -> Self {
        Self {
            programs: default_tui_programs(),
            shells: default_shells(),
            resume: default_resume_rules(),
        }
    }
}

// ── Queries ────────────────────────────────────────────────────────────────

impl Config {
    pub fn is_ignored(&self, class: &str) -> bool {
        self.filters.ignore_classes.iter().any(|c| c == class)
    }

    pub fn terminal_for(&self, class: &str) -> Option<&TerminalConfig> {
        self.terminals.get(class)
    }

    pub fn tui_programs(&self) -> BTreeSet<String> {
        self.tui.programs.iter().cloned().collect()
    }

    pub fn shells(&self) -> BTreeSet<String> {
        self.tui.shells.iter().cloned().collect()
    }

    /// Whether restores should go through `uwsm-app`.
    pub fn use_uwsm(&self) -> bool {
        match self.launch.use_uwsm {
            Some(explicit) => explicit,
            None => which::which("uwsm-app").is_ok(),
        }
    }
}

impl TerminalConfig {
    /// argv for opening this terminal at `cwd`, optionally running `program`.
    pub fn build_argv(&self, cwd: &str, program: Option<&[String]>) -> Vec<String> {
        let mut argv = vec![self.binary.clone()];
        if let Some(flag) = self.cwd_flag.strip_suffix('=') {
            argv.push(format!("{flag}={cwd}"));
        } else {
            argv.push(self.cwd_flag.clone());
            argv.push(cwd.to_string());
        }
        argv.extend(self.extra_args.iter().cloned());
        if let Some(prog) = program {
            if let Some(exec) = &self.exec_flag {
                argv.push(exec.clone());
            }
            argv.extend(prog.iter().cloned());
        }
        argv
    }
}

// ── Paths and IO ───────────────────────────────────────────────────────────

pub fn config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("~/.config"))
        .join("hyprwake")
        .join("config.toml")
}

pub fn sessions_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("~/.local/share"))
        .join("hyprwake")
        .join("sessions")
}

/// Load the config, falling back to defaults. A malformed file is reported
/// rather than silently ignored: restore behaviour would otherwise change
/// without explanation.
pub fn load_config() -> Config {
    let path = config_path();
    if !path.exists() {
        return Config::default();
    }
    match std::fs::read_to_string(&path) {
        Ok(content) => match toml::from_str(&content) {
            Ok(config) => config,
            Err(e) => {
                eprintln!(
                    "hyprwake: {} is invalid, using defaults: {e}",
                    path.display()
                );
                crate::logging::log(format!("config invalid, using defaults: {e}"));
                Config::default()
            }
        },
        Err(e) => {
            eprintln!("hyprwake: cannot read {}: {e}", path.display());
            Config::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_populated() {
        let c = Config::default();
        assert_eq!(c.general.default_session, "latest");
        assert!(c.is_ignored("waybar"));
        assert!(!c.is_ignored("foot"));
    }

    #[test]
    fn default_and_empty_file_agree() {
        // A missing config file falls back to Default; if the two disagreed,
        // terminal handling would work only for users who have a config.
        let from_file: Config = toml::from_str("").unwrap();
        let from_default = Config::default();
        assert!(from_default.terminal_for("foot").is_some());
        assert!(from_default.terminal_for("org.omarchy.terminal").is_some());
        assert_eq!(
            from_file.terminals.len(),
            from_default.terminals.len(),
            "serde defaults and Default::default() must populate the same tables"
        );
        assert_eq!(from_file.tui.programs, from_default.tui.programs);
        assert_eq!(
            from_file.filters.ignore_classes,
            from_default.filters.ignore_classes
        );
    }

    #[test]
    fn foot_argv_uses_separate_directory_argument() {
        let c: Config = toml::from_str("").unwrap();
        let argv = c.terminal_for("foot").unwrap().build_argv("/home/rc", None);
        assert_eq!(argv, vec!["foot", "-D", "/home/rc"]);
    }

    #[test]
    fn omarchy_terminal_keeps_its_app_id() {
        let c: Config = toml::from_str("").unwrap();
        let argv = c
            .terminal_for("org.omarchy.terminal")
            .unwrap()
            .build_argv("/tmp", None);
        assert!(argv.contains(&"--app-id=org.omarchy.terminal".to_string()));
    }

    #[test]
    fn joined_cwd_flag_is_glued_to_its_value() {
        let c: Config = toml::from_str("").unwrap();
        let argv = c
            .terminal_for("com.mitchellh.ghostty")
            .unwrap()
            .build_argv("/tmp", None);
        assert_eq!(argv[1], "--working-directory=/tmp");
    }

    #[test]
    fn alacritty_needs_an_exec_flag_before_the_program() {
        let c: Config = toml::from_str("").unwrap();
        let prog = vec!["nvim".to_string(), "a.rs".to_string()];
        let argv = c
            .terminal_for("Alacritty")
            .unwrap()
            .build_argv("/tmp", Some(&prog));
        assert_eq!(
            argv,
            vec![
                "alacritty",
                "--working-directory",
                "/tmp",
                "-e",
                "nvim",
                "a.rs"
            ]
        );
    }

    #[test]
    fn foot_takes_the_program_positionally() {
        let c: Config = toml::from_str("").unwrap();
        let prog = vec!["yazi".to_string()];
        let argv = c
            .terminal_for("foot")
            .unwrap()
            .build_argv("/tmp", Some(&prog));
        assert_eq!(argv, vec!["foot", "-D", "/tmp", "yazi"]);
    }

    #[test]
    fn user_config_overrides_and_merges() {
        let c: Config = toml::from_str(
            r#"
[general]
default_session = "work"
sweep_timeout_secs = 45

[launch]
use_uwsm = false

[terminals.foot]
binary = "footclient"
cwd_flag = "-D"
"#,
        )
        .unwrap();
        assert_eq!(c.general.default_session, "work");
        assert_eq!(c.general.sweep_timeout_secs, 45);
        assert_eq!(c.general.autosave_retain, 5, "unset keys keep defaults");
        assert!(!c.use_uwsm());
        assert_eq!(c.terminal_for("foot").unwrap().binary, "footclient");
        assert!(
            c.terminal_for("kitty").is_none(),
            "an explicit terminals table replaces the default one"
        );
    }

    #[test]
    fn browser_profile_mapping_parses() {
        let c: Config = toml::from_str(
            r#"
[browsers.google-chrome]
binary = "google-chrome-stable"
local_state = "google-chrome/Local State"
default_workspace = "2"
profile_workspaces = { "Default" = "2", "Profile 1" = "6" }
"#,
        )
        .unwrap();
        let b = c.browsers.get("google-chrome").unwrap();
        assert_eq!(b.profile_workspaces.get("Profile 1").unwrap(), "6");
        assert_eq!(b.default_workspace.as_deref(), Some("2"));
    }
}
