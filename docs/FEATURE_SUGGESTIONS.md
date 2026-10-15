# Feature suggestions from related projects

A survey of session save/restore tools for Hyprland and other environments,
compared against hyprwake 0.1.3 (October 2026). Each suggestion names the
project it comes from and checks that hyprwake does not already have it.

## Status in 0.2.0

The survey was written before 0.2.0 shipped. Where each suggestion stands
now, and where it sits in the [roadmap](ROADMAP.md):

| # | Suggestion | Status |
|---|---|---|
| 1 | Recover the tiling order | **Done** (opt-in): `[restore] layout = "best-effort"` replays the saved split tree with `preselect` and an exact `splitratio`, for dwindle and master |
| 2 | `restore --layout-only` | Phase 1 |
| 3 | `.desktop` fallback, Flatpak, AppImage | Phase 1 |
| 4 | Monitor hotplug and display profiles | **Partly**: monitors matched by identity, and `[monitors] missing = "remap"` at restore time (opt-in). Re-applying on hotplug is Phase 1 |
| 5 | Skip the next restore; shutdown snapshot | Phase 1 |
| 6 | Hooks and notifications | Phase 2 |
| 7 | More terminals; terminals hosting sessions | **Partly**: a tmux recipe (`{pid}`, `keep_flags`; docs/PROVIDERS.md). WezTerm can be declared as a provider (`config --init` shows how) but is not built in. zellij and screen are Phase 2 |
| 8 | ssh and mosh reconnect | Phase 2 |
| 9 | Launch-command rewrite rules | Not yet. An external provider can take over a window class entirely |
| 10 | Confirm before re-running | Phase 2 |
| 11 | Snapshot history and recovery | **Partly**: the restored session is kept as `prerestore`, beside autosave rotation. The rest is Phase 3 |
| 12 | Session picker | Phase 3 |
| 13 | Waybar module | Phase 3 |
| 14 | Session scoped to a workspace | Phase 3 (contexts) |
| 15 | Track `xdg-session-management` | Phase 5. Hyprland 0.56.2 does not implement it |
| 16 | Graceful close before shutdown | Phase 2 |
| 17 | Background and tray apps | Not placed yet |
| 18 | Encryption at rest | Phase 3 |
| 19 | Reassemble window groups | **Done** (opt-in): `[restore] groups = "regroup"`, in saved tab order. Hyprland 0.56.2 ignores a window address in `window.move { into_group }` (measured), so members are launched into the focused group through `group:auto_group`. A by-address dispatcher is still worth proposing upstream (Phase 4) |

## What was reviewed

**Hyprland**

- [hyprflow](https://github.com/isorensen/hyprflow) and both [hypr-session-restore](https://github.com/UpayanChatterjee/hypr-session-restore) versions ([upstream](https://github.com/UpayanChatterjee/hypr-session-restore) and the [Omarchy fork](https://github.com/SotoAugusto/hypr-session-restore)), the projects hyprwake comes from
- [hyprsession](https://github.com/joshurtree/hyprsession), the most popular, which its author has stopped developing
- [hyprdrover](https://github.com/S-Sigdel/hyprdrover)
- [saveland](https://github.com/MAX-786/saveland)

**Other compositors**

- [i3-resurrect](https://github.com/JonnyHaystack/i3-resurrect), [i3-layout-manager](https://github.com/klaxalk/i3-layout-manager)
- [swayrst](https://github.com/Nama/swayrst), [sway-session](https://github.com/gumieri/sway-session)
- [nirinit](https://github.com/amaanq/nirinit) for niri

**Desktop environments**

- KDE Plasma 6.7
- GNOME's session restore work
- [Another Window Session Manager](https://github.com/nlpsuge/gnome-shell-extension-another-window-session-manager), a GNOME extension
- macOS Resume

**Terminals and editors**

- [tmux-resurrect](https://github.com/tmux-plugins/tmux-resurrect) and [tmux-continuum](https://github.com/tmux-plugins/tmux-continuum)
- [zellij](https://zellij.dev/documentation/session-resurrection)
- [kitty sessions](https://github.com/kovidgoyal/kitty/blob/master/docs/sessions.rst)
- [resurrect.wezterm](https://github.com/MLFlexer/resurrect.wezterm)
- [auto-session](https://github.com/rmagatti/auto-session) and [persistence.nvim](https://github.com/folke/persistence.nvim)

**Protocol**

- The [xdg-session-management](https://isaacfreund.com/docs/wayland/xdg-session-management-v1) Wayland protocol

### Where hyprwake already leads

Most of the issues people open against other Hyprland tools are already solved
in hyprwake:

| Request elsewhere | Where | hyprwake |
|---|---|---|
| Restore the active workspace and window | hypr-session-restore #2 | done (0.1.2) |
| Magic and special workspaces | hypr-session-restore #1, hyprsession #16 | done |
| Multi-monitor | hyprdrover #6, hypr-session-restore PR #5 | done (0.1.2) |
| Never overwrite a good snapshot with an empty one | hypr-session-restore PR #4 | done, plus a guard against collapsing snapshots |
| Ghostty support | hyprflow #3 | done |
| Don't relaunch windows that are already open | hyprdrover #5 | `--missing-only` |
| Flattened Chromium argv | hypr-session-restore PR #3 | fixed in 0.1.0 |

Resuming TUI sessions by id (Claude Code, codex) has no equivalent in any
window-manager tool reviewed. The closest analogue is the per-program
strategies in tmux-resurrect.

## Suggestions

These are roughly in priority order within each tier. Effort is a rough guess:
S is under a day, M is a few days, L is a week or more.

### Tier 1: high demand, fits the current design

#### 1. Recover the tiling order after spawning (M–L)

**Most requested**, and the README admits the gap.

- **Demand.** "Tiled windows land in random places" is the top complaint across tools: [hyprsession #7](https://github.com/joshurtree/hyprsession/issues/7), [hyprdrover #4](https://github.com/S-Sigdel/hyprdrover/issues/4).
- **The split tree is not exposed.** Hyprland's IPC still doesn't give it out, but each tiled window's `at` and `size` are already saved.
- **Proposed pass.** After the sweep, compare each tiled workspace's live geometry with the saved geometry. Correct it with `swapwindow` / `movewindow`, plus `togglesplit` when the orientation is wrong.
- **Dwindle sequencing (cheaper option).** Spawn a workspace's tiled windows one at a time, in an order rebuilt from the saved rectangles: the largest split first. Then set `layoutmsg preselect` before each spawn. The split direction can be inferred from which axis the two halves share.
- **Scope.** Even a "best effort, dwindle only" version would end the most common complaint.

#### 2. Adopt windows that are already open: `restore --layout-only` (S–M)

- **Sources.** hyprsession's `--adjust-clients-only`, i3-resurrect `--layout-only`, swayrst.
- **Behaviour.** Move live windows onto their saved workspaces, monitors and geometry, and launch nothing.
- **Mostly built already.** `sweep`, `pick_match` and `fix_calls` already do this for unattributed windows; this mode runs only that pass, over every window.
- **Uses.** It fixes a desktop that drifted. It is also the building block for #4.

#### 3. Fall back to `.desktop` files, and support Flatpak and AppImage launches (M)

- **Sources.** hyprsession, nirinit, GNOME's design.
- **The gap.** hyprwake has no `.desktop` handling at all (`grep -r desktop src` is empty), and the README lists D-Bus-activated apps as a known failure.
- **Fallback.** When `/proc` gives a useless argv (a D-Bus activator, a `bwrap` / Flatpak helper, an AppImage mount under `/tmp/.mount_*`, a Nix wrapper), resolve the window class against `.desktop` files. Use `StartupWMClass`, then the file name, then `Exec`, in `$XDG_DATA_DIRS`, the Flatpak exports and `~/.local/share/applications`.
- **Flatpak.** Detect `/.flatpak-info` in the process's root and relaunch with `flatpak run <app-id>`.
- **AppImage.** Use `$APPIMAGE` from the process's environment, not the temporary mount path.
- **Doctor.** `hyprwake doctor` reports which saved windows have no launch command it can trust.

#### 4. Monitor hotplug and display profiles (M)

- **Source.** swayrst (triggered by kanshi or shikane), plus hyprwake's own "docked vs on the road" case.
- **Behaviour.** The watcher already reads the event socket. On `monitoradded` / `monitorremoved`, it re-applies the workspace-to-monitor map from #2's adopt pass. It could also pick a named session tied to the monitor set (`[profiles] "DP-1+eDP-1" = "desk"`).
- **Restores the right layout.** Today a workspace whose monitor disappeared is left where it is, and nothing puts it back when the monitor returns.

#### 5. Skip the next restore once, and a pre-shutdown snapshot (S)

- **Kill switch.** Some way to skip one restore. hypr-session-restore uses a kill-switch file; macOS uses a one-time opt-out on the restart dialog. Examples: `hyprwake restore --skip-next`, or a `~/.local/state/hyprwake/skip-next` file the boot hook consumes and deletes. It is useful when a session is known to be bad, such as before a demo or after a crash loop.
- **Shutdown snapshot.** Add `ExecStop=hyprwake save --exact` to the systemd unit, or a sibling oneshot ordered before `graphical-session.target` stops. That gives every distro the guaranteed final snapshot that Omarchy only gets on its update path. It must run before any window closes, so it needs care with the collapsing-session guard. Run it as an exact save keyed off the stop signal, not a watcher save.

#### 6. Save and restore hooks, and desktop notifications (S)

- **Sources.** tmux-resurrect, auto-session, resurrect.wezterm.
- **Config.** `[hooks] pre_save`, `post_save`, `pre_restore`, `post_restore`. These are commands run with environment variables such as `HYPRWAKE_SESSION`, `HYPRWAKE_REASON=boot|manual|watch|update` (from GNOME's restore reason) and the report counts.
- **Veto.** A `pre_*` hook exiting non-zero cancels an *automatic* action (auto-session).
- **Notification.** An optional `notify-send` summary after a boot restore, such as "Restored 14 windows; 2 never appeared", would end the "did it work?" question without opening the log.

### Tier 2: clear value, moderate demand

#### 7. More terminals, and terminals that host sessions (S–M)

- **WezTerm.** Missing from `default_terminals`. Use `wezterm start --cwd`.
- **Konsole and xfce4-terminal.** Possible additions.
- **tmux, zellij or a WezTerm mux in the foreground.** Today it would replay `tmux` as a TUI and get a fresh server. A resume rule should reattach instead: `tmux attach -t <session>`, `zellij attach <name>`. The session name can be read from the client's argv or environment, or `TMUX` / `ZELLIJ_SESSION_NAME` in `/proc/<pid>/environ`.
- **Composes with existing tools.** tmux-resurrect / continuum or zellij resurrection then restore the inside. The two layers fit together with no extra work.

#### 8. Reconnect terminals running ssh or mosh (M)

- **Sources.** kitty's ssh kitten, tmux-resurrect's mosh strategy.
- **Behaviour.** A terminal whose foreground process is `ssh host` or `mosh host` comes back reconnecting. It is already allowlisted as a "TUI" if added, but `ssh` with a remote `cd` is better.
- **Opt-in.** This should be opt-in: replaying a network command at boot has side effects.

#### 9. Rules to rewrite launch commands (S)

- **Source.** tmux-resurrect's `@resurrect-processes` syntax: `~match->replacement`, `*` meaning "keep the original arguments".
- **What exists.** `[apps.X]` today can swap the binary and strip arguments.
- **Generalised version.**

  ```toml
  [[rewrite]]
  match = "~node .*/electron"
  launch = "obsidian {args}"
  ```

  This covers wrapper processes, updater re-execs (hypr-session-restore PR #3: Discord's updater) and Nix store paths without code changes.

#### 10. Confirm before re-running commands (S)

- **Source.** zellij shows resurrected commands behind a "Press ENTER to run" prompt.
- **Behaviour.** Add a per-program option `confirm = true`. The terminal opens with the command pre-typed or behind a prompt instead of executing. This suits `lazydocker`, `k9s` and anything with side effects.
- **Default.** Off for today's read-only TUI list.

#### 11. Snapshot history and recovery (S)

- **Sources.** The GNOME extension (a backup on overwrite, trash on delete); auto-session (purge orphaned sessions).
- **What exists.** Autosave rotation already exists, but `save` over `latest` and `delete` are destructive.
- **Proposals.**
  - Keep `latest.prev`.
  - Make `delete` move to a trash folder with `--purge`.
  - Add `hyprwake restore --previous`.
  - `hyprwake list` flags snapshots whose cwds no longer exist.

#### 12. Session picker and quick switch (S)

- **Sources.** resurrect.wezterm `fuzzy_load`, kitty `goto_session -1`.
- **`hyprwake pick`.** Lists sessions with their age and window counts through `$HYPRWAKE_MENU`, which defaults to `walker --dmenu`, `fuzzel --dmenu` or `rofi -dmenu`, whichever is installed. It then restores with `--missing-only`. Omarchy users would bind it to a key.
- **`hyprwake switch NAME`.** Optional. Closes the current named layout and opens another, as kitty's sessions do.

#### 13. A Waybar module (S)

- **Source.** tmux-continuum's status indicator.
- **Behaviour.** `hyprwake status --waybar` prints Waybar JSON: the time since the last good save, the watcher's health, and a warning class when the default snapshot is stale or the watcher is down. `status` already computes all of this.

#### 14. Scope a session to a workspace (M)

- **Source.** i3-resurrect, where profiles cover one workspace and sessions cover all of them.
- **Behaviour.** `hyprwake save dev --workspace 3`, then `hyprwake restore dev --workspace 5`. This turns named sessions into reusable project templates ("my coding layout"), not just whole-desktop snapshots.
- **Related.** Sessions keyed by project or git branch (auto-session).

### Tier 3: larger bets and future work

#### 15. Track xdg-session-management (L, blocked upstream)

- **Status.**
  - The protocol was merged in wayland-protocols 1.48 (April 2026).
  - KWin 6.7 implements it, and Chromium and Qt 6.12 clients support it.
  - Hyprland has no implementation or tracking issue yet.
- **When Hyprland lands it.** hyprwake should hand geometry to apps that opt in, and keep argv relaunch plus exec-rule placement for the rest.
- **Small step now.** Open or follow an upstream Hyprland issue, and have `doctor` report whether the compositor advertises the global.

#### 16. Graceful close before shutdown (M)

- **Sources.** The GNOME extension; the reason macOS Resume works.
- **Behaviour.** Send `closewindow` to browsers and editors before shutdown, so they write their own session (tabs, unsaved buffers) instead of being killed. Pair it with #5's final snapshot: save first, then close.
- **Opt-in.** Per class.

#### 17. Background and tray apps (M)

- **Demand.** [hyprsession #17](https://github.com/joshurtree/hyprsession/issues/17).
- **Behaviour.** Record allowlisted processes with no window, such as Spotify minimised to the tray or a Syncthing GUI. Relaunch them without placing anything.
- **Prior art.** `no_spawn` is the inverse of this.

#### 18. Encrypt snapshots at rest (M)

- **Source.** resurrect.wezterm (age, rage, GPG).
- **The case for it.** The README already treats snapshots as private (command lines, titles, cwds). With optional `age` encryption to a key on disk, a snapshot synced or backed up with the home directory carries no plaintext.
- **Caveat.** The value is lower than in wezterm's case, because hyprwake does not store scrollback.

#### 19. Reassemble window groups (S once unblocked)

- **Demand.** [hyprsession #10](https://github.com/joshurtree/hyprsession/issues/10).
- **Status.** The data is captured already. Watch Hyprland for a dispatcher that adds a window to a group by address. Until then a best-effort try could work: focus the first member, `togglegroup`, then `moveintogroup` for each neighbour. It would steal focus, so it should be opt-in.

## Not recommended

- **Scrollback capture** (zellij, wezterm). hyprwake does not own the terminal, so this would mean per-terminal scraping. tmux and zellij users get it from #7 for free.
- **Diff-based placement after each launch** (hyprflow's model). hyprwake's exec-rule placement replaced it on purpose.
- **Restoring every process** (`:all:` in tmux-resurrect). Even tmux documents this mode as dangerous; keep the allowlist model.
