//! Configuration: `~/.config/hyprwake/config.toml`.
//!
//! Everything has a working default, so the file is optional. `hyprwake
//! config --init` writes one seeded from what is actually installed on the
//! machine.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
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
    /// Providers declared in the new form, keyed by name. Read together with
    /// the legacy tables above into one [`ProviderSet`](crate::provider::ProviderSet).
    #[serde(default)]
    pub providers: BTreeMap<String, crate::provider::ProviderEntry>,
    #[serde(default)]
    pub monitors: MonitorConfig,
    #[serde(default)]
    pub restore: RestoreConfig,
}

/// How a restore rebuilds each workspace's tiling.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RestoreConfig {
    #[serde(default)]
    pub layout: LayoutMode,
    /// `best-effort` replays a workspace's split tree only at or above this
    /// confidence, which capture records alongside the tree.
    #[serde(default = "default_layout_min_confidence")]
    pub layout_min_confidence: f32,
    #[serde(default)]
    pub groups: GroupMode,
}

/// What a restore does with windows that were tabbed together in a group.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GroupMode {
    /// Members come back as separate windows on the group's workspace, and
    /// the report names the group. What 0.1.x did.
    #[default]
    Separate,
    /// Rebuild each group: launch its first member, make it a group, and
    /// launch the others into it in their saved order, checking each joined.
    Regroup,
}

impl Default for RestoreConfig {
    fn default() -> Self {
        RestoreConfig {
            layout: LayoutMode::default(),
            layout_min_confidence: default_layout_min_confidence(),
            groups: GroupMode::default(),
        }
    }
}

fn default_recheck_secs() -> u64 {
    300
}

fn default_layout_min_confidence() -> f32 {
    0.7
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LayoutMode {
    /// No layout work: no tree is inferred at save, and tiled windows are
    /// launched without waiting for one another.
    Off,
    /// Tiled windows launched in their saved reading order, each waiting
    /// for the one before it on its workspace — the 0.1.x restore, with the
    /// later tiled-order fix.
    #[default]
    ReadingOrder,
    /// Replay the saved split tree wherever capture was confident enough,
    /// and reading order elsewhere.
    BestEffort,
    /// Replay every saved tree, however low its confidence. For debugging
    /// inference.
    Strict,
}

/// Where workspaces go when the monitors attached now are not the ones they
/// were saved on.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MonitorConfig {
    /// What to do with a workspace whose monitor is not attached.
    #[serde(default)]
    pub missing: MissingMonitor,
    /// Saved connector -> connector to use instead, when the saved one is not
    /// attached. Applies whatever `missing` says.
    #[serde(default)]
    pub remap: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MissingMonitor {
    /// Workspaces keep their numbers and land wherever Hyprland puts them;
    /// monitors are matched by connector name only. What 0.1.x did.
    #[default]
    PreserveNumbers,
    /// Every workspace of a missing monitor moves to the focused one.
    Collapse,
    /// Each missing monitor's workspaces move to the attached monitor most
    /// like it — nearest aspect ratio, then area — with floating windows
    /// scaled into it.
    Remap,
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
    /// How long a restore waits for one tiled window to map before launching
    /// the next one on the same workspace.
    ///
    /// Dwindle tiles windows in the order they *map*, not the order they were
    /// launched, and a terminal maps in milliseconds while an Electron app or
    /// a browser takes seconds. Launching a workspace's windows back to back
    /// let the fast one overtake the slow one and swapped them. Waiting keeps
    /// the saved left-to-right order; the timeout keeps one program that never
    /// maps from holding up the rest of the restore. 0 turns the wait off.
    #[serde(default = "default_tiled_map_timeout")]
    pub tiled_map_timeout_ms: u64,
    /// Refuse a save that keeps less than this fraction of the windows the
    /// previous save held. 1.0 refuses any loss, 0.0 disables the guard.
    #[serde(default = "default_drop_fraction")]
    pub save_drop_fraction: f64,
    /// The guard only applies this soon after the previous save. A desktop
    /// genuinely emptied over minutes is a real change; one emptied seconds
    /// after a full snapshot is a session being torn down.
    #[serde(default = "default_drop_window_secs")]
    pub save_drop_window_secs: i64,
    /// `watch` re-captures the desktop after this many quiet seconds and
    /// saves if it changed: resizing a tiled window emits no event. 0 turns
    /// it off.
    #[serde(default = "default_recheck_secs")]
    pub recheck_secs: u64,
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
///
/// Unknown keys are ignored rather than rejected: this struct also reads the
/// legacy `[terminals]` table, and a config that parsed under 0.1.3 must
/// still parse.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
///
/// Two strategies, tried in order. `fd_glob` reads the id out of a path the
/// running program holds open, which is exact. `id_command` runs a helper
/// that prints the id for a working directory, for programs that keep their
/// sessions in a database instead.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResumeConfig {
    /// Path pattern matched against the program's open files. One segment
    /// carries `{id}`; `*` matches within a segment.
    #[serde(default)]
    pub fd_glob: Option<String>,
    /// Command whose first line of output is the session id. `{cwd}`,
    /// `{cwd_sql}` (single quotes doubled) and `{home}` are substituted.
    /// It runs as you, at save time, with a short timeout.
    #[serde(default)]
    pub id_command: Vec<String>,
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

/// How a browser family records and selects profiles.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BrowserKind {
    /// Profiles in `Local State`, selected with `--profile-directory`.
    #[default]
    Chromium,
    /// Profiles in `profiles.ini`, selected by name with `-P`.
    Firefox,
}

/// Browsers whose windows all belong to one process, so a window's argv says
/// nothing about which profile it shows and the profile list has to come from
/// the browser's own state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserConfig {
    pub binary: String,
    #[serde(default)]
    pub kind: BrowserKind,
    /// Chromium only: path to `Local State`, relative to the config dir.
    #[serde(default)]
    pub local_state: String,
    /// Firefox only: path to `profiles.ini`, relative to the home directory.
    #[serde(default)]
    pub profiles_ini: String,
    /// Profile directory -> workspace. Only mapped profiles are restored.
    #[serde(default)]
    pub profile_workspaces: HashMap<String, String>,
    #[serde(default)]
    pub default_workspace: Option<String>,
    /// Chromium only: pass `--restore-last-session` so each profile reopens
    /// its tabs whatever its startup setting says. Off by default.
    #[serde(default)]
    pub restore_last_session: bool,
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
fn default_tiled_map_timeout() -> u64 {
    10_000
}
fn default_abort_above() -> usize {
    3
}
fn default_drop_fraction() -> f64 {
    0.5
}
fn default_drop_window_secs() -> i64 {
    45
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
        // A nested Hyprland — a test or development compositor — shows on
        // the host as a window of its backend library, aquamarine. Saved, it
        // would be relaunched at boot as a compositor inside the session.
        "aquamarine",
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
        // A session manager, not a resumable TUI: its server owns the panes
        // and restores them itself, so relaunching the client with the argv
        // it was started with is the whole integration. Verified against
        // herdr 0.8.2: `--session <name>` survives in /proc/<pid>/cmdline, and
        // both `herdr --session <name>` and `herdr session attach <name>`
        // restart a stopped session. The descendant search stops at the
        // client, so nothing inside herdr is ever reconstructed twice.
        "herdr",
        "claude",
        "codex",
        "opencode",
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
            fd_glob: Some("/tmp/claude-*/*/{id}/*".to_string()),
            id_command: vec![],
            args: vec!["--resume".to_string(), "{id}".to_string()],
            fallback: vec!["--continue".to_string()],
            strip_flags: vec![
                "--resume".to_string(),
                "--continue".to_string(),
                "-c".to_string(),
            ],
        },
    );
    // codex keeps no per-session file open; its state database records the
    // working directory of every thread, so the session for this directory
    // can be looked up. The database name carries a migration number, hence
    // the glob for the newest one — which is the only reason this needs a
    // shell at all.
    //
    // The home directory and the query arrive as positional parameters ($1,
    // $2) rather than spliced into the script, so no directory name is ever
    // parsed as shell. 0.1.3 spliced `{cwd_sql}` into the script text, and a
    // directory named `$(…)` was executed on every save.
    rules.insert(
        "codex".to_string(),
        ResumeConfig {
            fd_glob: None,
            id_command: vec![
                "sh".to_string(),
                "-c".to_string(),
                "db=$(ls -t \"$1\"/.codex/state_*.sqlite 2>/dev/null | head -1); \
                 [ -n \"$db\" ] || exit 0; \
                 exec sqlite3 \"file:$db?mode=ro\" \"$2\" 2>/dev/null"
                    .to_string(),
                "sh".to_string(),
                "{home}".to_string(),
                "SELECT id FROM threads WHERE cwd = '{cwd_sql}' AND archived = 0 \
                 ORDER BY updated_at DESC LIMIT 1;"
                    .to_string(),
            ],
            args: vec!["resume".to_string(), "{id}".to_string()],
            fallback: vec!["resume".to_string(), "--last".to_string()],
            strip_flags: vec!["resume".to_string(), "--last".to_string()],
        },
    );
    // opencode keeps its sessions in SQLite, one row per conversation, keyed
    // by the directory it was started in. No shell: sqlite3 is run directly
    // and the query is an ordinary argument. Validated against 1.18.32:
    //
    // - `directory` is the physical working directory (what /proc/<pid>/cwd
    //   reports), not the git worktree root — even when entered through a
    //   symlink.
    // - Subagent runs are child sessions sharing their parent's directory,
    //   and are usually the newest row. Without `parent_id IS NULL` the
    //   restored terminal resumes *into a subagent*.
    // - `mode=ro` reads fine while opencode holds the database in WAL mode.
    // - `--session <id>` resumes in place; it forks only with `--fork`.
    // - `--continue` skips child sessions, but inside a git repository it is
    //   project-wide rather than per-directory: it can pick a sibling
    //   directory's conversation. Acceptable as the fallback, which is only
    //   reached when the exact lookup fails.
    // - `--prompt` is stripped: replaying it would resend the prompt on
    //   every restore.
    rules.insert(
        "opencode".to_string(),
        ResumeConfig {
            fd_glob: None,
            id_command: vec![
                "sqlite3".to_string(),
                "file:{home}/.local/share/opencode/opencode.db?mode=ro".to_string(),
                "SELECT id FROM session WHERE directory = '{cwd_sql}' \
                 AND parent_id IS NULL AND time_archived IS NULL \
                 ORDER BY time_updated DESC LIMIT 1;"
                    .to_string(),
            ],
            args: vec!["--session".to_string(), "{id}".to_string()],
            fallback: vec!["--continue".to_string()],
            strip_flags: vec![
                "--session".to_string(),
                "-s".to_string(),
                "--continue".to_string(),
                "-c".to_string(),
                "--fork".to_string(),
                "--prompt".to_string(),
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
            providers: BTreeMap::new(),
            monitors: MonitorConfig::default(),
            restore: RestoreConfig::default(),
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
            tiled_map_timeout_ms: default_tiled_map_timeout(),
            save_drop_fraction: default_drop_fraction(),
            save_drop_window_secs: default_drop_window_secs(),
            recheck_secs: default_recheck_secs(),
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
    fn a_nested_compositor_is_never_saved() {
        assert!(Config::default().is_ignored("aquamarine"));
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
        assert_eq!(from_file.restore, from_default.restore);
        assert_eq!(
            from_file.filters.ignore_classes,
            from_default.filters.ignore_classes
        );
    }

    #[test]
    fn foot_argv_uses_separate_directory_argument() {
        let c: Config = toml::from_str("").unwrap();
        let argv = c
            .terminal_for("foot")
            .unwrap()
            .build_argv("/home/user", None);
        assert_eq!(argv, vec!["foot", "-D", "/home/user"]);
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
    fn resume_rules_ship_for_the_agents_that_support_it() {
        let c: Config = toml::from_str("").unwrap();
        let claude = c.tui.resume.get("claude").expect("claude rule");
        assert!(
            claude.fd_glob.is_some(),
            "claude's id comes from an open file"
        );
        assert!(claude.id_command.is_empty());

        let codex = c.tui.resume.get("codex").expect("codex rule");
        assert!(codex.fd_glob.is_none(), "codex exposes no per-session file");
        assert!(!codex.id_command.is_empty(), "codex needs a lookup instead");
        assert_eq!(codex.fallback, vec!["resume", "--last"]);
    }

    #[test]
    fn a_resume_rule_can_be_defined_with_either_strategy() {
        let c: Config = toml::from_str(
            r#"
[tui.resume.myagent]
id_command = ["myagent", "current-session"]
args = ["--session", "{id}"]
"#,
        )
        .unwrap();
        let rule = c.tui.resume.get("myagent").unwrap();
        assert!(rule.fd_glob.is_none());
        assert_eq!(rule.args, vec!["--session", "{id}"]);
        assert!(rule.fallback.is_empty());
    }

    /// A working directory is attacker-controlled — a subdirectory of a cloned
    /// repository can be named anything — and every shipped id_command runs
    /// on every save. No directory name may reach a shell as code.
    #[test]
    fn a_hostile_directory_name_is_never_executed_by_a_shipped_rule() {
        let home = tempfile::TempDir::new().unwrap();
        // Give the codex rule a database to find, so its lookup runs past
        // the early exit and actually uses {cwd_sql}.
        std::fs::create_dir_all(home.path().join(".codex")).unwrap();
        std::fs::write(home.path().join(".codex/state_1.sqlite"), b"").unwrap();
        let marker_dir = tempfile::TempDir::new().unwrap();

        let c: Config = toml::from_str("").unwrap();
        let mut ran = Vec::new();
        for (name, rule) in &c.tui.resume {
            if rule.id_command.is_empty() {
                continue;
            }
            for (i, template) in [
                "/w/x$(touch {m})",
                "/w/x`touch {m}`",
                "/w/x\"; touch {m}; \"",
                "/w/x'; touch {m}; '",
            ]
            .iter()
            .enumerate()
            {
                let marker = marker_dir.path().join(format!("{name}-{i}"));
                let cwd = template.replace("{m}", &marker.to_string_lossy());
                if let Some(command) = crate::resume::render_command(
                    &rule.id_command,
                    &cwd,
                    &home.path().to_string_lossy(),
                ) {
                    let _ =
                        crate::resume::run_id_command(&command, std::time::Duration::from_secs(5));
                }
                if marker.exists() {
                    ran.push(format!("{name}: {template}"));
                }
            }
        }
        assert!(ran.is_empty(), "directory names executed as code: {ran:#?}");
    }

    /// A shipped rule must never need the guard: values reach any script as
    /// positional parameters, not spliced into its text.
    #[test]
    fn no_shipped_rule_splices_a_value_into_script_text() {
        let c: Config = toml::from_str("").unwrap();
        for (name, rule) in &c.tui.resume {
            if let Some(i) = crate::resume::script_argument(&rule.id_command) {
                let script = &rule.id_command[i];
                for placeholder in ["{cwd}", "{cwd_sql}", "{home}"] {
                    assert!(
                        !script.contains(placeholder),
                        "{name} splices {placeholder} into script text"
                    );
                }
            }
        }
    }

    fn sqlite(db: &std::path::Path, sql: &str) -> bool {
        std::process::Command::new("sqlite3")
            .arg(db)
            .arg(sql)
            .status()
            .is_ok_and(|s| s.success())
    }

    /// Run a shipped rule against a real database. Directory names that are
    /// awkward but legitimate must still resolve to their own session, and a
    /// hostile one must resolve as *data* — its own id — rather than run.
    fn check_rule_against(name: &str, home: &std::path::Path, rows: &[(&str, &str)]) {
        let c: Config = toml::from_str("").unwrap();
        let rule = c.tui.resume.get(name).unwrap();
        let marker = home.join("executed");
        for (cwd, id) in rows {
            let cwd = cwd.replace("{m}", &marker.to_string_lossy());
            let command =
                crate::resume::render_command(&rule.id_command, &cwd, &home.to_string_lossy())
                    .unwrap_or_else(|| panic!("{name}: refused {cwd:?}"));
            let got = crate::resume::run_id_command(&command, std::time::Duration::from_secs(5));
            assert_eq!(got.as_deref(), Some(*id), "{name}: {cwd:?}");
        }
        assert!(!marker.exists(), "{name}: a directory name was executed");
    }

    const AWKWARD: &[(&str, &str)] = &[
        ("/w/plain", "id-plain"),
        ("/w/Project (old)", "id-parens"),
        ("/w/it's", "id-quote"),
        ("/w/Проекти", "id-cyrillic"),
        ("/w/$(touch {m})", "id-dollar"),
        ("/w/`touch {m}`", "id-backtick"),
        ("/w/x\"; touch {m}; \"", "id-dquote"),
    ];

    fn sql_quote(s: &str) -> String {
        s.replace('\'', "''")
    }

    #[test]
    fn the_codex_rule_resolves_awkward_directories_as_data() {
        if which::which("sqlite3").is_err() {
            eprintln!("skipped: sqlite3 not installed");
            return;
        }
        let home = tempfile::TempDir::new().unwrap();
        let marker = home.path().join("executed");
        std::fs::create_dir_all(home.path().join(".codex")).unwrap();
        let db = home.path().join(".codex/state_5.sqlite");
        let mut sql = String::from(
            "CREATE TABLE threads (id TEXT, cwd TEXT, archived INTEGER, updated_at INTEGER);",
        );
        for (i, (cwd, id)) in AWKWARD.iter().enumerate() {
            let cwd = cwd.replace("{m}", &marker.to_string_lossy());
            sql.push_str(&format!(
                "INSERT INTO threads VALUES ('{id}', '{}', 0, {i});",
                sql_quote(&cwd)
            ));
        }
        assert!(sqlite(&db, &sql));
        check_rule_against("codex", home.path(), AWKWARD);
    }

    #[test]
    fn the_opencode_rule_resolves_awkward_directories_as_data() {
        if which::which("sqlite3").is_err() {
            eprintln!("skipped: sqlite3 not installed");
            return;
        }
        let home = tempfile::TempDir::new().unwrap();
        let marker = home.path().join("executed");
        let dir = home.path().join(".local/share/opencode");
        std::fs::create_dir_all(&dir).unwrap();
        let mut sql = String::from(
            "CREATE TABLE session (id TEXT, directory TEXT, parent_id TEXT, \
             time_archived INTEGER, time_updated INTEGER);",
        );
        for (i, (cwd, id)) in AWKWARD.iter().enumerate() {
            let cwd = sql_quote(&cwd.replace("{m}", &marker.to_string_lossy()));
            sql.push_str(&format!(
                "INSERT INTO session VALUES ('{id}', '{cwd}', NULL, NULL, {i});"
            ));
            // A newer subagent session in the same directory, as opencode
            // writes one per Task call: it must never be the one resumed.
            sql.push_str(&format!(
                "INSERT INTO session VALUES ('{id}-child', '{cwd}', '{id}', NULL, {});",
                i + 100
            ));
            // And a newer archived one, likewise.
            sql.push_str(&format!(
                "INSERT INTO session VALUES ('{id}-archived', '{cwd}', NULL, 1, {});",
                i + 200
            ));
        }
        assert!(sqlite(&dir.join("opencode.db"), &sql));
        check_rule_against("opencode", home.path(), AWKWARD);
    }

    #[test]
    fn a_missing_database_falls_back_rather_than_failing() {
        let home = tempfile::TempDir::new().unwrap();
        let c: Config = toml::from_str("").unwrap();
        for name in ["codex", "opencode"] {
            let rule = c.tui.resume.get(name).unwrap();
            let command = crate::resume::render_command(
                &rule.id_command,
                "/w",
                &home.path().to_string_lossy(),
            )
            .unwrap();
            assert_eq!(
                crate::resume::run_id_command(&command, std::time::Duration::from_secs(5)),
                None,
                "{name}"
            );
            assert!(!rule.fallback.is_empty(), "{name} has a fallback");
        }
        assert!(
            !home
                .path()
                .join(".local/share/opencode/opencode.db")
                .exists(),
            "a read-only lookup must never create the database"
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
