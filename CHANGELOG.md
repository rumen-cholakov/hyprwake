# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).

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

### Known Issues

- Idle shell windows may show `/bin/zsh` as the last command hint instead of nothing
- Monitor index to name mapping may be incorrect when monitor order differs between `hyprctl monitors` and `hyprctl clients`
- No duplicate detection on restore: running restore when apps are already open creates duplicates
