use clap::{Parser, Subcommand};
use hyprflow::capture::capture_session;
use hyprflow::config::{config_path, load_config, sessions_dir};
use hyprflow::hyprctl::RealHyprctl;
use hyprflow::process::RealProcessInfo;
use hyprflow::restore::restore_session;
use hyprflow::session::{delete_session, list_sessions, load_session, save_session, session_exists};

#[derive(Parser)]
#[command(name = "hyprflow", version, about = "Save and restore Hyprland sessions")]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Show detailed output
    #[arg(short, long, global = true)]
    verbose: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Save current session
    Save {
        /// Session name (default: "latest")
        name: Option<String>,
        /// Overwrite without prompt
        #[arg(short, long)]
        force: bool,
    },
    /// Restore a saved session
    Restore {
        /// Session name (default: "latest")
        name: Option<String>,
        /// Preview without executing
        #[arg(short, long)]
        dry_run: bool,
    },
    /// List saved sessions
    List,
    /// Delete a saved session
    Delete {
        /// Session name to delete
        name: String,
    },
    /// Show config info
    Config,
}

fn main() {
    let cli = Cli::parse();
    let config = load_config();
    let sessions_dir = sessions_dir();

    match cli.command {
        Commands::Save { name, force } => {
            let name = name.unwrap_or_else(|| config.general.default_session.clone());

            if !force && session_exists(&name, &sessions_dir) && name != "latest" {
                eprintln!(
                    "Session '{}' already exists. Use --force to overwrite.",
                    name
                );
                std::process::exit(1);
            }

            let hyprctl = RealHyprctl;
            let process_info = RealProcessInfo;

            match capture_session(&name, &hyprctl, &process_info, &config) {
                Ok(session) => {
                    let client_count = session.clients.len();
                    match save_session(&session, &sessions_dir) {
                        Ok(()) => {
                            println!("Saved session '{}' ({} windows)", name, client_count);
                            if cli.verbose {
                                for c in &session.clients {
                                    println!("  ws={} {} — {}", c.workspace, c.class, c.title);
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!("Error saving session: {}", e);
                            std::process::exit(1);
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Error capturing session: {}", e);
                    std::process::exit(1);
                }
            }
        }

        Commands::Restore { name, dry_run } => {
            let name = name.unwrap_or_else(|| config.general.default_session.clone());

            match load_session(&name, &sessions_dir) {
                Ok(session) => {
                    let hyprctl = RealHyprctl;
                    match restore_session(&session, &hyprctl, &config, dry_run, cli.verbose) {
                        Ok(report) => {
                            if dry_run {
                                println!("Dry run for session '{}':", name);
                            } else {
                                println!("Restored session '{}':", name);
                            }
                            println!(
                                "  {} restored, {} skipped, {} failed",
                                report.restored, report.skipped, report.failed
                            );
                            for detail in &report.details {
                                println!("  {}", detail);
                            }
                        }
                        Err(e) => {
                            eprintln!("Error restoring session: {}", e);
                            std::process::exit(1);
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Error: {}", e);
                    std::process::exit(1);
                }
            }
        }

        Commands::List => match list_sessions(&sessions_dir) {
            Ok(sessions) => {
                if sessions.is_empty() {
                    println!("No saved sessions.");
                } else {
                    println!("Saved sessions:");
                    for s in sessions {
                        println!(
                            "  {} — {} windows ({})",
                            s.name,
                            s.client_count,
                            s.created_at.format("%Y-%m-%d %H:%M")
                        );
                    }
                }
            }
            Err(e) => {
                eprintln!("Error listing sessions: {}", e);
                std::process::exit(1);
            }
        },

        Commands::Delete { name } => match delete_session(&name, &sessions_dir) {
            Ok(()) => println!("Deleted session '{}'", name),
            Err(e) => {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        },

        Commands::Config => {
            println!("Config path: {}", config_path().display());
            println!("Sessions dir: {}", sessions_dir.display());
            println!("Default session: {}", config.general.default_session);
            println!("Restore delay: {}ms", config.general.restore_delay_ms);
            println!(
                "Window detect timeout: {}ms",
                config.general.window_detect_timeout_ms
            );
            println!("Ignored classes: {:?}", config.filters.ignore_classes);
            if !config.apps.is_empty() {
                println!("App configs:");
                for (name, app) in &config.apps {
                    println!(
                        "  {}: binary={:?} capture_cwd={:?}",
                        name, app.binary, app.capture_cwd
                    );
                }
            }
        }
    }
}
