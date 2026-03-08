# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build & Test Commands

```bash
cargo build --release              # Release binary (~2 MB)
cargo test                         # All 34 tests (unit + integration)
cargo test --lib                   # 29 unit tests only
cargo test --test cli_test         # 5 integration tests only
cargo test <test_name>             # Single test by name
cargo clippy --all-targets         # Lint (5 deprecation warnings are known/accepted)
cargo install --path .             # Install to ~/.cargo/bin/
```

## Architecture

Hyprflow is a Rust CLI that captures and restores Hyprland window sessions via `hyprctl` IPC.

### Core Modules

- **main.rs** — clap-derived CLI with subcommands: `save`, `restore`, `list`, `delete`, `config`
- **capture.rs** — Queries `hyprctl clients/monitors`, resolves CWD/last-command per window, builds `Session`
- **restore.rs** — Groups clients by workspace, spawns binaries sequentially, polls for new window address, positions via `hyprctl dispatch` (movetoworkspacesilent → resizewindowpixel exact → movewindowpixel exact)
- **session.rs** — `Session`/`SessionClient`/`LaunchInfo` data model, JSON file I/O under `$XDG_DATA_HOME/hyprflow/sessions/`
- **config.rs** — TOML config at `$XDG_CONFIG_HOME/hyprflow/config.toml`, per-app capture settings, ignore-class filters
- **hyprctl.rs** — `HyprctlClient` trait + `RealHyprctl` (shells out to `hyprctl`) + `MockHyprctl` for tests
- **process.rs** — `ProcessInfoProvider` trait + `RealProcessInfo` (reads `/proc`) + `MockProcessInfo` for tests

### Key Design Pattern

Trait-based dependency injection (`HyprctlClient`, `ProcessInfoProvider`) enables full unit testing without a running Hyprland session. Mocks record dispatches and return fixture data.

### Restore Flow Detail

Each window is restored sequentially: spawn → poll for new address (100ms intervals, configurable timeout) → position via hyprctl dispatch commands. The address-diff approach detects which new window belongs to which spawn.

### Test Fixtures

`tests/fixtures/` contains real `hyprctl` JSON output (3 windows on 2 monitors) used by both unit and integration tests.

## Known P0 Bugs

1. Hint shows `/bin/zsh` for idle shells instead of `None`
2. Monitor index→name mapping may be incorrect (hyprctl monitor order ≠ client monitor index)
3. No duplicate detection on restore
