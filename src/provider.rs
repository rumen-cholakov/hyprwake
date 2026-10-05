//! Providers: which mechanism captures and relaunches a window of a class.
//!
//! The *mechanisms* are a closed set, chosen in Rust: a terminal reopened at
//! its shell's directory, a browser reopened profile by profile, a command
//! replayed from its own argv. Adding one is a Rust change, and the enum makes
//! the compiler name every site that has to handle it.
//!
//! The *instances* are open, declared in config. `[providers.<key>]` names a
//! mechanism with `kind` and claims window classes; the legacy `[terminals]`,
//! `[apps]` and `[browsers]` tables still work and are read into the same set,
//! so which behaviour applies to a class is one lookup rather than an if-else
//! chain in capture.
//!
//! ```toml
//! [providers.wezterm]
//! kind = "terminal"
//! classes = ["org.wezfurlong.wezterm"]
//! binary = "wezterm"
//! cwd_flag = "--cwd"
//! ```

use crate::config::{
    default_terminals, AppConfig, BrowserConfig, BrowserKind, Config, TerminalConfig,
};
use crate::external::ExternalConfig;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A mechanism, with the data one instance of it needs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ProviderSpec {
    /// Reopen at the directory its shell was in, and reopen a TUI running
    /// inside it — resuming the TUI's own session where `[tui.resume]` knows
    /// how.
    Terminal(TerminalConfig),
    /// A Chromium-family browser: profiles from `Local State`, each reopened
    /// with `--profile-directory`.
    Chromium(ChromiumConfig),
    /// A Firefox-family browser: profiles from `profiles.ini`, reopened by
    /// name with `-P`.
    Firefox(FirefoxConfig),
    /// Replay the window's own argv, optionally with a different binary or
    /// arguments stripped.
    Command(CommandConfig),
    /// Hand capture and relaunch to a helper program speaking JSON. See
    /// [`crate::external`] for the contract and what it is trusted with.
    External(ExternalConfig),
}

impl ProviderSpec {
    pub fn kind(&self) -> &'static str {
        match self {
            ProviderSpec::Terminal(_) => "terminal",
            ProviderSpec::Chromium(_) => "chromium",
            ProviderSpec::Firefox(_) => "firefox",
            ProviderSpec::Command(_) => "command",
            ProviderSpec::External(_) => "external",
        }
    }

    /// The profile configuration, for the browser kinds.
    pub fn browser(&self) -> Option<BrowserConfig> {
        match self {
            ProviderSpec::Chromium(c) => Some(BrowserConfig {
                binary: c.binary.clone(),
                kind: BrowserKind::Chromium,
                local_state: c.local_state.clone(),
                profiles_ini: String::new(),
                profile_workspaces: c.profile_workspaces.clone(),
                default_workspace: c.default_workspace.clone(),
                restore_last_session: c.restore_last_session,
            }),
            ProviderSpec::Firefox(f) => Some(BrowserConfig {
                binary: f.binary.clone(),
                kind: BrowserKind::Firefox,
                local_state: String::new(),
                profiles_ini: f.profiles_ini.clone(),
                profile_workspaces: f.profile_workspaces.clone(),
                default_workspace: f.default_workspace.clone(),
                restore_last_session: false,
            }),
            ProviderSpec::Terminal(_) | ProviderSpec::Command(_) | ProviderSpec::External(_) => {
                None
            }
        }
    }

    /// Argument hygiene for a window replayed from its own argv.
    fn strip_args(&self) -> &[String] {
        match self {
            ProviderSpec::Chromium(c) => &c.strip_args,
            ProviderSpec::Firefox(f) => &f.strip_args,
            ProviderSpec::Command(c) => &c.strip_args,
            ProviderSpec::Terminal(_) | ProviderSpec::External(_) => &[],
        }
    }
}

/// Unknown keys are an error here, unlike the legacy `[browsers]` table: an
/// option set on the wrong kind should fail loudly, not do nothing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChromiumConfig {
    pub binary: String,
    /// Path to `Local State`, relative to the config directory.
    pub local_state: String,
    /// Profile directory -> workspace. Only mapped profiles are reopened.
    #[serde(default)]
    pub profile_workspaces: HashMap<String, String>,
    #[serde(default)]
    pub default_workspace: Option<String>,
    /// Pass `--restore-last-session`, so each profile reopens its tabs even
    /// when its own startup setting is "Open the New Tab page". Off by
    /// default: it overrides a preference you may have set on purpose.
    #[serde(default)]
    pub restore_last_session: bool,
    /// Arguments dropped, by prefix, when the browser is relaunched as an
    /// ordinary window rather than profile by profile.
    #[serde(default)]
    pub strip_args: Vec<String>,
}

/// Firefox has no switch that forces session restore — only the
/// `browser.startup.page` preference — so there is no `restore_last_session`
/// here, and setting one is an error.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FirefoxConfig {
    pub binary: String,
    /// Path to `profiles.ini`, relative to the home directory.
    pub profiles_ini: String,
    /// Profile name -> workspace. Only mapped profiles are reopened.
    #[serde(default)]
    pub profile_workspaces: HashMap<String, String>,
    #[serde(default)]
    pub default_workspace: Option<String>,
    #[serde(default)]
    pub strip_args: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandConfig {
    /// Replace the binary; by default the captured argv is replayed as-is.
    #[serde(default)]
    pub binary: Option<String>,
    /// Arguments to drop, by prefix (session-scoped temp paths, tray flags).
    #[serde(default)]
    pub strip_args: Vec<String>,
}

/// One `[providers.<key>]` table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderEntry {
    /// Window classes this provider claims. An exact class beats a pattern;
    /// a pattern is a prefix ending in `*`, and the longest match wins.
    pub classes: Vec<String>,
    /// `false` records the window but never launches it: it is placed if the
    /// program reopens it by itself.
    #[serde(default = "yes")]
    pub spawn: bool,
    #[serde(flatten)]
    pub spec: ProviderSpec,
    /// Legacy `[apps]` overrides for a class a legacy browser also claims:
    /// 0.1.3 applied both. Nothing in the new form sets this.
    #[serde(skip)]
    pub overrides: Option<CommandConfig>,
}

fn yes() -> bool {
    true
}

/// Where an entry came from, for `hyprwake providers`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    BuiltIn,
    Terminals,
    Apps,
    Browsers,
    Providers,
}

impl Source {
    pub fn label(&self) -> &'static str {
        match self {
            Source::BuiltIn => "built-in",
            Source::Terminals => "[terminals]",
            Source::Apps => "[apps]",
            Source::Browsers => "[browsers]",
            Source::Providers => "[providers]",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Provider {
    pub key: String,
    pub entry: ProviderEntry,
    pub source: Source,
}

/// How a class found its provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Match {
    Exact,
    Pattern(String),
    /// Nothing claims the class; the built-in generic provider replays argv.
    Generic,
}

#[derive(Debug, Clone)]
pub struct Resolution<'a> {
    pub key: &'a str,
    pub spec: &'a ProviderSpec,
    pub spawn: bool,
    pub overrides: Option<&'a CommandConfig>,
    pub source: Source,
    pub matched: Match,
}

impl Resolution<'_> {
    /// The binary and strip rules applied to a window replayed from its own
    /// argv: a legacy `[apps]` overlay when there is one, otherwise the
    /// spec's own `strip_args`.
    pub fn command(&self) -> CommandConfig {
        if let Some(o) = self.overrides {
            return o.clone();
        }
        match self.spec {
            ProviderSpec::Command(c) => c.clone(),
            other => CommandConfig {
                binary: None,
                strip_args: other.strip_args().to_vec(),
            },
        }
    }
}

/// The key of the fallback provider. Reserved: a `[providers.generic]` is
/// reported and ignored rather than silently changing every unclaimed class.
pub const GENERIC: &str = "generic";

static GENERIC_SPEC: ProviderSpec = ProviderSpec::Command(CommandConfig {
    binary: None,
    strip_args: Vec::new(),
});

/// Every provider in effect, and whatever was wrong with how they were
/// declared.
#[derive(Debug, Clone)]
pub struct ProviderSet {
    providers: Vec<Provider>,
    problems: Vec<String>,
}

impl ProviderSet {
    /// Read the legacy tables and `[providers]` into one set.
    ///
    /// Legacy semantics are kept exactly, because an existing config file
    /// must capture the same argv it did under 0.1.3: an `[apps]` entry for a
    /// terminal's class contributes only `no_spawn`, and one for a browser's
    /// class still overrides that browser's window argv.
    pub fn from_config(config: &Config) -> Self {
        let mut providers: Vec<Provider> = Vec::new();
        let mut problems = Vec::new();
        let builtin_terminals = default_terminals();

        let mut terminals: Vec<_> = config.terminals.iter().collect();
        terminals.sort_by(|a, b| a.0.cmp(b.0));
        for (class, term) in terminals {
            let source = if builtin_terminals.get(class) == Some(term) {
                Source::BuiltIn
            } else {
                Source::Terminals
            };
            providers.push(legacy(class, ProviderSpec::Terminal(term.clone()), source));
        }

        let mut browsers: Vec<_> = config.browsers.iter().collect();
        browsers.sort_by(|a, b| a.0.cmp(b.0));
        for (class, browser) in browsers {
            if providers.iter().any(|p| p.key == *class) {
                problems.push(format!(
                    "[browsers.\"{class}\"] is ignored: \"{class}\" is already a terminal"
                ));
                continue;
            }
            if browser.kind == BrowserKind::Firefox && browser.restore_last_session {
                problems.push(format!(
                    "[browsers.\"{class}\"] restore_last_session has no effect on a firefox \
                     browser; Firefox restores tabs only through browser.startup.page"
                ));
            }
            providers.push(legacy(
                class,
                legacy_browser_spec(browser),
                Source::Browsers,
            ));
        }

        let mut apps: Vec<_> = config.apps.iter().collect();
        apps.sort_by(|a, b| a.0.cmp(b.0));
        for (class, app) in apps {
            if let Some(existing) = providers.iter_mut().find(|p| p.key == *class) {
                // 0.1.3: a terminal class ignored the app's binary and
                // strip_args but honoured no_spawn; a browser class applied
                // all three to its window argv.
                existing.entry.spawn &= !app.no_spawn;
                if existing.entry.spec.browser().is_some() {
                    existing.entry.overrides = Some(command_of(app));
                }
                continue;
            }
            let mut p = legacy(class, ProviderSpec::Command(command_of(app)), Source::Apps);
            p.entry.spawn = !app.no_spawn;
            providers.push(p);
        }

        for (key, entry) in &config.providers {
            if key == GENERIC {
                problems.push(format!(
                    "[providers.{GENERIC}] is ignored: the name is reserved for the fallback \
                     that replays argv"
                ));
                continue;
            }
            if entry.classes.is_empty() {
                problems.push(format!(
                    "[providers.{key}] claims no classes, so it is never used"
                ));
            }
            for class in &entry.classes {
                if class.contains('*') && !is_pattern(class) {
                    problems.push(format!(
                        "[providers.{key}] class \"{class}\": only a single trailing * is \
                         supported, so it will never match"
                    ));
                }
            }
            // A [providers] entry with a legacy or built-in key redefines it.
            providers.retain(|p| p.key != *key);
            providers.push(Provider {
                key: key.clone(),
                entry: entry.clone(),
                source: Source::Providers,
            });
        }

        providers.sort_by(|a, b| a.key.cmp(&b.key));
        let declared: Vec<&Provider> = providers
            .iter()
            .filter(|p| p.source == Source::Providers)
            .collect();
        for (i, a) in declared.iter().enumerate() {
            for b in &declared[i + 1..] {
                for class in a
                    .entry
                    .classes
                    .iter()
                    .filter(|c| b.entry.classes.contains(c))
                {
                    problems.push(format!(
                        "class \"{class}\" is claimed by both [providers.{}] and \
                         [providers.{}]; [providers.{}] wins",
                        a.key, b.key, a.key
                    ));
                }
            }
        }

        Self {
            providers,
            problems,
        }
    }

    /// Which provider handles `class`.
    ///
    /// Order-independent by construction, so the answer cannot depend on how
    /// a TOML parser happens to order tables: a `[providers]` exact claim,
    /// then any other exact claim, then the longest matching pattern, then
    /// generic. Ties go to the first key in sort order, which `from_config`
    /// has already reported.
    pub fn resolve(&self, class: &str) -> Resolution<'_> {
        let claims = |p: &&Provider| p.entry.classes.iter().any(|c| c == class);
        let found = self
            .providers
            .iter()
            .filter(|p| p.source == Source::Providers)
            .find(claims)
            .or_else(|| self.providers.iter().find(claims));
        if let Some(p) = found {
            return p.resolution(Match::Exact);
        }

        let mut best: Option<(&Provider, &str)> = None;
        for p in &self.providers {
            for pattern in p.entry.classes.iter().filter(|c| is_pattern(c)) {
                let prefix = &pattern[..pattern.len() - 1];
                if !class.starts_with(prefix) {
                    continue;
                }
                let better = match best {
                    None => true,
                    Some((current, current_prefix)) => {
                        prefix.len() > current_prefix.len()
                            || (prefix.len() == current_prefix.len()
                                && p.source == Source::Providers
                                && current.source != Source::Providers)
                    }
                };
                if better {
                    best = Some((p, prefix));
                }
            }
        }
        if let Some((p, prefix)) = best {
            return p.resolution(Match::Pattern(format!("{prefix}*")));
        }

        Resolution {
            key: GENERIC,
            spec: &GENERIC_SPEC,
            spawn: true,
            overrides: None,
            source: Source::BuiltIn,
            matched: Match::Generic,
        }
    }

    pub fn providers(&self) -> &[Provider] {
        &self.providers
    }

    pub fn problems(&self) -> &[String] {
        &self.problems
    }

    /// Terminal providers, for `doctor` and `config`.
    pub fn terminals(&self) -> impl Iterator<Item = (&Provider, &TerminalConfig)> {
        self.providers.iter().filter_map(|p| match &p.entry.spec {
            ProviderSpec::Terminal(t) => Some((p, t)),
            _ => None,
        })
    }

    /// A provider by its key, as recorded in a snapshot.
    pub fn get(&self, key: &str) -> Option<&Provider> {
        self.providers.iter().find(|p| p.key == key)
    }

    /// External providers, for `doctor`.
    pub fn externals(&self) -> impl Iterator<Item = (&Provider, &ExternalConfig)> {
        self.providers.iter().filter_map(|p| match &p.entry.spec {
            ProviderSpec::External(e) => Some((p, e)),
            _ => None,
        })
    }

    /// The browser configuration for a class, if a browser provider claims
    /// it.
    pub fn browser_for(&self, class: &str) -> Option<BrowserConfig> {
        self.resolve(class).spec.browser()
    }
}

impl Provider {
    fn resolution(&self, matched: Match) -> Resolution<'_> {
        Resolution {
            key: &self.key,
            spec: &self.entry.spec,
            spawn: self.entry.spawn,
            overrides: self.entry.overrides.as_ref(),
            source: self.source,
            matched,
        }
    }
}

/// A prefix followed by a single trailing `*`.
fn is_pattern(class: &str) -> bool {
    class.ends_with('*') && class.matches('*').count() == 1
}

fn legacy(class: &str, spec: ProviderSpec, source: Source) -> Provider {
    Provider {
        key: class.to_string(),
        entry: ProviderEntry {
            classes: vec![class.to_string()],
            spawn: true,
            spec,
            overrides: None,
        },
        source,
    }
}

fn legacy_browser_spec(browser: &BrowserConfig) -> ProviderSpec {
    match browser.kind {
        BrowserKind::Chromium => ProviderSpec::Chromium(ChromiumConfig {
            binary: browser.binary.clone(),
            local_state: browser.local_state.clone(),
            profile_workspaces: browser.profile_workspaces.clone(),
            default_workspace: browser.default_workspace.clone(),
            restore_last_session: browser.restore_last_session,
            strip_args: vec![],
        }),
        BrowserKind::Firefox => ProviderSpec::Firefox(FirefoxConfig {
            binary: browser.binary.clone(),
            profiles_ini: browser.profiles_ini.clone(),
            profile_workspaces: browser.profile_workspaces.clone(),
            default_workspace: browser.default_workspace.clone(),
            strip_args: vec![],
        }),
    }
}

fn command_of(app: &AppConfig) -> CommandConfig {
    CommandConfig {
        binary: app.binary.clone(),
        strip_args: app.strip_args.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(toml_text: &str) -> ProviderSet {
        let config: Config = toml::from_str(toml_text).unwrap();
        ProviderSet::from_config(&config)
    }

    fn parse_error(toml_text: &str) -> String {
        toml::from_str::<Config>(toml_text).unwrap_err().to_string()
    }

    #[test]
    fn with_no_config_every_builtin_terminal_is_a_provider() {
        let s = set("");
        for class in [
            "foot",
            "org.omarchy.terminal",
            "kitty",
            "Alacritty",
            "com.mitchellh.ghostty",
        ] {
            let r = s.resolve(class);
            assert_eq!(r.spec.kind(), "terminal", "{class}");
            assert_eq!(r.key, class);
            assert_eq!(r.source, Source::BuiltIn);
            assert_eq!(r.matched, Match::Exact);
        }
        assert!(s.problems().is_empty(), "{:?}", s.problems());
    }

    #[test]
    fn an_unclaimed_class_falls_through_to_generic() {
        let s = set("");
        let r = s.resolve("obsidian");
        assert_eq!(r.key, GENERIC);
        assert_eq!(r.spec.kind(), "command");
        assert_eq!(r.matched, Match::Generic);
        assert!(r.spawn);
        assert_eq!(r.command(), CommandConfig::default());
    }

    #[test]
    fn a_legacy_terminals_table_still_replaces_the_builtins() {
        let s = set("[terminals.foot]\nbinary = \"footclient\"\ncwd_flag = \"-D\"\n");
        assert_eq!(s.resolve("foot").source, Source::Terminals);
        assert_eq!(s.resolve("kitty").key, GENERIC, "0.1.3 replace semantics");
    }

    #[test]
    fn an_unchanged_builtin_in_a_legacy_table_is_still_labelled_built_in() {
        let s = set("[terminals.foot]\nbinary = \"foot\"\ncwd_flag = \"-D\"\n");
        assert_eq!(s.resolve("foot").source, Source::BuiltIn);
    }

    #[test]
    fn a_legacy_app_on_a_terminal_class_only_contributes_no_spawn() {
        let s = set("[apps.foot]\nno_spawn = true\nbinary = \"footclient\"\n");
        let r = s.resolve("foot");
        assert_eq!(r.spec.kind(), "terminal");
        assert!(!r.spawn);
        assert!(r.overrides.is_none());
    }

    #[test]
    fn a_legacy_app_on_a_browser_class_overrides_its_window_argv() {
        let s = set(
            "[browsers.google-chrome]\nbinary = \"google-chrome-stable\"\n\
             local_state = \"google-chrome/Local State\"\n\
             [apps.google-chrome]\nstrip_args = [\"--load-extension\"]\n",
        );
        let r = s.resolve("google-chrome");
        assert_eq!(r.spec.kind(), "chromium");
        assert_eq!(r.command().strip_args, vec!["--load-extension".to_string()]);
        assert!(r.spawn);
    }

    #[test]
    fn a_legacy_app_alone_is_a_command_provider() {
        let s = set("[apps.Spotify]\nno_spawn = true\n");
        let r = s.resolve("Spotify");
        assert_eq!(r.spec.kind(), "command");
        assert_eq!(r.source, Source::Apps);
        assert!(!r.spawn);
    }

    #[test]
    fn every_kind_parses_in_the_new_form() {
        let s = set(r#"
[providers.wezterm]
kind = "terminal"
classes = ["org.wezfurlong.wezterm"]
binary = "wezterm"
cwd_flag = "--cwd"

[providers.brave]
kind = "chromium"
classes = ["brave-browser"]
binary = "brave"
local_state = "BraveSoftware/Brave-Browser/Local State"
restore_last_session = true

[providers.zen]
kind = "firefox"
classes = ["zen", "zen-alpha"]
binary = "zen-browser"
profiles_ini = ".zen/profiles.ini"

[providers.steam]
kind = "command"
classes = ["steam"]
spawn = false
"#);
        assert_eq!(s.resolve("org.wezfurlong.wezterm").spec.kind(), "terminal");
        assert!(s.browser_for("brave-browser").unwrap().restore_last_session);
        assert_eq!(s.resolve("zen-alpha").key, "zen");
        assert!(!s.resolve("steam").spawn);
        assert!(s.problems().is_empty(), "{:?}", s.problems());
    }

    #[test]
    fn a_new_form_provider_adds_to_the_builtins_rather_than_replacing_them() {
        let s = set(
            "[providers.wezterm]\nkind = \"terminal\"\nclasses = [\"org.wezfurlong.wezterm\"]\n\
             binary = \"wezterm\"\ncwd_flag = \"--cwd\"\n",
        );
        assert_eq!(s.resolve("foot").source, Source::BuiltIn);
    }

    #[test]
    fn a_new_form_claim_beats_a_builtin_one() {
        let s = set(
            "[providers.myfoot]\nkind = \"terminal\"\nclasses = [\"foot\"]\n\
             binary = \"footclient\"\ncwd_flag = \"-D\"\n",
        );
        let r = s.resolve("foot");
        assert_eq!(r.key, "myfoot");
        assert_eq!(r.source, Source::Providers);
    }

    #[test]
    fn a_new_form_key_redefines_the_legacy_entry_of_that_name() {
        let s = set("[providers.foot]\nkind = \"command\"\nclasses = [\"something-else\"]\n");
        assert!(!s
            .providers()
            .iter()
            .any(|p| p.key == "foot" && p.source == Source::BuiltIn));
        assert_eq!(s.resolve("foot").key, GENERIC);
    }

    #[test]
    fn the_longest_pattern_wins_and_exact_beats_any_pattern() {
        let s = set(r#"
[providers.jet]
kind = "command"
classes = ["jetbrains-*"]

[providers.idea]
kind = "command"
classes = ["jetbrains-idea*"]

[providers.pycharm]
kind = "command"
classes = ["jetbrains-pycharm"]
"#);
        let idea = s.resolve("jetbrains-idea-ce");
        assert_eq!(idea.key, "idea");
        assert_eq!(idea.matched, Match::Pattern("jetbrains-idea*".into()));
        assert_eq!(s.resolve("jetbrains-goland").key, "jet");
        assert_eq!(s.resolve("jetbrains-pycharm").matched, Match::Exact);
        assert_eq!(s.resolve("jetbrains-pycharm").key, "pycharm");
    }

    #[test]
    fn a_duplicate_claim_is_reported_and_resolves_deterministically() {
        let s = set("[providers.b]\nkind = \"command\"\nclasses = [\"x\"]\n\
             [providers.a]\nkind = \"command\"\nclasses = [\"x\"]\n");
        assert_eq!(s.resolve("x").key, "a", "first key in sort order");
        assert_eq!(s.problems().len(), 1, "{:?}", s.problems());
        assert!(s.problems()[0].contains("claimed by both"));
    }

    #[test]
    fn declaration_mistakes_are_reported() {
        let s = set(
            "[providers.generic]\nkind = \"command\"\nclasses = [\"x\"]\n\
             [providers.empty]\nkind = \"command\"\nclasses = []\n\
             [providers.mid]\nkind = \"command\"\nclasses = [\"a*b\"]\n",
        );
        let all = s.problems().join("\n");
        assert!(all.contains("reserved"), "{all}");
        assert!(all.contains("claims no classes"), "{all}");
        assert!(all.contains("single trailing *"), "{all}");
        assert_eq!(s.resolve("x").key, GENERIC, "the reserved name is ignored");
        assert_eq!(
            s.resolve("aXb").key,
            GENERIC,
            "a mid-string * never matches"
        );
    }

    #[test]
    fn an_external_provider_parses_and_rejects_unknown_keys() {
        let s = set(
            "[providers.mail]\nkind = \"external\"\nclasses = [\"mailapp\"]\n\
             capture = [\"~/.config/hyprwake/providers/mail\", \"capture\"]\n\
             relaunch = [\"~/.config/hyprwake/providers/mail\", \"relaunch\"]\n",
        );
        let r = s.resolve("mailapp");
        assert_eq!(r.spec.kind(), "external");
        assert_eq!(s.externals().count(), 1);
        assert_eq!(s.get("mail").unwrap().key, "mail");
        let e = parse_error(
            "[providers.mail]\nkind = \"external\"\nclasses = [\"m\"]\ncapture = [\"/x\"]\n\
             plan = [\"/x\"]\n",
        );
        assert!(e.contains("plan"), "{e}");
        let e = parse_error("[providers.mail]\nkind = \"external\"\nclasses = [\"m\"]\n");
        assert!(e.contains("capture"), "{e}");
    }

    #[test]
    fn a_wrong_kind_is_a_parse_error() {
        let e = parse_error("[providers.x]\nkind = \"telepathy\"\nclasses = [\"x\"]\n");
        assert!(e.contains("telepathy"), "{e}");
    }

    #[test]
    fn a_terminal_without_a_cwd_flag_is_a_parse_error() {
        let e =
            parse_error("[providers.x]\nkind = \"terminal\"\nclasses = [\"x\"]\nbinary = \"x\"\n");
        assert!(e.contains("cwd_flag"), "{e}");
    }

    #[test]
    fn restore_last_session_on_firefox_is_a_parse_error() {
        let e = parse_error(
            "[providers.ff]\nkind = \"firefox\"\nclasses = [\"firefox\"]\nbinary = \"firefox\"\n\
             profiles_ini = \".mozilla/firefox/profiles.ini\"\nrestore_last_session = true\n",
        );
        assert!(e.contains("restore_last_session"), "{e}");
    }

    #[test]
    fn a_typo_in_a_command_provider_is_a_parse_error() {
        let e = parse_error(
            "[providers.x]\nkind = \"command\"\nclasses = [\"x\"]\nstrip_arg = [\"--a\"]\n",
        );
        assert!(e.contains("strip_arg"), "{e}");
    }

    #[test]
    fn a_parse_error_names_its_line() {
        let e = parse_error(
            "[general]\ndebounce_ms = 100\n\n[providers.x]\nkind = \"terminal\"\n\
             classes = [\"x\"]\nbinary = \"x\"\n",
        );
        assert!(e.contains("line 4") || e.contains("4 |"), "{e}");
    }

    #[test]
    fn restore_last_session_on_a_legacy_firefox_is_reported() {
        let s = set(
            "[browsers.firefox]\nbinary = \"firefox\"\nkind = \"firefox\"\n\
             profiles_ini = \".mozilla/firefox/profiles.ini\"\nrestore_last_session = true\n",
        );
        assert!(s.problems().iter().any(|p| p.contains("no effect")));
    }
}
