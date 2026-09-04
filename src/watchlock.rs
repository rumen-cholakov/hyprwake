//! Single-instance guard for the watcher.
//!
//! The watcher is started from a boot hook, and again by hand or by tooling
//! after an upgrade. Without a guard those pile up, each saving the same
//! session. A pid file makes "make sure it is running" a safe thing to say
//! twice.

use std::path::{Path, PathBuf};

#[derive(Debug)]
pub enum LockError {
    /// A live watcher already holds the lock.
    AlreadyRunning(i32),
    Io(std::io::Error),
}

impl std::fmt::Display for LockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LockError::AlreadyRunning(pid) => write!(f, "already watching (pid {pid})"),
            LockError::Io(e) => write!(f, "{e}"),
        }
    }
}

pub fn lock_path(state_dir: &Path) -> PathBuf {
    state_dir.join("watch.pid")
}

/// Owns the pid file and removes it on the way out.
#[derive(Debug)]
pub struct WatchLock {
    path: PathBuf,
}

impl Drop for WatchLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Claim the watcher lock.
///
/// A stale pid file — from a watcher that was killed, or whose pid has since
/// been reused by something else — is taken over rather than treated as a
/// live instance.
pub fn acquire(
    state_dir: &Path,
    replace: bool,
    is_live: &dyn Fn(i32) -> bool,
) -> Result<WatchLock, LockError> {
    std::fs::create_dir_all(state_dir).map_err(LockError::Io)?;
    let path = lock_path(state_dir);

    if let Some(existing) = read_pid(&path) {
        if is_live(existing) {
            if !replace {
                return Err(LockError::AlreadyRunning(existing));
            }
            terminate(existing);
        }
    }

    std::fs::write(&path, std::process::id().to_string()).map_err(LockError::Io)?;
    Ok(WatchLock { path })
}

fn read_pid(path: &Path) -> Option<i32> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

/// Whether `pid` is a running hyprwake, rather than a recycled pid.
pub fn is_live_hyprwake(pid: i32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/comm"))
        .map(|c| c.trim() == "hyprwake")
        .unwrap_or(false)
}

fn terminate(pid: i32) {
    let _ = std::process::Command::new("kill")
        .arg(pid.to_string())
        .status();
    // Give it a moment to drop its own lock file before overwriting it.
    std::thread::sleep(std::time::Duration::from_millis(200));
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn nothing_is_live(_: i32) -> bool {
        false
    }
    fn everything_is_live(_: i32) -> bool {
        true
    }

    #[test]
    fn the_first_watcher_takes_the_lock() {
        let dir = TempDir::new().unwrap();
        let lock = acquire(dir.path(), false, &nothing_is_live).unwrap();
        assert!(lock_path(dir.path()).exists());
        drop(lock);
        assert!(
            !lock_path(dir.path()).exists(),
            "the lock is released on exit"
        );
    }

    #[test]
    fn a_second_watcher_is_refused() {
        let dir = TempDir::new().unwrap();
        let _first = acquire(dir.path(), false, &everything_is_live).unwrap();
        match acquire(dir.path(), false, &everything_is_live) {
            Err(LockError::AlreadyRunning(pid)) => {
                assert_eq!(pid, std::process::id() as i32)
            }
            other => panic!("expected AlreadyRunning, got {other:?}"),
        }
    }

    #[test]
    fn a_stale_lock_is_taken_over() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path()).unwrap();
        std::fs::write(lock_path(dir.path()), "999999").unwrap();
        // The recorded process is gone, so the lock is free.
        assert!(acquire(dir.path(), false, &nothing_is_live).is_ok());
    }

    #[test]
    fn a_lock_held_by_an_unrelated_process_is_not_honoured() {
        let dir = TempDir::new().unwrap();
        std::fs::write(lock_path(dir.path()), "1").unwrap();
        // pid 1 is alive but is not a watcher; the check is by program name.
        assert!(acquire(dir.path(), false, &|pid| is_live_hyprwake(pid)).is_ok());
    }

    #[test]
    fn a_garbage_lock_file_does_not_block_startup() {
        let dir = TempDir::new().unwrap();
        std::fs::write(lock_path(dir.path()), "not-a-pid").unwrap();
        assert!(acquire(dir.path(), false, &everything_is_live).is_ok());
    }

    #[test]
    fn this_process_is_not_mistaken_for_a_watcher() {
        // The test binary is not called hyprwake.
        assert!(!is_live_hyprwake(std::process::id() as i32));
        assert!(!is_live_hyprwake(999_999));
    }
}
