# TODO

## v0.2

- [ ] Fix: filter `/bin/zsh` and other bare shells from last-command hint (show nothing for idle shells)
- [ ] Fix: correct monitor index to name mapping when hyprctl monitor order differs from client monitor index
- [ ] Fix: warn user when restoring would create duplicate windows
- [ ] Autosave daemon via systemd timer
- [ ] Restore on login via `exec-once` in Hyprland config
- [ ] Brave browser profile support (`--profile-directory` flag based on saved profile)

## v0.3

- [ ] Custom hooks per app (pre-save, post-restore shell commands)
- [ ] Dwindle layout tree preservation (split ratios)
- [ ] Graceful fallback when monitor configuration changes between save and restore

## Future

- [ ] Plugin system for app-specific state capture
- [ ] Partial restore (single workspace)
- [ ] Layout message integration for split reconstruction
