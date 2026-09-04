//! Keeping the saved session fresh: a systemd timer, a polling daemon, and
//! an event-driven watcher.
//!
//! The watcher is the one to prefer. Hyprland publishes layout changes on its
//! event socket, so a save can follow the last change by a couple of seconds
//! instead of arriving up to a minute late — while staying idle when nothing
//! moves.

use crate::config::Config;
use crate::logging::log;
use crate::save::{perform_save, SaveOutcome};
use crate::session::{autosave_name_now, rotate_autosaves};
use std::io::{BufRead, BufReader};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const SERVICE_NAME: &str = "hyprwake-autosave.service";
const TIMER_NAME: &str = "hyprwake-autosave.timer";

// ── Event filtering and debouncing (pure, unit-tested) ─────────────────────

/// Hyprland events that change the window layout. Title and focus events are
/// deliberately absent: they fire constantly and change nothing worth saving.
const LAYOUT_EVENTS: &[&str] = &[
    "openwindow",
    "closewindow",
    "movewindow",
    "movewindowv2",
    "changefloatingmode",
    "fullscreen",
    "pin",
    "movewindowtoworkspace",
    "movewindowtoworkspacev2",
];

pub fn is_layout_event(line: &str) -> bool {
    let name = line.split_once(">>").map(|(n, _)| n).unwrap_or(line);
    LAYOUT_EVENTS.contains(&name)
}

/// Collapses a burst of events into a single save once things go quiet.
pub struct Debouncer {
    quiet_period: Duration,
    pending_since: Option<Instant>,
}

impl Debouncer {
    pub fn new(quiet_period: Duration) -> Self {
        Self {
            quiet_period,
            pending_since: None,
        }
    }

    pub fn on_event(&mut self, now: Instant) {
        self.pending_since = Some(now);
    }

    pub fn is_pending(&self) -> bool {
        self.pending_since.is_some()
    }

    /// True once the quiet period has elapsed since the last event; the
    /// pending state is consumed.
    pub fn should_fire(&mut self, now: Instant) -> bool {
        match self.pending_since {
            Some(last) if now.duration_since(last) >= self.quiet_period => {
                self.pending_since = None;
                true
            }
            _ => false,
        }
    }
}

// ── Run loops ──────────────────────────────────────────────────────────────

fn event_socket_path() -> Option<PathBuf> {
    let runtime = std::env::var("XDG_RUNTIME_DIR").ok()?;
    let signature = std::env::var("HYPRLAND_INSTANCE_SIGNATURE").ok()?;
    Some(
        PathBuf::from(runtime)
            .join("hypr")
            .join(signature)
            .join(".socket2.sock"),
    )
}

/// Save whenever the layout settles. Runs until the compositor goes away.
///
/// Holds the watcher lock for its lifetime, so starting a watcher when one is
/// already running is refused rather than doubled up. `replace` stops the
/// running one first, which is what an upgrade wants.
pub fn watch(
    name: &str,
    sessions_dir: &Path,
    config: &Config,
    replace: bool,
) -> std::io::Result<()> {
    let _lock = crate::watchlock::acquire(
        &crate::logging::state_dir(),
        replace,
        &crate::watchlock::is_live_hyprwake,
    )
    .map_err(|e| match e {
        crate::watchlock::LockError::Io(e) => e,
        // AddrInUse lets the caller tell "one is already running" -- which is
        // success for anything making sure the watcher is up -- apart from a
        // real failure to start.
        other => std::io::Error::new(std::io::ErrorKind::AddrInUse, other.to_string()),
    })?;

    let path = event_socket_path().ok_or_else(|| {
        std::io::Error::other("HYPRLAND_INSTANCE_SIGNATURE is not set; is Hyprland running?")
    })?;
    let stream = UnixStream::connect(&path)?;
    // The read timeout is what lets a quiet socket still tick the debouncer.
    stream.set_read_timeout(Some(Duration::from_millis(500)))?;
    log(format!("watch: connected to {}", path.display()));
    // Announced only once the lock is held and the socket is open, so the
    // message is never a claim about a watcher that did not start.
    println!("Watching for layout changes; saving '{name}' when things settle.");

    let mut debouncer = Debouncer::new(Duration::from_millis(config.general.debounce_ms));
    let mut reader = BufReader::new(stream);
    let mut line = String::new();

    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => {
                log("watch: event socket closed, saving once more and exiting");
                save_now(name, sessions_dir, config);
                return Ok(());
            }
            Ok(_) => {
                if is_layout_event(line.trim_end()) {
                    debouncer.on_event(Instant::now());
                }
            }
            Err(e) if is_timeout(&e) => {}
            Err(e) => return Err(e),
        }
        if debouncer.should_fire(Instant::now()) {
            save_now(name, sessions_dir, config);
        }
    }
}

fn is_timeout(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
    )
}

/// Save on a fixed interval. Simpler than [`watch`] and independent of the
/// event socket, at the cost of a stale window between ticks.
pub fn poll_daemon(name: &str, sessions_dir: &Path, config: &Config) -> ! {
    let interval = Duration::from_secs(config.general.save_interval_secs.max(1));
    log(format!("daemon: saving '{name}' every {interval:?}"));
    loop {
        save_now(name, sessions_dir, config);
        std::thread::sleep(interval);
    }
}

fn save_now(name: &str, sessions_dir: &Path, config: &Config) {
    let hyprctl = crate::hyprctl::RealHyprctl;
    let process = crate::process::RealProcessInfo;
    match perform_save(name, sessions_dir, config, &hyprctl, &process) {
        Ok(SaveOutcome::Saved(n)) => log(format!("autosave: {n} windows")),
        Ok(SaveOutcome::RefusedEmpty { kept }) => {
            log(format!("autosave: kept previous session ({kept} windows)"))
        }
        Err(e) => log(format!("autosave failed: {e}")),
    }
}

/// One timestamped snapshot plus rotation, for the systemd timer.
pub fn run_once(sessions_dir: &Path, config: &Config) -> Result<(usize, usize), String> {
    let hyprctl = crate::hyprctl::RealHyprctl;
    let process = crate::process::RealProcessInfo;
    let name = autosave_name_now();
    let outcome =
        perform_save(&name, sessions_dir, config, &hyprctl, &process).map_err(|e| e.to_string())?;
    let saved = match outcome {
        SaveOutcome::Saved(n) => n,
        SaveOutcome::RefusedEmpty { kept } => kept,
    };
    let pruned = rotate_autosaves(sessions_dir, config.general.autosave_retain)
        .map_err(|e| e.to_string())?;
    Ok((saved, pruned))
}

// ── systemd timer ──────────────────────────────────────────────────────────

pub fn systemd_user_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("~/.config"))
        .join("systemd")
        .join("user")
}

fn binary_path() -> String {
    which::which("hyprwake")
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "hyprwake".to_string())
}

fn service_content() -> String {
    format!(
        "[Unit]\n\
         Description=hyprwake session autosave\n\
         \n\
         [Service]\n\
         Type=oneshot\n\
         ExecStart={} autosave --now\n",
        binary_path()
    )
}

fn timer_content() -> String {
    "[Unit]\n\
     Description=hyprwake session autosave timer\n\
     \n\
     [Timer]\n\
     OnUnitActiveSec=10min\n\
     OnBootSec=1min\n\
     \n\
     [Install]\n\
     WantedBy=timers.target\n"
        .to_string()
}

pub fn install(systemd_dir: &Path) -> std::io::Result<(PathBuf, PathBuf)> {
    std::fs::create_dir_all(systemd_dir)?;
    if which::which("hyprwake").is_err() {
        eprintln!(
            "hyprwake: not on PATH; edit {}/{SERVICE_NAME} to use an absolute path.",
            systemd_dir.display()
        );
    }
    let service_path = systemd_dir.join(SERVICE_NAME);
    let timer_path = systemd_dir.join(TIMER_NAME);
    std::fs::write(&service_path, service_content())?;
    std::fs::write(&timer_path, timer_content())?;
    Ok((service_path, timer_path))
}

pub fn uninstall(systemd_dir: &Path) -> std::io::Result<()> {
    let _ = std::process::Command::new("systemctl")
        .args(["--user", "disable", "--now", TIMER_NAME])
        .output();
    for name in [TIMER_NAME, SERVICE_NAME] {
        let path = systemd_dir.join(name);
        if path.exists() {
            std::fs::remove_file(path)?;
        }
    }
    Ok(())
}

pub fn is_installed(systemd_dir: &Path) -> bool {
    systemd_dir.join(TIMER_NAME).exists()
}

fn systemctl_ok(args: &[&str]) -> bool {
    std::process::Command::new("systemctl")
        .args(args)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

pub fn is_active() -> bool {
    systemctl_ok(&["--user", "is-active", "--quiet", TIMER_NAME])
}

pub fn is_enabled() -> bool {
    systemctl_ok(&["--user", "is-enabled", "--quiet", TIMER_NAME])
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn layout_events_are_recognised() {
        assert!(is_layout_event("openwindow>>0x1,2,foot,title"));
        assert!(is_layout_event("closewindow>>0x1"));
        assert!(is_layout_event("movewindowv2>>0x1,3,3"));
        assert!(is_layout_event("changefloatingmode>>0x1,1"));
    }

    #[test]
    fn noisy_events_are_ignored() {
        // These fire on every keystroke in a shell; saving on them would
        // rewrite the session file continuously.
        assert!(!is_layout_event("windowtitle>>0x1"));
        assert!(!is_layout_event("windowtitlev2>>0x1,some title"));
        assert!(!is_layout_event("activewindow>>foot,title"));
        assert!(!is_layout_event("activewindowv2>>0x1"));
        assert!(!is_layout_event("mouseenter>>"));
    }

    #[test]
    fn a_prefix_alone_is_not_an_event_name() {
        assert!(!is_layout_event("openwindowextra>>0x1"));
    }

    #[test]
    fn debouncer_waits_for_quiet() {
        let mut d = Debouncer::new(Duration::from_millis(100));
        let t0 = Instant::now();
        d.on_event(t0);
        assert!(!d.should_fire(t0 + Duration::from_millis(50)));
        assert!(d.should_fire(t0 + Duration::from_millis(100)));
    }

    #[test]
    fn a_burst_of_events_produces_one_save() {
        let mut d = Debouncer::new(Duration::from_millis(100));
        let t0 = Instant::now();
        for offset in [0, 40, 80] {
            d.on_event(t0 + Duration::from_millis(offset));
            assert!(!d.should_fire(t0 + Duration::from_millis(offset + 10)));
        }
        assert!(d.should_fire(t0 + Duration::from_millis(200)));
        assert!(
            !d.should_fire(t0 + Duration::from_millis(400)),
            "firing must consume the pending state"
        );
    }

    #[test]
    fn nothing_fires_without_events() {
        let mut d = Debouncer::new(Duration::from_millis(10));
        assert!(!d.is_pending());
        assert!(!d.should_fire(Instant::now()));
    }

    #[test]
    fn timer_units_are_written_and_removed() {
        let dir = TempDir::new().unwrap();
        assert!(!is_installed(dir.path()));
        let (service, timer) = install(dir.path()).unwrap();
        assert!(service.exists() && timer.exists());
        assert!(is_installed(dir.path()));

        let unit = std::fs::read_to_string(&service).unwrap();
        assert!(unit.contains("autosave --now"));
        assert!(std::fs::read_to_string(&timer)
            .unwrap()
            .contains("OnUnitActiveSec=10min"));

        uninstall(dir.path()).unwrap();
        assert!(!is_installed(dir.path()));
        assert!(!service.exists());
    }

    #[test]
    fn uninstalling_when_nothing_is_installed_is_fine() {
        let dir = TempDir::new().unwrap();
        assert!(uninstall(dir.path()).is_ok());
    }
}
