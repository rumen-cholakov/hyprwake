# Hyprflow — Session Handoff

**Date:** 2026-03-08
**Branch:** main
**Status:** MVP v0.1 functionally complete, needs polish before release

## What Was Built

Hyprflow — a Rust CLI tool to save and restore Hyprland window sessions.

### Completed (Tasks 1-12)
- Project scaffold with Cargo, MIT license, .gitignore
- Session data model (serde JSON serialization)
- TOML config with XDG paths and sensible defaults
- HyprctlClient trait (abstracted for testability)
- ProcessInfo trait (/proc CWD + child process detection)
- Capture engine (hyprctl clients → session JSON, with filtering)
- Restore engine (sequential launch + address-diff polling + pixel positioning)
- Session file I/O (save/load/list/delete)
- CLI with clap (save/restore/list/delete/config + --dry-run/--verbose/--force)
- AUR PKGBUILD
- README, CHANGELOG, TODO

### Stats
- **34 tests** (29 unit + 5 integration), all passing
- **Clippy clean** (zero warnings)
- **Release binary:** 2.0 MB
- **10 commits** on main

## Key Files
```
src/main.rs        — CLI entry point (clap)
src/lib.rs         — module re-exports
src/capture.rs     — capture engine (largest file, ~200 lines)
src/restore.rs     — restore engine with sequential launch
src/session.rs     — data model + file I/O
src/config.rs      — TOML config parsing
src/hyprctl.rs     — HyprctlClient trait + RealHyprctl
src/process.rs     — ProcessInfo trait + RealProcessInfo
tests/cli_test.rs  — integration tests
tests/fixtures/    — sample JSON from real hyprctl output
docs/plans/        — design doc + implementation plan
```

## Live Testing Results (2026-03-08)

### Save: WORKING
- Captures 15 windows across 6 workspaces correctly
- Kitty CWD capture working (after fix for kitten __atexit__ process)
- Kitty hint capture working (detects `claude`, `python3`, etc.)
- Brave binary mapping via config.toml working

### Dry-run Restore: WORKING
- Generates correct hyprctl dispatch commands
- Correct workspace assignment, pixel positions, sizes

### Real Restore: NOT YET TESTED
- User did not test a real (non-dry-run) restore yet
- Recommended test: close 1 non-critical kitty, restore, verify position

## Known Issues / Bugs to Fix

### P0 (fix before release)
1. **Hint shows `/bin/zsh` for idle shells** — should be `None` when last command is a plain shell (zsh, bash, fish, sh). The filter exists for the shell cmdline itself but not for when `/bin/zsh` comes from grandchild detection.
2. **Monitor index mapping may be wrong** — ws=2 (on DP-5) was saved as `monitor: "DP-4"`. The `hyprctl monitors -j` array order doesn't always match the monitor index in `hyprctl clients -j`. Need to investigate and fix the mapping in `capture.rs:build_session_client()`.
3. **No duplicate detection on restore** — restoring when apps are already open creates duplicates. MVP limitation, but should at least warn.

### P1 (v0.2 scope)
4. **Brave profile support** — all Brave windows restore without profile. Profiles: Default→Credifit, Profile 1→LinkPJ, Profile 2→ABRH Bahia, Profile 3→admin-crm, Profile 4→iSorensen. Mapping in `~/.config/BraveSoftware/Brave-Browser/Local State`.
5. **Autosave daemon** — systemd timer for periodic saves
6. **Restore on boot** — `exec-once` integration

## Config File
Already deployed at `~/.config/hyprflow/config.toml`:
```toml
[general]
default_session = "latest"
restore_delay_ms = 800

[filters]
ignore_classes = ["waybar", "wofi", "mako", "polkit", "nm-applet", "xdg-desktop-portal"]

[apps.kitty]
binary = "kitty"
capture_cwd = true
capture_last_command = true
hint_template = "# Last: {last_command}"

[apps.brave-browser]
binary = "brave"
```

## Design Docs
- `docs/plans/2026-03-08-hyprflow-design.md` — approved design (architecture, data model, algorithms)
- `docs/plans/2026-03-08-hyprflow-implementation.md` — 12-task implementation plan

## Next Session Prompt

```
Continue hyprflow development. Read HANDOFF.md for context.

Priority:
1. Fix P0 bugs (hint /bin/zsh noise, monitor mapping, duplicate warning)
2. Test a real restore (non-dry-run) — close 1 kitty, restore, verify
3. After P0 fixes: start v0.2 (Brave profiles, autosave daemon)
```

## Environment
- Arch Linux, Hyprland 0.54.1, Rust 1.91.1
- Dual monitor: DP-4 (2560x1440@165Hz) + DP-5 (1920x1080@60Hz, rotated)
- 10 persistent workspaces (1,3-9 on DP-4; 2,10 on DP-5)
