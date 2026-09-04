# Changelog

## 0.1.0

First release, forked from [hyprflow](https://github.com/isorensen/hyprflow)
0.2.1 and extended with the restore model of
[hypr-session-restore](https://github.com/UpayanChatterjee/hypr-session-restore)
and its [Omarchy fork](https://github.com/SotoAugusto/hypr-session-restore).

### Restore engine

- Windows are placed by Hyprland's own `exec_cmd` window rules as they map,
  replacing the launch-then-poll-then-move loop. No races, no stolen focus.
- Added a sweep pass so single-instance apps (browsers, Electron) that escape
  process attribution still land on their saved workspace.
- Restore refuses to run over a populated desktop unless forced.

### Capture

- Launch commands come from `/proc/<pid>/cmdline` instead of being guessed
  from the window class.
- Terminals reopen in their shell's working directory; a TUI running inside
  one is relaunched with it. foot, kitty, Alacritty, ghostty and Omarchy's
  terminal are recognised out of the box.
- Fixed: an argv that the process flattened into a single space-separated
  string (Chromium does this) was replayed as one unusable argument.

### Session model

- Workspaces are identified by dispatcher selector, so named workspaces and
  scratchpads survive; ids alone do not.
- Pinned, fullscreen and maximized states are captured and restored.
- Fixed: a window that must be floated before positioning is re-centred by
  the resize, so geometry is now applied size-first, position-last, and
  absolutely.
- An empty save never overwrites a populated session.

### Integration

- `uwsm-app` launching so restored apps get their own systemd scopes.
- `hyprwake install` wires Omarchy's post-boot and post-update hooks, or
  prints the `autostart.lua` snippet on plain Hyprland.
- `hyprwake watch` saves off the Hyprland event socket, debounced, instead of
  polling.
- `hyprwake doctor` reports what would be saved and restored right now.
- Brave-only profile support generalised to the Chromium family.
- Logging to `~/.local/state/hyprwake/hyprwake.log`.
