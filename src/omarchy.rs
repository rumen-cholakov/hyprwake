//! Integration with Omarchy's hook system.
//!
//! Omarchy runs `omarchy-hook post-boot` a couple of seconds after Hyprland
//! starts and `post-update` when an update finishes. Dropping scripts into
//! those directories wires restore and autosave into the desktop without
//! touching the Hyprland config at all.

use std::path::{Path, PathBuf};

/// Scripts are named so they sort after Omarchy's own and are recognisable.
const RESTORE_HOOK: &str = "40-hyprwake-restore.hook";
const WATCH_HOOK: &str = "45-hyprwake-watch.hook";
const UPDATE_SAVE_HOOK: &str = "30-hyprwake-save.hook";

pub fn is_omarchy() -> bool {
    if let Ok(os_release) = std::fs::read_to_string("/etc/os-release") {
        if os_release.lines().any(|l| l.trim() == "ID=omarchy") {
            return true;
        }
    }
    hooks_dir().is_some_and(|d| d.exists())
}

pub fn hooks_dir() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("omarchy").join("hooks"))
}

fn binary() -> String {
    which::which("hyprwake")
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "hyprwake".to_string())
}

pub fn restore_hook_body(binary: &str, max_age: &str) -> String {
    format!(
        "#!/bin/bash\n\
         # Reopen the previous session shortly after the desktop is up.\n\
         # Detached: the sweep runs for a while and must not block the hook runner.\n\
         sleep 4\n\
         setsid {binary} restore --max-age {max_age} >/dev/null 2>&1 &\n"
    )
}

pub fn watch_hook_body(binary: &str) -> String {
    format!(
        "#!/bin/bash\n\
         # Save the session whenever the window layout settles, so any exit\n\
         # path — clean logout, crash, power loss — has a fresh snapshot.\n\
         setsid {binary} watch >/dev/null 2>&1 &\n"
    )
}

pub fn update_save_hook_body(binary: &str) -> String {
    format!(
        "#!/bin/bash\n\
         # Exact snapshot right after an update finishes, before the reboot\n\
         # prompt, independent of when the watcher last saved.\n\
         {binary} save\n"
    )
}

pub struct Installed {
    pub written: Vec<PathBuf>,
}

pub fn install(hooks: &Path, max_age: &str) -> std::io::Result<Installed> {
    let bin = binary();
    let files = [
        (
            "post-boot.d",
            RESTORE_HOOK,
            restore_hook_body(&bin, max_age),
        ),
        ("post-boot.d", WATCH_HOOK, watch_hook_body(&bin)),
        (
            "post-update.d",
            UPDATE_SAVE_HOOK,
            update_save_hook_body(&bin),
        ),
    ];

    let mut written = Vec::new();
    for (dir, name, body) in files {
        let target_dir = hooks.join(dir);
        std::fs::create_dir_all(&target_dir)?;
        let path = target_dir.join(name);
        std::fs::write(&path, body)?;
        set_executable(&path)?;
        written.push(path);
    }
    Ok(Installed { written })
}

pub fn uninstall(hooks: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut removed = Vec::new();
    for (dir, name) in [
        ("post-boot.d", RESTORE_HOOK),
        ("post-boot.d", WATCH_HOOK),
        ("post-update.d", UPDATE_SAVE_HOOK),
    ] {
        let path = hooks.join(dir).join(name);
        if path.exists() {
            std::fs::remove_file(&path)?;
            removed.push(path);
        }
    }
    Ok(removed)
}

pub fn is_installed(hooks: &Path) -> bool {
    hooks.join("post-boot.d").join(RESTORE_HOOK).exists()
}

fn set_executable(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)?.permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms)
}

/// What to add to a plain Hyprland setup that has no hook system.
pub fn autostart_snippet(max_age: &str) -> String {
    let bin = binary();
    format!(
        "-- ~/.config/hypr/autostart.lua\n\
         hl.on(\"hyprland.start\", function()\n\
         \x20 hl.exec_cmd(\"sleep 4 && {bin} restore --max-age {max_age}\")\n\
         \x20 hl.exec_cmd(\"{bin} watch\")\n\
         end)\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn install_writes_all_three_hooks_executable() {
        let dir = TempDir::new().unwrap();
        let installed = install(dir.path(), "7d").unwrap();
        assert_eq!(installed.written.len(), 3);
        assert!(is_installed(dir.path()));

        for path in &installed.written {
            assert!(path.exists());
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = std::fs::metadata(path).unwrap().permissions().mode();
                assert_eq!(mode & 0o111, 0o111, "hooks must be executable");
            }
        }
    }

    #[test]
    fn the_restore_hook_detaches_and_honours_max_age() {
        let body = restore_hook_body("hyprwake", "7d");
        assert!(body.contains("setsid hyprwake restore --max-age 7d"));
        assert!(
            body.trim_end().ends_with('&'),
            "must not block the hook runner"
        );
    }

    #[test]
    fn the_update_hook_saves_synchronously() {
        // It has to finish before the reboot prompt appears.
        let body = update_save_hook_body("hyprwake");
        assert!(body.contains("hyprwake save"));
        assert!(!body.contains("setsid"));
    }

    #[test]
    fn hooks_land_in_the_right_directories() {
        let dir = TempDir::new().unwrap();
        install(dir.path(), "24h").unwrap();
        assert!(dir.path().join("post-boot.d").join(RESTORE_HOOK).exists());
        assert!(dir.path().join("post-boot.d").join(WATCH_HOOK).exists());
        assert!(dir
            .path()
            .join("post-update.d")
            .join(UPDATE_SAVE_HOOK)
            .exists());
    }

    #[test]
    fn uninstall_removes_exactly_what_was_installed() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("post-boot.d")).unwrap();
        let foreign = dir.path().join("post-boot.d").join("99-mine.hook");
        std::fs::write(&foreign, "#!/bin/bash\n").unwrap();

        install(dir.path(), "24h").unwrap();
        let removed = uninstall(dir.path()).unwrap();
        assert_eq!(removed.len(), 3);
        assert!(!is_installed(dir.path()));
        assert!(foreign.exists(), "other people's hooks must be left alone");
    }

    #[test]
    fn uninstall_is_idempotent() {
        let dir = TempDir::new().unwrap();
        assert!(uninstall(dir.path()).unwrap().is_empty());
    }

    #[test]
    fn the_autostart_snippet_uses_the_lua_api() {
        let snippet = autostart_snippet("7d");
        assert!(snippet.contains("hl.on(\"hyprland.start\""));
        assert!(snippet.contains("restore --max-age 7d"));
        assert!(snippet.contains("watch"));
    }
}
