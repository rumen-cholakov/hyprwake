# Roadmap

## End goal: the desktop never loses your place

hyprwake starts as "reopen my windows after a reboot". The end goal is
larger: a person's working desktop becomes **durable, editable and
portable**, like a document. It survives a reboot, a crash or a kernel
update. It can be inspected and changed, and it moves between machines.

By 1.x, hyprwake should be:

1. **The session layer for tiling Wayland compositors.** One core, with
   backends for Hyprland first, then niri and sway.
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

## Phase 1: placement you can trust (0.2–0.3)

*Goal: a restore that looks exactly like what you left, on any monitor
setup, verified by machines and not only by one desktop.*

- **A test harness with a real compositor.** Run Hyprland headless in CI
  (Aquamarine's headless backend), spawn real clients and assert on
  `hyprctl clients` after save → kill → restore.
  - This is the base for everything later. Today all 211 tests run against
    doubles, and the compatibility matrix is empty.
- **Recover the tiling order.** Rebuild each tiled workspace's split order
  from the saved rectangles, and replay it with `preselect` /
  `swapwindow` / `togglesplit`. This is the most-requested fix in the space.
- **`restore --layout-only`**: rearrange the windows already open without
  launching anything.
- **Monitor hotplug and display profiles**, driven by the watcher.
- **Launch commands that hold up.** Fall back to `.desktop` files; handle
  Flatpak, AppImage and Nix wrappers. `doctor` flags windows it cannot
  relaunch.
- **Shutdown and opt-out.** A final snapshot when the unit stops, and a
  one-time "skip the next restore".

**Exit criteria:**

- The headless suite covers dwindle, master, floating, special workspaces
  and two monitors.
- Ten or more confirmed rows in the compatibility matrix, across at least
  three terminals, two browsers and multi-monitor setups.

## Phase 2: everything inside the windows (0.4–0.5)

*Goal: what the windows contain comes back, not just the windows.*

- **Adapters instead of special cases.** Generalise `[tui.resume]`,
  terminal tables, browser kinds and `[apps]` overrides into one adapter
  model. An adapter does three things:
  - matches a process;
  - extracts state at save time (from fds, a database, the environment or
    a helper command);
  - renders a launch.
- **A community adapter registry.** Adapters are versioned TOML in a
  separate repository, shipped as data and not code. This is how coverage
  grows without the core growing. Seed it with:
  - tmux, zellij and WezTerm-mux reattach;
  - ssh and mosh reconnect (opt-in);
  - nvim with auto-session or persistence.nvim;
  - Jupyter, VS Code workspaces, Zed, Obsidian vaults;
  - more agent CLIs.
- **Confirm-before-run** for adapters with side effects, as zellij does.
- **Hooks** (`pre_*` hooks can veto), a restore reason
  (`boot|crash|manual|update`), and desktop notifications.
- **Graceful close before shutdown** for apps that persist state on a clean
  exit (browsers, editors). Snapshot first, then close.

**Exit criteria:**

- Adapters can be added without a release.
- The top 30 applications in compatibility reports each have an adapter or
  a documented reason why none can exist.

## Phase 3: the session as a document (0.6–0.7)

*Goal: sessions are things you can read, edit, keep and reuse.*

- **A stable, versioned schema (v1)** with migrations, and a published JSON
  Schema, so other tools can read and write sessions.
- **Contexts.**
  - Sessions scoped to a set of workspaces.
  - `hyprwake open|close|switch <context>`.
  - Templates with variables (`{project}`, `{cwd}`), created from a live
    workspace.
  - Optional binding to a project directory or git branch, like
    auto-session.
- **History and time travel.**
  - Content-addressed snapshots: deduplicated and cheap, so the watcher can
    keep every change.
  - `hyprwake restore --at "yesterday 17:00"`.
  - Diffs between any two points in time.
  - Retention policies.
- **Tools around it.**
  - `hyprwake edit` (open in `$EDITOR`, then validate)
  - a picker (fuzzel, walker or rofi)
  - a Waybar module
  - a small TUI to browse history and contexts
- **Encryption at rest** (age), required before anything in Phase 5.

**Exit criteria:**

- A context saved on one machine opens on another with cwds remapped.
- The history of a working day costs less than 1 MB.

## Phase 4: beyond Hyprland, and into the compositor (0.8–0.9)

*Goal: one core, several compositors, and the capabilities that IPC alone
cannot give.*

- **Split the crate.**
  - `hyprwake-core`: schema, adapters, matching, safety rails, history.
  - One `Compositor` backend per compositor. The current `Hyprctl` trait is
    already that seam.
- **Backends.** niri (its IPC exposes columns, which suits it well) and
  sway / i3 (the layout tree is exposed, so exact tiling restore comes for
  free there). River or others only if users ask.
- **Upstream before plugins.** Propose the missing Hyprland dispatchers:
  - add a window to a group by address;
  - export and import the dwindle/master tree.

  Fall back to a small Hyprland plugin only for what upstream declines. Each
  of these removes a "What it cannot do" item from the README.
- **Naming.** Decide whether the multi-compositor project keeps the name,
  with `hyprwake` as the Hyprland distribution of a more neutral core.

**Exit criteria:**

- The same session file restores on Hyprland and niri, with
  compositor-specific placement dropped cleanly.
- At least one upstream dispatcher has been merged.

## Phase 5: the protocol era (1.0)

*Goal: apps restore their own state; hyprwake supplies the policy and the
memory.*

- **Help land `xdg-session-management` in Hyprland**, by contributing or
  co-authoring the implementation. The protocol was merged in
  wayland-protocols 1.48. KWin and Chromium/Qt 6.12 already speak it.
- **Become the session store behind it.** The compositor asks hyprwake for
  stored toplevel state, and hyprwake decides what to restore, where, and
  for which context.
- **A hybrid restore.** Apps that support the protocol restore exactly:
  geometry, and their own documents and tabs. Legacy apps keep argv relaunch
  and exec-rule placement. A single report covers both.
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

- **Keep up with Hyprland.** Track its IPC and config changes (the 0.55
  Lua config is the latest), and test against Hyprland git in a
  non-blocking CI job.
- **Safety and privacy.** Every new kind of state has to say:
  - where it is stored;
  - whether it can be executed;
  - whether it can leave the machine.

  `support-bundle` stays limited to diagnostic data by design.
- **Community.** Compatibility reports, an adapter registry with its own
  reviewers, and good-first-issue adapters. The bus factor is the biggest
  risk to this roadmap; adapters as data are the main way to lower it.

## Decisions ahead

| When | Decision |
|---|---|
| Phase 2 | Adapter format: declarative TOML only, or also sandboxed scripts? |
| Phase 3 | History storage: plain files with dedup, or an embedded store (SQLite)? |
| Phase 4 | Name and crate split; plugin vs upstream-only for Hyprland gaps |
| Phase 5 | How deep to go into the Hyprland protocol implementation |
| Phase 6 | Sync transport: bring-your-own folder, or a built-in peer link |

See [FEATURE_SUGGESTIONS.md](FEATURE_SUGGESTIONS.md) for the survey of
related projects behind Phases 1–3.
