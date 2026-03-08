# hyprflow

Save and restore [Hyprland](https://hyprland.org) window sessions.

When you reboot or after a power loss, hyprflow restores your applications to their correct workspaces and positions.

## Features

- **Save** current session — captures all windows, positions, workspaces, and monitor layout
- **Restore** saved session — relaunches apps and positions them precisely
- **Kitty terminal support** — restores working directory + shows hint of last command
- **Smart filtering** — ignores transient windows (Waybar, Wofi, popups)
- **Dry run** — preview restore without executing
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

# Manage sessions
hyprflow list              # list all saved sessions
hyprflow delete work       # delete a session

# Show config
hyprflow config
```

## Configuration

Config file: `~/.config/hyprflow/config.toml`

```toml
[general]
default_session = "latest"
restore_delay_ms = 500
window_detect_timeout_ms = 5000

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

### Sessions storage

Sessions are stored as JSON files in `~/.local/share/hyprflow/sessions/`.

## How it works

**Save:** Captures window state via `hyprctl clients -j`, reads terminal CWD from `/proc`, and serializes to JSON.

**Restore:** Launches apps sequentially, polls for new windows via address diff, then positions each window using `hyprctl dispatch` with exact pixel coordinates.

## Requirements

- Hyprland 0.54+
- Linux (uses `/proc` for terminal CWD detection)

## License

MIT
