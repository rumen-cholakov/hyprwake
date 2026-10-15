# Roadmap

Where hyprwake goes after 0.2.0, and why in this order. Reliability is the
product: every item has to make a reboot come back more exactly, more often,
or say more precisely why it did not.

## End goal: the desktop never loses your place

hyprwake starts as "reopen my windows after a reboot". The end goal is
larger: a person's working desktop becomes **durable, editable and
portable**, like a document. It survives a reboot, a crash or a kernel
update. It can be inspected and changed, and it moves between machines.

By 1.x, hyprwake should be:

1. **The session layer for tiling Wayland compositors.** One core, with
   Hyprland first. Other compositors come later, and only if Phase 4's gate
   is met.
2. **The reference session store for `xdg-session-management`.** Apps that
   speak the protocol restore their own window state. hyprwake keeps argv
   relaunch for everything else, and owns the policy: when to restore, what,
   and where.
3. **A home for contexts.** A "context" is a named set of windows, terminals,
   TUI sessions and browser profiles: "client X", "thesis", "on-call". You
   can open, close, switch and template contexts, and take one from your
   laptop to your desktop.

These principles do not change at any stage:

- never steal focus;
- never destroy a good snapshot;
- private by default;
- every automatic action can be dry-run, explained and undone.

**Hyprland only, until Phase 4.** Depth on one compositor is what makes
restores exact. A second backend is worth considering only once restores are
dependable on Hyprland itself, which is what Phase 1's exit criteria measure.
Until then, nothing that would serve another compositor is allowed to cost
Hyprland anything.

## Where 0.2.0 stands

Several items in the survey behind this roadmap
([FEATURE_SUGGESTIONS.md](FEATURE_SUGGESTIONS.md), written against 0.1.3)
shipped in 0.2.0. Behaviours marked opt-in are off by default until real
reboots confirm them.

- **Tiling layouts.** Each save records every workspace's split tree, with a
  confidence. `[restore] layout = "best-effort"` rebuilds the tree split by
  split, for dwindle and master (opt-in).
- **Window groups.** `[restore] groups = "regroup"` rebuilds tabbed groups in
  their saved tab order (opt-in). Hyprland 0.56.2 cannot add a window to a
  group by address: `window.move { into_group }` ignores it (measured). So
  members are launched into a focused group through `group:auto_group`.
- **Monitor topology.** Monitors are matched by identity, and
  `[monitors] missing = "remap"` moves a missing monitor's workspaces to the
  most similar attached one at restore time (opt-in).
- **Providers.** Terminals, browsers, apps and external helpers are handled
  through one model: match a window, capture its state, render its launch.
  This is the seed of Phase 2's adapters.
- **A versioned snapshot schema.** Snapshots are schema v2, older ones are
  migrated on load, and a newer one is refused by name (docs/SCHEMA.md).
- **Smaller pieces:**
  - a tmux recipe (`{pid}`, `keep_flags`);
  - TUIs run by an interpreter, recognised by their script's name;
  - the restored session kept as `prerestore`;
  - a watcher that notices resizes.
- **Testing.** The suite has 364 tests, all against doubles. Nested Hyprland
  sessions (agent-stack's harness) run real round trips, and the release
  gate passes 10 of its 12 cases on one machine. The compatibility matrix
  has no rows from real reboots yet.

## Phase 1: placement you can trust (0.2.x–0.3)

*Goal: a restore that looks exactly like what you left, on any monitor
setup, verified by machines and not only by one desktop.*

- **Real hardware, then defaults.**
  - Run release-gate cases 6 (monitor identity, which needs real EDIDs) and
    7 (a real rotated or scaled panel).
  - Fill `docs/COMPATIBILITY.md` from real reboots.
  - Then make the opt-ins defaults one at a time: `missing = "remap"`,
    `layout = "best-effort"`, `groups = "regroup"`.
- **A test harness with a real compositor in CI.** Nested sessions run on
  the development machine today, through agent-stack's harness, and
  `bench/hyprwake-regress.sh` covers the round trips that found 0.2.0's
  bugs. Next steps:
  - run Hyprland headless in CI (Aquamarine's headless backend), spawn real
    clients, and assert on `hyprctl clients` after save → kill → restore;
  - put the regression nest in the nightly, so a behaviour change shows up
    the next morning.
- **Layouts and groups around windows hyprwake does not launch.** Both
  rebuilds skip a workspace holding a window its program reopens on its own:
  a browser's second window, an Electron app's. Such a window can still be
  focused and put into a group with `window.move { into_group = <direction> }`,
  checking afterwards that it joined. It can also be slotted into a rebuilt
  tree with `window.swap`.
- **`restore --layout-only`.** Rearrange the windows already open, without
  launching anything. The sweep's matching and placement already do this for
  the windows it adopts; this mode runs only that pass, over every window.
- **Monitor hotplug and display profiles, driven by the watcher.** 0.2.0
  places workspaces at restore time. On `monitoradded` and `monitorremoved`,
  the watcher should re-apply that placement, and optionally pick a named
  session for the attached set of monitors.
- **Launch commands that hold up.**
  - Fall back to `.desktop` files when `/proc` gives a useless argv.
  - Handle Flatpak (`flatpak run <app-id>`), AppImage (`$APPIMAGE`, not the
    temporary mount) and Nix wrappers.
  - `doctor` flags windows it cannot relaunch.
- **Shutdown and opt-out.**
  - A final exact snapshot when the session stops, before any window
    closes.
  - A one-time "skip the next restore".

**Exit criteria:**

- The headless suite covers dwindle, master, floating, special workspaces
  and two monitors.
- Ten or more confirmed rows in the compatibility matrix, across at least
  three terminals, two browsers and multi-monitor setups.
- Remap, best-effort layouts and regrouping are defaults.

## Phase 2: everything inside the windows (0.4–0.5)

*Goal: what the windows contain comes back, not just the windows.*

- **Providers become adapters.** 0.2.0's providers already match a window,
  capture its state and render a launch. The special cases left outside them
  (`[tui.resume]`, `[apps]` overrides) move in, so there is one model. An
  adapter extracts state at save time from fds, a database, the environment
  or a helper command.
- **A community adapter registry.** Adapters are versioned TOML in a
  separate repository, shipped as data and not code. This is how coverage
  grows without the core growing. Seed it with:
  - session managers: zellij, screen, a WezTerm mux, and the tmux recipe
    made a default;
  - ssh and mosh reconnect (opt-in);
  - editors reopening their buffers through their own session files: nvim
    (`Session.vim`, auto-session, persistence.nvim), helix, emacs
    (desktop.el);
  - Jupyter, VS Code workspaces, Zed, Obsidian vaults;
  - more agent CLIs.
- **Browser profiles beyond the first.** `--restore-last-session` restores
  only the first profile a Chromium browser starts with (measured), and
  `doctor` names the others. Restoring them without asking the user to
  change a setting means starting the browser once per profile, or reopening
  tabs from its session files.
- **Confirm-before-run** for adapters with side effects, as zellij does.
- **Hooks, a restore reason and a notification.**
  - `pre_*` hooks can veto an automatic action.
  - Every restore has a reason: `boot|crash|manual|update`.
  - One desktop notification after a boot restore, such as "12 windows
    back, 1 never appeared, workspace 3 not rebuilt: confidence 0.4", makes
    a failure visible the moment it matters.
- **Graceful close before shutdown** for apps that persist state on a clean
  exit (browsers, editors). Snapshot first, then close.

**Exit criteria:**

- Adapters can be added without a release.
- The top 30 applications in compatibility reports each have an adapter or
  a documented reason why none can exist.

## Phase 3: the session as a document (0.6–0.7)

*Goal: sessions are things you can read, edit, keep and reuse.*

- **A stable schema.** v2 is versioned and migrated already. Declare it
  stable and publish a JSON Schema, so other tools can read and write
  sessions.
- **Contexts.**
  - Sessions scoped to a set of workspaces.
  - `hyprwake open|close|switch <context>`.
  - Templates with variables (`{project}`, `{cwd}`), created from a live
    workspace.
  - Optional binding to a project directory or git branch, like
    auto-session.
- **History, time travel and undo.**
  - Content-addressed snapshots: deduplicated and cheap, so the watcher can
    keep every change.
  - `hyprwake restore --at "yesterday 17:00"`.
  - `hyprwake undo` reopens what was closed in the last few minutes. That
    needs what the snapshots do not have yet: a record of what was *closed*,
    not only what is open.
  - Diffs between any two points in time.
  - Retention policies.
- **Tools around it.**
  - `hyprwake edit` (open in `$EDITOR`, then validate)
  - a picker (walker, fuzzel or rofi)
  - a Waybar module
  - a small TUI to browse history and contexts
- **Encryption at rest** (age), required before anything in Phase 6.

**Exit criteria:**

- A context saved on one machine opens on another with cwds remapped.
- The history of a working day costs less than 1 MB.

## Phase 4: beyond Hyprland, and into the compositor (0.8–0.9)

*Goal: one core, several compositors, and the capabilities that IPC alone
cannot give.*

**Gate:** this phase starts only when Phase 1's exit criteria hold on
Hyprland, and only if there is demand. Otherwise Hyprland-only continues,
and only the upstream work below goes ahead.

- **Upstream before plugins.** Propose the missing Hyprland dispatchers:
  - add a window to a group by address;
  - export and import the dwindle/master tree.

  Fall back to a small Hyprland plugin only for what upstream declines. Each
  of these removes a "What it cannot do" item from the README.
- **Split the crate.**
  - `hyprwake-core`: schema, adapters, matching, safety rails, history.
  - One `Compositor` backend per compositor. The current `Hyprctl` trait is
    already that seam.
- **Backends.** niri (its IPC exposes columns, which suits it well) and
  sway / i3 (the layout tree is exposed, so exact tiling restore comes for
  free there). River or others only if users ask.
- **Naming.** Decide whether the multi-compositor project keeps the name,
  with `hyprwake` as the Hyprland distribution of a more neutral core.

**Exit criteria:**

- The same session file restores on Hyprland and niri, with
  compositor-specific placement dropped cleanly.
- At least one upstream dispatcher has been merged.

## Phase 5: the protocol era (1.0)

*Goal: apps restore their own state; hyprwake supplies the policy and the
memory.*

`xdg-session-management` is in wayland-protocols' staging set (1.49 on the
validation machine). Hyprland 0.56.2 does not implement it. A small step now
is for `doctor` to report whether the compositor offers it.

- **Help land `xdg-session-management` in Hyprland**, by contributing or
  co-authoring the implementation.
- **Become the session store behind it.** The compositor asks hyprwake for
  stored toplevel state, and hyprwake decides what to restore, where, and
  for which context.
- **A hybrid restore.** Apps that support the protocol restore exactly:
  geometry, and their own documents and tabs. Legacy apps keep argv relaunch
  and exec-rule placement. A single report covers both. The provider model
  was shaped to take a provider kind that asks the app to restore.
- **1.0 means:**
  - the schema is stable;
  - the adapter API is stable;
  - a documented security model covers what is stored, what is executed and
    what is shared;
  - packages for Arch/Omarchy, Fedora, Nix and Debian are in place.

## Phase 6: continuity across machines (1.x and beyond)

*The ambitious part.*

- **Handoff.** `hyprwake handoff thesis --to desktop` closes the context
  here and opens it there. The pieces it relies on:
  - encrypted sync through a folder you already sync (Syncthing, git), or a
    direct peer link;
  - path remapping between home directories and mount points;
  - per-machine monitor profiles;
  - adapters that know how to move state, such as pushing a git branch or
    reattaching a remote tmux session.
- **Crash continuity.** After a compositor crash, restore within seconds
  from the watcher's last state, without a re-login. A `crash` restore
  reason triggers more aggressive matching of windows that survived.
- **Shared context templates.** A team publishes "the on-call layout" or
  "the repo dev layout" as a template anyone can open, built from Phase 3
  templates and Phase 2 adapters.
- **Measured fidelity.** `hyprwake verify` scores how closely a restore
  matched the snapshot (windows, placement, adapters). That score drives the
  compatibility matrix automatically, through opt-in sanitised reports built
  on `support-bundle`.

## Cross-cutting work that runs through every phase

- **Keep up with Hyprland.** Track its IPC and config changes: the Lua
  config manager arrived in 0.55, and 0.56.2 is the validation machine's.
  Test against Hyprland git in a non-blocking CI job.
- **Safety and privacy.** Every new kind of state has to say:
  - where it is stored;
  - whether it can be executed;
  - whether it can leave the machine.

  `support-bundle` stays limited to diagnostic data by design.
- **Community.** Compatibility reports, an adapter registry with its own
  reviewers, and good-first-issue adapters. The bus factor is the biggest
  risk to this roadmap; adapters as data are the main way to lower it.

## Not planned

- **Memory images** (CRIU and the like). Programs restore their own state
  better than a process image can, and survive upgrades that an image does
  not.
- **Scrollback capture.** hyprwake does not own the terminal, so this would
  mean scraping each terminal separately. tmux and zellij users get it from
  their session manager.
- **Diff-based placement after each launch.** hyprwake's exec-rule placement
  replaced it on purpose.
- **Restoring every process.** Even tmux documents that mode as dangerous;
  the allowlist model stays.

## Decisions ahead

| When | Decision |
|---|---|
| Phase 1 | When each opt-in becomes a default, and on what evidence |
| Phase 2 | Adapter format: declarative TOML only, or also sandboxed scripts? |
| Phase 3 | History storage: plain files with dedup, or an embedded store (SQLite)? |
| Phase 4 | Whether to go beyond Hyprland at all; then the name, the crate split, and plugin vs upstream-only for Hyprland gaps |
| Phase 5 | How deep to go into the Hyprland protocol implementation |
| Phase 6 | Sync transport: bring-your-own folder, or a built-in peer link |

See [FEATURE_SUGGESTIONS.md](FEATURE_SUGGESTIONS.md) for the survey of
related projects behind Phases 1–3.
