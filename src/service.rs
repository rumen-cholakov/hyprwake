//! The watcher as a systemd user service.
//!
//! Started from a boot hook with `setsid`, the watcher has exactly one life:
//! if it dies, nothing notices until the next reboot. As a user unit it is
//! restarted on failure, its output lands in the journal, and "is it running?"
//! has an answer that does not involve grepping the process table.

use std::path::{Path, PathBuf};

pub const UNIT: &str = "hyprwake-watch.service";

pub fn systemd_user_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("~/.config"))
        .join("systemd")
        .join("user")
}

pub fn unit_content(binary: &str) -> String {
    format!(
        "[Unit]\n\
         Description=hyprwake session watcher\n\
         Documentation=https://github.com/rumen-cholakov/hyprwake\n\
         # Bound to the graphical session: there is nothing to watch without\n\
         # a compositor, and the unit should go away with it.\n\
         PartOf=graphical-session.target\n\
         After=graphical-session.target\n\
         \n\
         [Service]\n\
         Type=simple\n\
         # --replace so starting the unit takes over from a watcher started\n\
         # by hand, rather than refusing and leaving the old binary running.\n\
         ExecStart={binary} watch --replace\n\
         Restart=always\n\
         RestartSec=5\n\
         \n\
         [Install]\n\
         WantedBy=graphical-session.target\n"
    )
}

fn binary() -> String {
    which::which("hyprwake")
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "hyprwake".to_string())
}

pub fn install(systemd_dir: &Path) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(systemd_dir)?;
    let path = systemd_dir.join(UNIT);
    std::fs::write(&path, unit_content(&binary()))?;
    Ok(path)
}

pub fn uninstall(systemd_dir: &Path) -> std::io::Result<bool> {
    let path = systemd_dir.join(UNIT);
    if !path.exists() {
        return Ok(false);
    }
    let _ = systemctl(&["disable", "--now", UNIT]);
    std::fs::remove_file(&path)?;
    let _ = systemctl(&["daemon-reload"]);
    Ok(true)
}

pub fn is_installed(systemd_dir: &Path) -> bool {
    systemd_dir.join(UNIT).exists()
}

/// Reload units and enable the watcher, reporting whether it came up.
pub fn enable() -> bool {
    let _ = systemctl(&["daemon-reload"]);
    systemctl(&["enable", "--now", UNIT])
}

pub fn is_active() -> bool {
    systemctl(&["is-active", "--quiet", UNIT])
}

pub fn is_enabled() -> bool {
    systemctl(&["is-enabled", "--quiet", UNIT])
}

fn systemctl(args: &[&str]) -> bool {
    let mut full = vec!["--user"];
    full.extend_from_slice(args);
    std::process::Command::new("systemctl")
        .args(&full)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn the_unit_restarts_the_watcher_and_follows_the_session() {
        let unit = unit_content("/usr/bin/hyprwake");
        assert!(unit.contains("ExecStart=/usr/bin/hyprwake watch --replace"));
        assert!(unit.contains("Restart=always"));
        assert!(unit.contains("PartOf=graphical-session.target"));
        assert!(unit.contains("WantedBy=graphical-session.target"));
    }

    #[test]
    fn the_unit_takes_over_rather_than_refusing() {
        // Without --replace, starting the unit after a hand-started watcher
        // would exit immediately and leave the old binary running.
        assert!(unit_content("hyprwake").contains("watch --replace"));
    }

    #[test]
    fn install_writes_the_unit_and_uninstall_removes_it() {
        let dir = TempDir::new().unwrap();
        assert!(!is_installed(dir.path()));
        let path = install(dir.path()).unwrap();
        assert!(path.exists());
        assert!(is_installed(dir.path()));
        assert!(uninstall(dir.path()).unwrap());
        assert!(!is_installed(dir.path()));
    }

    #[test]
    fn uninstalling_nothing_is_not_an_error() {
        let dir = TempDir::new().unwrap();
        assert!(!uninstall(dir.path()).unwrap());
    }
}
