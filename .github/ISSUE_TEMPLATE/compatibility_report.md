---
name: Compatibility report
about: Report a Hyprwake reboot result on your desktop
title: "compatibility: "
labels: compatibility
---

## Result

Did the desktop restore correctly after a reboot? Describe what restored, what
did not, and whether windows landed on the expected workspaces and monitors.

## Environment

- Hyprwake version:
- Omarchy version and channel:
- Hyprland version:
- Architecture:
- Terminal(s):
- Browser(s):
- Monitor setup:

## Commands run

Run this and attach the resulting file:

```sh
hyprwake support-bundle --output hyprwake-support.json
```

The bundle is the preferred diagnostic: it excludes snapshots, logs,
configuration, command arguments, working directories, and window titles.
Do not attach raw snapshots, logs, or `hyprwake doctor --json` output without
reviewing it for private paths, session IDs, and other secrets.
