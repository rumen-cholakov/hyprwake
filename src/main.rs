use clap::{Parser, Subcommand};
use hyprwake::autosave;
use hyprwake::config::{config_path, load_config, sessions_dir, Config};
use hyprwake::doctor;
use hyprwake::hyprctl::RealHyprctl;
use hyprwake::omarchy;
use hyprwake::process::RealProcessInfo;
use hyprwake::restore::{restore_session, RestoreOptions};
use hyprwake::save::{perform_save, SaveOutcome};
use hyprwake::service;
use hyprwake::session::{
    delete_session, list_autosave_sessions, list_sessions, load_session, parse_max_age,
    session_exists, SessionError,
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
        /// Skip the restore when the session is older than this (30m, 24h, 7d)
        #[arg(long)]
        max_age: Option<String>,
        /// Restore even though windows are already open
        #[arg(short, long)]
        force: bool,
        /// Open only what is missing, leaving open windows alone
        #[arg(long)]
        missing_only: bool,
    },
    /// List saved sessions
    List,
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
            max_age,
            force,
            missing_only,
        } => cmd_restore(
            name,
            dry_run,
            max_age,
            force,
            missing_only,
            &config,
            &dir,
            cli.verbose,
        ),
        Commands::List => cmd_list(&dir, cli.verbose),
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

#[allow(clippy::too_many_arguments)]
fn cmd_restore(
    name: Option<String>,
    dry_run: bool,
    max_age: Option<String>,
    force: bool,
    missing_only: bool,
    config: &Config,
    dir: &Path,
    verbose: bool,
) -> ExitCode {
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

    let opts = RestoreOptions {
        dry_run,
        verbose,
        force,
        missing_only,
    };
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
            let mut keys: Vec<_> = config.terminals.keys().cloned().collect();
            keys.sort();
            keys.join(", ")
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
fn seeded_config() -> String {
    let defaults: Config = toml::from_str("").expect("defaults parse");
    let mut out = String::from(
        "# hyprwake configuration\n\
         # Every key is optional; what follows was detected on this machine.\n\n",
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

    let mut terminals: Vec<_> = defaults
        .terminals
        .iter()
        .filter(|(_, t)| which::which(&t.binary).is_ok())
        .collect();
    terminals.sort_by_key(|(class, _)| class.to_string());
    if !terminals.is_empty() {
        out.push_str("# Terminals found here. Listing any replaces the built-in table,\n");
        out.push_str("# so keep every terminal you use.\n");
        for (class, term) in terminals {
            out.push_str(&format!("[terminals.\"{class}\"]\n"));
            out.push_str(&format!("binary = \"{}\"\n", term.binary));
            out.push_str(&format!("cwd_flag = \"{}\"\n", term.cwd_flag));
            if let Some(exec) = &term.exec_flag {
                out.push_str(&format!("exec_flag = \"{exec}\"\n"));
            }
            if !term.extra_args.is_empty() {
                out.push_str(&format!(
                    "extra_args = [{}]\n",
                    term.extra_args
                        .iter()
                        .map(|a| format!("\"{a}\""))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            out.push('\n');
        }
    }

    out.push_str("# Chromium-family browsers: map profiles to workspaces to have each\n");
    out.push_str("# profile reopened in its own window. Without a mapping the browser is\n");
    out.push_str("# restored as an ordinary window.\n");
    for (class, browser) in hyprwake::browsers::known_browsers() {
        if !hyprwake::browsers::state_path(&browser).exists() {
            continue;
        }
        out.push_str(&format!("# [browsers.\"{class}\"]\n"));
        out.push_str(&format!("# binary = \"{}\"\n", browser.binary));
        match browser.kind {
            hyprwake::config::BrowserKind::Chromium => {
                out.push_str("# kind = \"chromium\"\n");
                out.push_str(&format!("# local_state = \"{}\"\n", browser.local_state));
                out.push_str("# profile_workspaces = { \"Default\" = \"2\" }\n\n");
            }
            hyprwake::config::BrowserKind::Firefox => {
                out.push_str("# kind = \"firefox\"\n");
                out.push_str(&format!("# profiles_ini = \"{}\"\n", browser.profiles_ini));
                out.push_str("# profile_workspaces = { \"default-release\" = \"2\" }\n\n");
            }
        }
    }

    out
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
        Ok(session) => println!(
            "Session: {} — {} window(s), saved {}",
            session.name,
            session.clients.len(),
            session
                .created_at
                .with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
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
