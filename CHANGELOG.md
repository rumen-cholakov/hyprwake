# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).

## [Unreleased] (v0.2.0)

### Added

- Brave browser profile support: capture active profiles from `Local State`, restore one window per profile with `--profile-directory` flag
- Configurable workspace mapping per Brave profile via `profile_workspaces` in config.toml
- `hyprflow config` now displays detected Brave profiles with mapping status
- Count-based duplicate detection on restore: skips already-running windows, restores only missing count
- Autosave with rotation: `hyprflow autosave --now` captures and keeps last N sessions (configurable via `autosave_retain`)
- Systemd timer management: `hyprflow autosave --install` / `--uninstall` for automated periodic saves

### Fixed

- Filter plain shell hints (`/bin/zsh`, `bash`, `fish`, `sh`) from last-command detection — idle terminals no longer show noisy hints
- Monitor mapping now uses monitor ID instead of array index, fixing incorrect monitor assignment
- Race condition in Brave profile restore: snapshot addresses before spawning (not after)

---

## [0.1.0] - 2026-03-08

Initial release.

### Added

- `hyprflow save [name]` — save current Hyprland session (defaults to "latest")
- `hyprflow restore [name]` — restore saved session with sequential launch and exact pixel positioning
- `hyprflow list` — list all saved sessions with metadata
- `hyprflow delete <name>` — delete a named session
- `hyprflow config` — print current configuration
- `--dry-run` flag for restore preview without executing
- `--verbose` flag for detailed output during save and restore
- Kitty terminal support: restore working directory and show last command hint
- Configurable ignore list for transient windows (Waybar, Wofi, Mako, etc.)
- TOML configuration at `~/.config/hyprflow/config.toml`
- Sessions stored as JSON at `~/.local/share/hyprflow/sessions/`
- Trait-based abstraction (`HyprctlClient`, `ProcessInfoProvider`) for full unit testability
- AUR PKGBUILD for Arch Linux

### Fixed

- Skip kitten `__atexit__` helper process when capturing Kitty CWD to avoid reading the wrong working directory
- Derive `Default` for `Config` instead of manual implementation (clippy compliance)

