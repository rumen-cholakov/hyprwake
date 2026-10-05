use clap::{Parser, Subcommand};
use hyprwake::autosave;
use hyprwake::config::{config_path, load_config, sessions_dir, Config};
use hyprwake::diff;
use hyprwake::doctor;
use hyprwake::hyprctl::RealHyprctl;
use hyprwake::omarchy;
use hyprwake::process::RealProcessInfo;
use hyprwake::restore::{restore_session, RestoreOptions};
use hyprwake::save::{perform_save, SaveOutcome};
use hyprwake::service;
use hyprwake::session::{
    delete_session, list_autosave_sessions, list_sessions, load_session, parse_max_age,
    save_session, session_exists, SessionError, SCHEMA_VERSION,
};
use hyprwake::support;
use std::path::Path;
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    name = "hyprwake",
    version,
    about = "Save and restore Hyprland sessions",
    long_about = "Save and restore Hyprland sessions: which windows were open, on which \
                  workspace, which directory each terminal was in and what it was running."
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Show per-window detail
    #[arg(short, long, global = true)]
    verbose: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum Topology {
    /// Fit the session to whatever monitors are attached ([monitors] config)
    Adapt,
    /// Only onto the monitors it was saved on
    Saved,
}

#[derive(Subcommand)]
enum Commands {
    /// Snapshot the current session
    Save {
        /// Session name (default: the configured default session)
        name: Option<String>,
        /// Overwrite a named session without complaint
        #[arg(short, long)]
        force: bool,
    },
    /// Reopen a saved session
    Restore {
        name: Option<String>,
        /// Print what would happen, dispatch nothing
        #[arg(short, long)]
        dry_run: bool,
        /// With --dry-run: print the restore plan as JSON actions
        #[arg(long, requires = "dry_run")]
        json: bool,
        /// Skip the restore when the session is older than this (30m, 24h, 7d)
        #[arg(long)]
        max_age: Option<String>,
        /// Restore even though windows are already open
        #[arg(short, long)]
        force: bool,
        /// Open only what is missing, leaving open windows alone
        #[arg(long)]
        missing_only: bool,
        /// `saved`: refuse unless the monitors it was saved on are attached,
        /// and rebuild that layout exactly
        #[arg(long, value_enum, default_value_t = Topology::Adapt)]
        topology: Topology,
    },
    /// List saved sessions
    List,
    /// Compare a saved session with the current desktop
    Diff { name: Option<String> },
    /// Delete a saved session
    Delete { name: String },
    /// Show or create the config file
    Config {
        /// Write a config seeded from this machine
        #[arg(long)]
        init: bool,
        /// Overwrite an existing config
        #[arg(long)]
        force: bool,
    },
    /// Save whenever the window layout settles (event-driven)
    Watch {
        name: Option<String>,
        /// Stop a watcher that is already running and take over
        #[arg(long)]
        replace: bool,
    },
    /// Save on a fixed interval
    Daemon { name: Option<String> },
    /// Timestamped snapshots on a systemd timer
    Autosave {
        /// Snapshot now and rotate old autosaves
        #[arg(long)]
        now: bool,
        #[arg(long)]
        install: bool,
        #[arg(long)]
        uninstall: bool,
    },
    /// Wire restore and autosave into the desktop
    Install {
        /// Age beyond which a session is not restored at boot
        #[arg(long, default_value = "7d")]
        max_age: String,
    },
    /// Remove the desktop wiring
    Uninstall,
    /// Show whether automatic session restoration is ready
    Status,
    /// Write a sanitized diagnostic report for a support request
    SupportBundle {
        /// Destination path; defaults to a timestamped file in this directory
        #[arg(short, long, value_name = "PATH")]
        output: Option<std::path::PathBuf>,
    },
    /// Show which provider handles each window class, and why
    Providers {
        /// Explain how this one window class resolves
        class: Option<String>,
    },
    /// Check what would be saved and restored right now
    Doctor {
        /// Emit checks as JSON for scripts and compatibility reports
        #[arg(long)]
        json: bool,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let config = load_config();
    let dir = sessions_dir();

    match cli.command {
        Commands::Save { name, force } => cmd_save(name, force, &config, &dir, cli.verbose),
        Commands::Restore {
            name,
            dry_run,
            json,
            max_age,
            force,
            missing_only,
            topology,
        } => cmd_restore(
            name,
            RestoreFlags {
                dry_run,
                json,
                force,
                missing_only,
                saved_topology: topology == Topology::Saved,
                verbose: cli.verbose,
            },
            max_age,
            &config,
            &dir,
        ),
        Commands::List => cmd_list(&dir, cli.verbose),
        Commands::Diff { name } => cmd_diff(name, &config, &dir),
        Commands::Delete { name } => cmd_delete(&name, &dir),
        Commands::Config { init, force } => cmd_config(init, force, &config),
        Commands::Watch { name, replace } => cmd_watch(name, replace, &config, &dir),
        Commands::Daemon { name } => cmd_daemon(name, &config, &dir),
        Commands::Autosave {
            now,
            install,
            uninstall,
        } => cmd_autosave(now, install, uninstall, &config, &dir),
        Commands::Install { max_age } => cmd_install(&max_age),
        Commands::Uninstall => cmd_uninstall(),
        Commands::Status => cmd_status(&config, &dir),
        Commands::SupportBundle { output } => cmd_support_bundle(output, &config, &dir),
        Commands::Providers { class } => cmd_providers(class, &config),
        Commands::Doctor { json } => cmd_doctor(&config, &dir, json),
    }
}

fn session_name(explicit: Option<String>, config: &Config) -> String {
    explicit.unwrap_or_else(|| config.general.default_session.clone())
}

fn cmd_save(
    name: Option<String>,
    force: bool,
    config: &Config,
    dir: &Path,
    verbose: bool,
) -> ExitCode {
    let name = session_name(name, config);
    let is_default = name == config.general.default_session;
    if !force && !is_default && session_exists(&name, dir) {
        eprintln!("Session '{name}' already exists. Use --force to overwrite.");
        return ExitCode::FAILURE;
    }

    match perform_save(&name, dir, config, &RealHyprctl, &RealProcessInfo, force) {
        Ok(SaveOutcome::Saved(count)) => {
            println!("Saved '{name}': {count} window(s)");
            if verbose {
                if let Ok(session) = load_session(&name, dir) {
                    for c in &session.clients {
                        println!(
                            "  {:<18} ws {:<14} {}{}",
                            c.class,
                            c.workspace.selector(),
                            hyprwake::lua::shell_join(&c.launch.argv),
                            if c.launch.spawn {
                                ""
                            } else {
                                "   (placed by sweep)"
                            }
                        );
                    }
                }
            }
            ExitCode::SUCCESS
        }
        Ok(SaveOutcome::RefusedEmpty { kept }) => {
            println!("Nothing is open; kept the previous '{name}' ({kept} windows).");
            ExitCode::SUCCESS
        }
        Ok(SaveOutcome::RefusedDrop { kept, captured }) => {
            println!(
                "Only {captured} of {kept} windows are left, moments after the last save; \
                 kept the previous '{name}'. Use --force to record it anyway."
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("hyprwake: save failed: {e}");
            ExitCode::FAILURE
        }
    }
}

/// The session a restore was started from, kept so it survives the autosave
/// that follows. Restore it by name if the restore came back wrong.
const PRERESTORE: &str = "prerestore";

struct RestoreFlags {
    dry_run: bool,
    json: bool,
    force: bool,
    missing_only: bool,
    saved_topology: bool,
    verbose: bool,
}

fn cmd_restore(
    name: Option<String>,
    flags: RestoreFlags,
    max_age: Option<String>,
    config: &Config,
    dir: &Path,
) -> ExitCode {
    let RestoreFlags {
        dry_run,
        json,
        force,
        missing_only,
        saved_topology,
        verbose,
    } = flags;
    let name = session_name(name, config);
    let session = match load_session(&name, dir) {
        Ok(s) => s,
        Err(SessionError::NotFound(_)) => {
            // Fall back to the newest autosave, which is what an unattended
            // boot-time restore usually wants.
            match list_autosave_sessions(dir) {
                Ok(list) if !list.is_empty() => {
                    println!("No '{name}' session; falling back to '{}'.", list[0].name);
                    match load_session(&list[0].name, dir) {
                        Ok(s) => s,
                        Err(e) => {
                            eprintln!("hyprwake: {e}");
                            return ExitCode::FAILURE;
                        }
                    }
                }
                _ => {
                    eprintln!("hyprwake: no session '{name}' and no autosaves to fall back on.");
                    return ExitCode::FAILURE;
                }
            }
        }
        Err(e) => {
            eprintln!("hyprwake: {e}");
            return ExitCode::FAILURE;
        }
    };

    if let Some(limit) = &max_age {
        match parse_max_age(limit) {
            Ok(max) => {
                if chrono::Utc::now() - session.created_at > max {
                    println!(
                        "'{}' was saved {} — older than {limit}; not restoring.",
                        session.name,
                        session.created_at.format("%Y-%m-%d %H:%M")
                    );
                    return ExitCode::SUCCESS;
                }
            }
            Err(e) => {
                eprintln!("hyprwake: {e}");
                return ExitCode::FAILURE;
            }
        }
    }

    // Keep what is about to be restored. `hyprwake watch` saves over `latest`
    // seconds after the restored desktop settles, so without this a restore
    // that came back wrong also destroys the only record of what was right.
    if !dry_run {
        let mut kept = session.clone();
        kept.name = PRERESTORE.to_string();
        if let Err(e) = save_session(&kept, dir) {
            eprintln!("hyprwake: could not keep a copy as '{PRERESTORE}': {e}");
        }
    }

    let opts = RestoreOptions {
        dry_run,
        verbose,
        force,
        missing_only,
        saved_topology,
    };
    if json {
        return match hyprwake::restore::plan_restore(&session, &RealHyprctl, config, &opts) {
            Ok(plan) => match serde_json::to_string_pretty(&plan) {
                Ok(out) => {
                    println!("{out}");
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("hyprwake: could not serialize the plan: {e}");
                    ExitCode::FAILURE
                }
            },
            Err(e) => {
                eprintln!("hyprwake: {e}");
                ExitCode::FAILURE
            }
        };
    }
    match restore_session(&session, &RealHyprctl, config, &opts) {
        Ok(report) => {
            for line in &report.details {
                println!("{line}");
            }
            if dry_run {
                println!("[dry-run] {} window(s) would be launched", report.spawned);
            } else {
                println!(
                    "Restored '{}': {} launched, {} placed, {} failed, {} never appeared",
                    session.name, report.spawned, report.placed, report.failed, report.unmatched
                );
                if report.already_open > 0 {
                    println!("  {} window(s) were already open", report.already_open);
                }
                if report.guessed > 0 {
                    println!(
                        "  {} window(s) matched by saved order alone, among windows \
                         nothing else told apart; `--verbose` names them",
                        report.guessed
                    );
                }
                if report.groups_unrestored > 0 {
                    println!(
                        "  {} group(s) came back as separate windows",
                        report.groups_unrestored
                    );
                }
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("hyprwake: {e}");
            ExitCode::FAILURE
        }
    }
}

fn cmd_list(dir: &Path, verbose: bool) -> ExitCode {
    match list_sessions(dir) {
        Ok(sessions) if sessions.is_empty() => {
            println!("No saved sessions. Run `hyprwake save`.");
            ExitCode::SUCCESS
        }
        Ok(sessions) => {
            for s in &sessions {
                println!(
                    "{:<28} {:>3} window(s)  {}",
                    s.name,
                    s.client_count,
                    s.created_at
                        .with_timezone(&chrono::Local)
                        .format("%Y-%m-%d %H:%M")
                );
                if verbose {
                    if let Ok(full) = load_session(&s.name, dir) {
                        for c in &full.clients {
                            println!("    {:<18} ws {}", c.class, c.workspace.selector());
                        }
                    }
                }
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("hyprwake: {e}");
            ExitCode::FAILURE
        }
    }
}

fn cmd_diff(name: Option<String>, config: &Config, dir: &Path) -> ExitCode {
    let name = session_name(name, config);
    let saved = match load_session(&name, dir) {
        Ok(session) => session,
        Err(e) => {
            eprintln!("hyprwake: {e}");
            return ExitCode::FAILURE;
        }
    };
    let live = match hyprwake::hyprctl::HyprctlClient::get_clients(&RealHyprctl) {
        Ok(clients) => clients,
        Err(e) => {
            eprintln!("hyprwake: could not read the current desktop: {e}");
            return ExitCode::FAILURE;
        }
    };
    let result = diff::compare(&saved, &live, config);
    if result.is_empty() {
        println!("'{}' matches the current desktop.", saved.name);
        return ExitCode::SUCCESS;
    }
    for item in &result.missing {
        println!(
            "Missing:    {} × {} on workspace {}",
            item.count, item.class, item.workspace
        );
    }
    for item in &result.unexpected {
        println!(
            "Unexpected: {} × {} on workspace {}",
            item.count, item.class, item.workspace
        );
    }
    for item in &result.moved {
        println!(
            "Moved:      {} × {} from workspace {} to {}",
            item.count, item.class, item.from, item.to
        );
    }
    ExitCode::SUCCESS
}

fn cmd_delete(name: &str, dir: &Path) -> ExitCode {
    match delete_session(name, dir) {
        Ok(()) => {
            println!("Deleted '{name}'.");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("hyprwake: {e}");
            ExitCode::FAILURE
        }
    }
}

fn cmd_config(init: bool, force: bool, config: &Config) -> ExitCode {
    let path = config_path();
    if !init {
        println!("Config:   {}", path.display());
        println!("Sessions: {}", sessions_dir().display());
        println!("Log:      {}", hyprwake::logging::log_path().display());
        println!();
        println!("Terminals recognised: {}", {
            let providers = hyprwake::provider::ProviderSet::from_config(config);
            let mut classes: Vec<_> = providers
                .terminals()
                .flat_map(|(p, _)| p.entry.classes.clone())
                .collect();
            classes.sort();
            classes.join(", ")
        });
        println!("Launch through uwsm:  {}", config.use_uwsm());
        if !path.exists() {
            println!();
            println!(
                "No config file yet; defaults are in use. `hyprwake config --init` writes one."
            );
        }
        return ExitCode::SUCCESS;
    }

    if path.exists() && !force {
        eprintln!(
            "{} already exists. Use --force to overwrite.",
            path.display()
        );
        return ExitCode::FAILURE;
    }
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            eprintln!("hyprwake: {e}");
            return ExitCode::FAILURE;
        }
    }
    match std::fs::write(&path, seeded_config()) {
        Ok(()) => {
            println!("Wrote {}", path.display());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("hyprwake: {e}");
            ExitCode::FAILURE
        }
    }
}

/// A config file describing what is actually installed on this machine.
///
/// Written in the `[providers]` form. Terminals are only listed: the
/// built-in providers already handle every one found, and a `[providers]`
/// entry adds to the built-ins rather than replacing them, so repeating them
/// would only shadow identical defaults.
fn seeded_config() -> String {
    let defaults: Config = toml::from_str("").expect("defaults parse");
    let providers = hyprwake::provider::ProviderSet::from_config(&defaults);
    let mut out = String::from(
        "# hyprwake configuration\n\
         # Every key is optional; what follows was detected on this machine.\n\
         # `hyprwake providers` shows which provider handles each window class.\n\n",
    );

    out.push_str("[general]\n");
    out.push_str(&format!(
        "default_session = \"{}\"\n\
         sweep_timeout_secs = {}\n\
         debounce_ms = {}\n\n",
        defaults.general.default_session,
        defaults.general.sweep_timeout_secs,
        defaults.general.debounce_ms
    ));

    out.push_str("[launch]\n");
    out.push_str(&format!(
        "# uwsm-app puts restored apps in their own systemd scopes.\nuse_uwsm = {}\n\n",
        which::which("uwsm-app").is_ok()
    ));

    let mut found: Vec<String> = providers
        .terminals()
        .filter(|(_, t)| which::which(&t.binary).is_ok())
        .flat_map(|(p, _)| p.entry.classes.clone())
        .collect();
    found.sort();
    if found.is_empty() {
        out.push_str("# No terminal with a built-in provider is installed here.\n");
    } else {
        out.push_str(&format!(
            "# Terminals found here, handled by built-in providers: {}.\n",
            found.join(", ")
        ));
    }
    out.push_str("# To add another, declare it; see docs/PROVIDERS.md.\n");
    out.push_str(&commented(TERMINAL_EXAMPLE));
    out.push('\n');

    let mut browsers: Vec<_> = hyprwake::browsers::known_browsers()
        .into_iter()
        .filter(|(_, b)| hyprwake::browsers::state_path(b).exists())
        .collect();
    browsers.sort_by(|a, b| a.0.cmp(&b.0));
    if !browsers.is_empty() {
        out.push_str("# Browsers found here. Map profiles to workspaces to have each profile\n");
        out.push_str("# reopened in its own window; without a mapping the browser is restored\n");
        out.push_str("# as an ordinary window.\n");
        for (class, browser) in browsers {
            out.push_str(&commented(&browser_example(&class, &browser)));
            out.push('\n');
        }
    }
    out
}

const TERMINAL_EXAMPLE: &str = "[providers.wezterm]
kind = \"terminal\"
classes = [\"org.wezfurlong.wezterm\"]
binary = \"wezterm\"
cwd_flag = \"--cwd\"
";

/// A `[providers]` entry for a detected browser, uncommented.
fn browser_example(class: &str, browser: &hyprwake::config::BrowserConfig) -> String {
    let mut out = format!(
        "[providers.\"{class}\"]\nclasses = [\"{class}\"]\nbinary = \"{}\"\n",
        browser.binary
    );
    match browser.kind {
        hyprwake::config::BrowserKind::Chromium => {
            out.push_str("kind = \"chromium\"\n");
            out.push_str(&format!("local_state = \"{}\"\n", browser.local_state));
            out.push_str("profile_workspaces = { \"Default\" = \"2\" }\n");
            out.push_str("# Reopen each profile's tabs whatever its startup setting says.\n");
            out.push_str("restore_last_session = false\n");
        }
        hyprwake::config::BrowserKind::Firefox => {
            out.push_str("kind = \"firefox\"\n");
            out.push_str(&format!("profiles_ini = \"{}\"\n", browser.profiles_ini));
            out.push_str("profile_workspaces = { \"default-release\" = \"2\" }\n");
        }
    }
    out
}

/// Comment out every line, keeping comments that are already comments.
fn commented(block: &str) -> String {
    block
        .lines()
        .map(|l| {
            if l.starts_with('#') {
                format!("{l}\n")
            } else {
                format!("# {l}\n")
            }
        })
        .collect()
}

fn cmd_watch(name: Option<String>, replace: bool, config: &Config, dir: &Path) -> ExitCode {
    let name = session_name(name, config);
    match autosave::watch(&name, dir, config, replace) {
        Ok(()) => ExitCode::SUCCESS,
        // Already running is the desired state, not an error: this is what
        // "make sure the watcher is up" looks like when it already is.
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            println!("hyprwake: {e}; nothing to do. Use --replace to take over.");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("hyprwake: watch failed: {e}");
            eprintln!("hyprwake: `hyprwake daemon` polls instead and needs no event socket.");
            ExitCode::FAILURE
        }
    }
}

fn cmd_daemon(name: Option<String>, config: &Config, dir: &Path) -> ExitCode {
    let name = session_name(name, config);
    println!(
        "Saving '{name}' every {}s.",
        config.general.save_interval_secs
    );
    autosave::poll_daemon(&name, dir, config)
}

fn cmd_autosave(
    now: bool,
    install: bool,
    uninstall: bool,
    config: &Config,
    dir: &Path,
) -> ExitCode {
    let systemd_dir = autosave::systemd_user_dir();

    if install {
        return match autosave::install(&systemd_dir) {
            Ok((service, timer)) => {
                println!("Wrote {}", service.display());
                println!("Wrote {}", timer.display());
                println!();
                println!("Enable it with:");
                println!("  systemctl --user daemon-reload");
                println!("  systemctl --user enable --now hyprwake-autosave.timer");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("hyprwake: {e}");
                ExitCode::FAILURE
            }
        };
    }
    if uninstall {
        return match autosave::uninstall(&systemd_dir) {
            Ok(()) => {
                println!("Removed the autosave timer.");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("hyprwake: {e}");
                ExitCode::FAILURE
            }
        };
    }
    if now {
        return match autosave::run_once(dir, config) {
            Ok((saved, pruned)) => {
                println!("Autosaved {saved} window(s); pruned {pruned} old autosave(s).");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("hyprwake: {e}");
                ExitCode::FAILURE
            }
        };
    }

    println!(
        "Timer: {}",
        if autosave::is_installed(&systemd_dir) {
            "installed"
        } else {
            "not installed"
        }
    );
    println!("Enabled: {}", autosave::is_enabled());
    println!("Active:  {}", autosave::is_active());
    println!("Retain:  {} autosaves", config.general.autosave_retain);
    ExitCode::SUCCESS
}

/// Install the watcher unit and report what happened.
fn install_watch_service() {
    let dir = service::systemd_user_dir();
    match service::install(&dir) {
        Ok(path) => {
            println!("Wrote {}", path.display());
            if service::enable() {
                println!("Started {} (restarts itself if it dies)", service::UNIT);
            } else {
                println!(
                    "Enable it with: systemctl --user enable --now {}",
                    service::UNIT
                );
            }
        }
        Err(e) => eprintln!("hyprwake: could not write the watcher unit: {e}"),
    }
}

fn cmd_install(max_age: &str) -> ExitCode {
    install_watch_service();

    if !omarchy::is_omarchy() {
        println!();
        println!("This is not an Omarchy system, so there are no hooks to install.");
        println!("Add this to your Hyprland startup instead:\n");
        println!("{}", omarchy::autostart_snippet(max_age));
        return ExitCode::SUCCESS;
    }
    let Some(hooks) = omarchy::hooks_dir() else {
        eprintln!("hyprwake: cannot locate ~/.config/omarchy/hooks");
        return ExitCode::FAILURE;
    };
    match omarchy::install(&hooks, max_age) {
        Ok(installed) => {
            for path in &installed.written {
                println!("Wrote {}", path.display());
            }
            println!();
            println!("Restore runs at boot; the watcher keeps the session fresh.");
            println!("Save one now with `hyprwake save` so there is something to restore.");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("hyprwake: {e}");
            ExitCode::FAILURE
        }
    }
}

fn cmd_uninstall() -> ExitCode {
    match service::uninstall(&service::systemd_user_dir()) {
        Ok(true) => println!("Removed {}", service::UNIT),
        Ok(false) => {}
        Err(e) => eprintln!("hyprwake: could not remove the watcher unit: {e}"),
    }
    let Some(hooks) = omarchy::hooks_dir() else {
        println!("Nothing to remove.");
        return ExitCode::SUCCESS;
    };
    match omarchy::uninstall(&hooks) {
        Ok(removed) if removed.is_empty() => {
            println!("No hyprwake hooks were installed.");
            ExitCode::SUCCESS
        }
        Ok(removed) => {
            for path in removed {
                println!("Removed {}", path.display());
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("hyprwake: {e}");
            ExitCode::FAILURE
        }
    }
}

/// A deliberately small health summary for everyday use. `doctor` remains the
/// detailed diagnostic command, including checks that need a running
/// compositor; status must still be useful from a terminal after login.
fn cmd_status(config: &Config, dir: &Path) -> ExitCode {
    let default = &config.general.default_session;
    match load_session(default, dir) {
        Ok(session) => {
            println!(
                "Session: {} — {} window(s), saved {}",
                session.name,
                session.clients.len(),
                session
                    .created_at
                    .with_timezone(&chrono::Local)
                    .format("%Y-%m-%d %H:%M")
            );
            // Only worth a line when it is not the current one: an older
            // snapshot still restores, but it explains anything missing.
            let on_disk = session.loaded_from.unwrap_or(session.schema_version);
            if on_disk < SCHEMA_VERSION {
                println!(
                    "  Schema: v{on_disk}, migrated to v{} on load",
                    SCHEMA_VERSION
                );
            }
        }
        Err(SessionError::TooNew { found, supported }) => println!(
            "Session: unreadable — '{default}' is schema v{found}, this hyprwake reads up to v{supported}"
        ),
        Err(_) => println!("Session: none — run `hyprwake save`"),
    }

    let watcher_dir = service::systemd_user_dir();
    let watcher = if !service::is_installed(&watcher_dir) {
        "not installed"
    } else if service::is_active() {
        "running"
    } else {
        "installed, not running"
    };
    println!("Watcher: {watcher}");

    if omarchy::is_omarchy() {
        let hooks = omarchy::hooks_dir();
        let hooks = if hooks.as_deref().is_some_and(omarchy::is_installed) {
            "installed"
        } else {
            "not installed"
        };
        println!("Omarchy hooks: {hooks}");
    }

    let timer_dir = autosave::systemd_user_dir();
    let timer = if !autosave::is_installed(&timer_dir) {
        "not installed"
    } else if autosave::is_active() {
        "running"
    } else {
        "installed, not running"
    };
    println!("Autosave timer: {timer}");
    ExitCode::SUCCESS
}

fn cmd_support_bundle(output: Option<std::path::PathBuf>, config: &Config, dir: &Path) -> ExitCode {
    let path = output.unwrap_or_else(support::default_path);
    let checks = doctor::run(&RealHyprctl, &RealProcessInfo, config, dir);
    match support::write_bundle(&path, &checks) {
        Ok(()) => {
            println!("Wrote sanitized support bundle to {}", path.display());
            ExitCode::SUCCESS
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            eprintln!(
                "hyprwake: {} already exists; choose a new --output path",
                path.display()
            );
            ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!("hyprwake: could not write support bundle: {e}");
            ExitCode::FAILURE
        }
    }
}

/// What a compatibility report needs: which mechanism applies to which class,
/// and where that was declared.
fn cmd_providers(class: Option<String>, config: &Config) -> ExitCode {
    use hyprwake::provider::{Match, ProviderSet, ProviderSpec, GENERIC};
    let providers = ProviderSet::from_config(config);

    if let Some(class) = class {
        let r = providers.resolve(&class);
        let how = match &r.matched {
            Match::Exact => "claimed exactly".to_string(),
            Match::Pattern(p) => format!("matches the pattern \"{p}\""),
            Match::Generic => "nothing claims it, so its own argv is replayed".to_string(),
        };
        println!(
            "{class} → {} ({}, {}): {how}",
            r.key,
            r.spec.kind(),
            r.source.label()
        );
        if !r.spawn {
            println!("  recorded but never launched (spawn = false)");
        }
        if matches!(r.spec, ProviderSpec::Terminal(_)) {
            println!("  a TUI running inside it is reopened too; `hyprwake providers` lists which");
        }
        if r.spec
            .browser()
            .is_some_and(|b| b.profile_workspaces.is_empty())
        {
            println!("  no profile_workspaces, so it is restored as an ordinary window");
        }
        return ExitCode::SUCCESS;
    }

    let rows: Vec<[String; 5]> = providers
        .providers()
        .iter()
        .map(|p| {
            [
                p.key.clone(),
                p.entry.spec.kind().to_string(),
                p.entry.classes.join(", "),
                if p.entry.spawn { "yes" } else { "no" }.to_string(),
                p.source.label().to_string(),
            ]
        })
        .chain(std::iter::once([
            GENERIC.to_string(),
            "command".to_string(),
            "(any other class)".to_string(),
            "yes".to_string(),
            "built-in".to_string(),
        ]))
        .collect();
    let header = ["KEY", "KIND", "CLASSES", "SPAWN", "SOURCE"];
    let widths: Vec<usize> = (0..4)
        .map(|i| {
            rows.iter()
                .map(|r| r[i].chars().count())
                .chain(std::iter::once(header[i].len()))
                .max()
                .unwrap_or(0)
        })
        .collect();
    let line = |r: [&str; 5]| {
        println!(
            "{:<w0$}  {:<w1$}  {:<w2$}  {:<w3$}  {}",
            r[0],
            r[1],
            r[2],
            r[3],
            r[4],
            w0 = widths[0],
            w1 = widths[1],
            w2 = widths[2],
            w3 = widths[3]
        )
    };
    line(header);
    for r in &rows {
        line([&r[0], &r[1], &r[2], &r[3], &r[4]]);
    }

    println!();
    println!("Reopened inside a terminal, resumed where the program allows it:");
    let mut programs: Vec<String> = config
        .tui_programs()
        .into_iter()
        .map(|p| match config.tui.resume.get(&p) {
            Some(rule) if rule.fd_glob.is_some() => format!("{p} (by open file)"),
            Some(_) => format!("{p} (by lookup)"),
            None => p,
        })
        .collect();
    programs.sort();
    println!("  {}", programs.join(", "));

    if !providers.problems().is_empty() {
        println!();
        println!("Problems:");
        for problem in providers.problems() {
            println!("  {problem}");
        }
    }
    ExitCode::SUCCESS
}

fn cmd_doctor(config: &Config, dir: &Path, json: bool) -> ExitCode {
    let checks = doctor::run(&RealHyprctl, &RealProcessInfo, config, dir);
    let failed = checks
        .iter()
        .any(|check| check.status == doctor::Status::Fail);
    if json {
        match serde_json::to_string_pretty(&checks) {
            Ok(output) => println!("{output}"),
            Err(e) => {
                eprintln!("hyprwake: could not serialize doctor checks: {e}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        for check in &checks {
            println!(
                "[{}] {:<15} {}",
                check.status.marker(),
                check.name,
                check.detail
            );
        }
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hyprwake::provider::ProviderSet;

    #[test]
    fn the_seeded_config_parses_and_changes_nothing_builtin() {
        let seeded: Config = toml::from_str(&seeded_config()).expect("seeded config parses");
        let defaults: Config = toml::from_str("").unwrap();
        let (a, b) = (
            ProviderSet::from_config(&seeded),
            ProviderSet::from_config(&defaults),
        );
        for class in [
            "foot",
            "org.omarchy.terminal",
            "kitty",
            "Alacritty",
            "obsidian",
        ] {
            assert_eq!(a.resolve(class).key, b.resolve(class).key, "{class}");
        }
        assert!(a.problems().is_empty(), "{:?}", a.problems());
    }

    #[test]
    fn every_commented_example_parses_once_uncommented() {
        let chromium = hyprwake::config::BrowserConfig {
            binary: "google-chrome-stable".into(),
            kind: hyprwake::config::BrowserKind::Chromium,
            local_state: "google-chrome/Local State".into(),
            profiles_ini: String::new(),
            profile_workspaces: Default::default(),
            default_workspace: None,
            restore_last_session: false,
        };
        let firefox = hyprwake::config::BrowserConfig {
            binary: "zen-browser".into(),
            kind: hyprwake::config::BrowserKind::Firefox,
            local_state: String::new(),
            profiles_ini: ".zen/profiles.ini".into(),
            profile_workspaces: Default::default(),
            default_workspace: None,
            restore_last_session: false,
        };
        for (text, class, kind) in [
            (
                TERMINAL_EXAMPLE.to_string(),
                "org.wezfurlong.wezterm",
                "terminal",
            ),
            (
                browser_example("google-chrome", &chromium),
                "google-chrome",
                "chromium",
            ),
            (
                browser_example("zen-alpha", &firefox),
                "zen-alpha",
                "firefox",
            ),
        ] {
            let config: Config = toml::from_str(&text).unwrap_or_else(|e| panic!("{e}\n{text}"));
            let set = ProviderSet::from_config(&config);
            assert_eq!(set.resolve(class).spec.kind(), kind, "{text}");
        }
    }
}
