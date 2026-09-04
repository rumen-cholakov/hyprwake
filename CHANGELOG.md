# Changelog

## 0.1.3

### Diagnostics

- **`hyprwake status` answers the question you actually have in the morning**:
  will the desktop come back? It reports the default snapshot's window count
  and age, the watcher, the Omarchy hooks and the autosave timer. Unlike
  `doctor` it asks the compositor nothing, so it still answers from a TTY when
  the graphical session is the thing that failed.
- **`hyprwake diff` compares a saved session with the desktop in front of
  you** and names what is missing and what is unexpected — the quick way to
  see whether a restore finished, or whether a snapshot has drifted from the
  way you now work. The comparison is by application class and workspace, not
  by window identity, so it stays coarse on purpose.
- **`hyprwake doctor --json`** emits the same checks as a stable structured
  document, for scripts and for filling in a compatibility report.

### Reporting

- **`hyprwake support-bundle` writes a report that is safe to attach to an
  issue.** A restore that fails nearly always fails because of the
  environment, and the obvious evidence — a snapshot, a log — is exactly the
  material that carries project names, session identifiers and credentials
  passed on command lines. The bundle carries the version, the OS and
  architecture, and the `doctor` checks with home directories redacted. It
  carries nothing else, and that is a deliberate constraint on what may be
  added to it later.
- **Compatibility and enhancement issue templates**, and a
  [compatibility matrix](docs/COMPATIBILITY.md) that records setups confirmed
  by a real reboot rather than by a passing build. Hyprwake has been verified
  on one machine, with one display; the matrix says so, and separates what CI
  establishes from what only a desktop can.

### Documentation

- **Snapshots are documented as private desktop state.** They record window
  titles, command lines and working directories because a restore needs them.
  The README now says that plainly, says which directories hold it, and says
  which diagnostic output is safe to share unreviewed and which is not.
- **A first-reboot procedure for Omarchy** — save, dry-run, install, doctor,
  and only then hand a boot hook your desktop.
- The install example pins the current tag, and the advertised test count
  matches the suite again.

## 0.1.2

### Restore

- **Workspaces return to their monitor.** Every session already recorded which
  monitor each window was on and restore discarded it, so on more than one
  display roughly half a layout landed wrong. The monitor is decided by
  majority of the windows saved on a workspace, applied once the windows
  exist, and skipped for monitors that are no longer attached — a session
  saved docked has to restore on the road.
- **The view and the focus come back.** Each monitor returns to the workspace
  it was showing, and the window that had focus regains it, as the last step
  of a restore. Previously you landed wherever the final spawn happened to go.
- **`--missing-only` merges into a desktop already in use**, launching what is
  absent and leaving open windows where they are. Refusing a populated desktop
  remains the default; this is the answer that refusal was asking for.
- **Stray windows are matched on the class they first announced** as well as
  their current one, so an application that renames itself after mapping is
  paired instead of reported as never having appeared.
- **Window groups are recorded**, and a restore names the groups it could not
  reassemble. Hyprland 0.56 offers no way to add a window to an existing group
  by address, so the members come back on the right workspace, ungrouped,
  rather than silently losing the fact that they belonged together.

### Saving

- **A collapsing session is no longer recorded.** The empty-save guard caught
  a desktop with nothing left; this catches one that has lost most of its
  windows seconds after a full snapshot — what a logout, or a restore still in
  progress, looks like from the outside. A desktop emptied gradually is a real
  change and is still saved.
- **The watcher runs as a systemd user unit**, restarted if it dies and tied
  to the graphical session, instead of a detached process with one life.

### Applications

- **Firefox, Zen and LibreWolf profiles** are restored alongside the Chromium
  family: profiles read from `profiles.ini` and opened by name.

## 0.1.1

A verification release: no behaviour changes.

- The package claimed x86_64 and aarch64, but nothing had ever built for ARM.
  CI now runs the suite and a release build on a native ARM runner, so the
  claim is tested rather than assumed. This matters for packaging: the
  Omarchy repository builds ARM packages and runs the package's `check()`,
  where an architecture-dependent test would surface as a failed build.

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

### Session resumption

- A TUI that can reopen its own session is asked to. The session id is
  recovered from the program's open files via `/proc/<pid>/fd`, so several
  sessions of one program in the same directory each come back as themselves
  rather than all resuming whichever was touched last.
- A second strategy for programs that expose nothing per-process: a rule may
  name a command that prints the session id for a working directory. codex
  records the `cwd` of every thread in its state database, so the session
  belonging to a terminal can be looked up and resumed exactly rather than
  falling back to "the most recent one anywhere".
- Ships with rules for Claude Code and codex; other programs can be described
  in `[tui.resume.<program>]`. Ids are validated before they reach a command
  line, and lookups are bounded by a timeout so a save cannot stall.
- t3code needs nothing: it is a GUI application that restores its own state
  on relaunch, like a browser.

### Packaging and CI

- CI runs formatting, clippy with warnings denied, the full test suite
  (including the CLI integration tests, which the inherited workflow skipped)
  and a release build; a second job compiles and tests on Arch itself and
  validates the PKGBUILD with `makepkg --printsrcinfo` and `namcap`.
- The release workflow publishes a binary archive with checksums and a
  ready-to-submit Omarchy Package Repository directory, with the source
  checksum computed for it. The inherited AUR publishing job is gone.
- `PKGBUILD` builds for x86_64 and aarch64, runs the test suite as its
  `check()`, and declares sqlite and uwsm as optional dependencies for codex
  resume and scoped launching.
- `scripts/opr-bundle.sh` produces the same submission directory by hand.

### Development workflow

- The watcher is now single-instance, guarded by a pid file; `--replace`
  takes over from a running one.
- `scripts/install-dev-hooks.sh` installs a pre-commit hook running the two
  fast checks CI runs first (rustfmt, clippy) and a post-commit hook that
  rebuilds, reinstalls, refreshes the desktop wiring, restarts the watcher
  and re-snapshots after any commit that touches the program.

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
