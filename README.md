# hyprflow

Save and restore [Hyprland](https://hyprland.org) window sessions.

When you reboot or after a power loss, hyprflow restores your applications to their correct workspaces and positions.

## Features

- **Save** current session — captures all windows, positions, workspaces, and monitor layout
- **Restore** saved session — relaunches apps and positions them precisely
- **Kitty terminal support** — restores working directory + shows hint of last command
- **Smart filtering** — ignores transient windows (Waybar, Wofi, popups)
- **Dry run** — preview restore without executing
- **Brave profile support** — restores each profile to its configured workspace
- **Autosave** — periodic saves with automatic rotation via systemd timer
- **Configurable** — TOML config with per-app settings

## Installation

### From source

```bash
cargo install --path .
```

### Arch Linux (AUR)

```bash
# Using your preferred AUR helper
yay -S hyprflow
```

## Usage

```bash
# Save current session
hyprflow save              # saves as "latest"
hyprflow save work         # saves as "work"

# Restore a session
hyprflow restore           # restores "latest"
hyprflow restore work      # restores "work"
hyprflow restore --dry-run # preview without executing
hyprflow restore --max-age 24h  # skip if session older than 24h
hyprflow restore --on-login     # print exec-once line for hyprland.conf

# Manage sessions
hyprflow list              # list all saved sessions
hyprflow delete work       # delete a session

# Show config
hyprflow config

# Autosave
hyprflow autosave              # check timer status
hyprflow autosave --now        # save now + rotate old
hyprflow autosave --install    # set up systemd timer
hyprflow autosave --uninstall  # remove systemd timer
```

## Configuration

Config file: `~/.config/hyprflow/config.toml`

```toml
[general]
default_session = "latest"
restore_delay_ms = 500
window_detect_timeout_ms = 5000
autosave_retain = 5

[filters]
ignore_classes = ["waybar", "wofi", "mako", "polkit", "nm-applet", "xdg-desktop-portal"]

[apps.kitty]
binary = "kitty"
capture_cwd = true
capture_last_command = true
hint_template = "# Last: {last_command}\n# Dir: {cwd}"

[apps.brave-browser]
binary = "brave"
```

### Brave Profile Support

Hyprflow captures and restores Brave browser profiles individually. Since Brave
runs all windows in a single process, profiles are detected from Brave's
`Local State` file rather than from window processes.

Only profiles listed in `profile_workspaces` are captured and restored. Use
`hyprflow config` to see detected profiles and their mapping status.

```toml
[apps.brave-browser]
binary = "brave"
default_workspace = 1
profile_workspaces = { "Default" = 1, "Profile 1" = 6, "Profile 2" = 7 }
```

On restore, one Brave window is launched per mapped profile and moved to its
configured workspace. Profiles not in `profile_workspaces` are skipped.

### Autosave

Hyprflow can automatically save sessions at regular intervals using a systemd
timer. Autosave sessions are named with timestamps (`autosave-20260309T143000`)
and automatically rotated, keeping only the last N saves.

```bash
# Check autosave status
hyprflow autosave

# Run autosave manually (capture + rotate)
hyprflow autosave --now

# Install systemd timer (saves every 10 minutes)
hyprflow autosave --install

# Remove systemd timer
hyprflow autosave --uninstall
```

Configure retention in `config.toml`:

```toml
[general]
autosave_retain = 5   # keep last 5 autosave sessions (default)
```

### Restore on Login

To automatically restore your session when Hyprland starts:

```bash
hyprflow restore --on-login
```

This prints an `exec-once` line to add to `~/.config/hypr/hyprland.conf`:

```
exec-once = hyprflow restore --max-age 24h
```

The `--max-age` flag prevents restoring stale sessions. Accepted formats:
`30m` (minutes), `24h` (hours), `7d` (days).

### Sessions storage

Sessions are stored as JSON files in `~/.local/share/hyprflow/sessions/`.

## How it works

**Save:** Captures window state via `hyprctl clients -j`, reads terminal CWD from `/proc`, and serializes to JSON.

**Restore:** Launches apps sequentially, polls for new windows via address diff, then positions each window using `hyprctl dispatch` with exact pixel coordinates.

## Requirements

- Hyprland 0.54+
- Linux (uses `/proc` for terminal CWD detection)
- Rust toolchain (for building from source)

## Contributing

Issues and pull requests are welcome. Please open an issue before submitting a large change so we can discuss the approach.

## License

MIT — see [LICENSE](LICENSE)
