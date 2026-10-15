# Roadmap

Where hyprwake goes after 0.2.0, and why in this order. Reliability is the
product: every item has to make a reboot come back more exactly, more often,
or say more precisely why it did not.

Hyprland-only stays the rule. Depth on one compositor is the whole advantage.

## 0.2.x — finish what 0.2.0 started

**Run the gate on real hardware, then promote the defaults.** Cases 6
(monitor identity, which needs real EDIDs) and 7 (a real rotated or scaled
panel) are the last two. When they pass, and a few real reboots fill
`docs/COMPATIBILITY.md`, the opt-in behaviours become defaults one at a time:
`[monitors] missing = "remap"`, `[restore] layout = "best-effort"`,
`[restore] groups = "regroup"`. Each was validated in nested compositors; none
has been through a real reboot.

**Groups and layouts around windows hyprwake does not launch.** Both rebuilds
skip a workspace holding a window its program reopens on its own — a
browser's second window, an Electron app's — because it arrives when it
arrives. Hyprland 0.56.2 can still put such a window into a group: focus it
and `window.move { into_group = <direction> }` (measured; it ignores a window
address, so focus it is). The direction has to come from where it landed,
and the result has to be checked, since `into_group` takes whatever lies that
way. The same arrival can be slotted into a rebuilt tree by `window.swap`.

**Session managers.** tmux has a validated recipe in 0.2.0 (`{pid}` and
`keep_flags`, docs/PROVIDERS.md); it becomes a default once real reboots
confirm it. zellij and screen need the same treatment — finding a client's
session — and neither is installed on the validation machine yet.

**Validate `restore_last_session`.** The Chromium flag is shipped off and
unvalidated. Its four questions — cold start, browser already running,
after an unclean exit, several profiles — can each be answered in a nest with
a throwaway `--user-data-dir`.

**A standing regression nest.** `bench/hyprwake-regress.sh`, proposed to
agent-stack, runs the round trips that found tonight's bugs (tab order,
`node-MainThread`, `--missing-only`) in a minute. In the nightly, a behaviour
change shows up the morning after, not after a reboot.

## 0.3 — the session, not just the windows

**A timeline, and `undo`.** Every save is a point in time; autosaves already
keep several. `hyprwake undo` — reopen what was closed in the last few
minutes — needs one thing the snapshots do not have: what was *closed*, not
only what is open. That is a capture change (diffing successive snapshots,
recording departures) with its own design, which is why it is not in 0.2.

**Editor sessions.** nvim, helix and emacs come back at the right directory
but not the right buffers. Their own session files (`Session.vim`,
desktop.el) make a third resume mechanism beside `fd_glob` and
`id_command`: find the session file the editor would write, and reopen with
it.

**Say what happened, where it is seen.** A restore at boot reports to a log
nobody reads. A single desktop notification — "12 windows back, 1 never
appeared, workspace 3 not rebuilt: confidence 0.4" — would make every
failure visible the moment it matters.

**The native protocol, when it exists.** Hyprland does not implement
`xdg-session-management` yet. When it does, applications that support it
restore their own window state, and hyprwake becomes the orchestrator for
those: a provider kind that asks the app to restore instead of replaying its
command line. The provider model was shaped to take one.

## Later, and only after reliability is demonstrably excellent

A Waybar module or small GUI for choosing and diffing snapshots. Not before
the compatibility matrix says restores are dependable on hardware people
actually use.

## Not planned

- Other compositors. tileroot covers three by replaying raw geometry, which
  is exactly the trade hyprwake avoids.
- Memory images (CRIU and the like). Programs restore their own state better
  than a process image can, and survive upgrades that an image does not.
