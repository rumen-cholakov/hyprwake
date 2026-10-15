# Changelog

## Unreleased

### Security

- **A directory name could be executed by the shipped codex resume rule.**
  The codex `id_command` spliced `{cwd_sql}` into the text of an `sh -c`
  script. `{cwd_sql}` doubles single quotes for SQL, but the query sat inside a
  shell *double-quoted* string, so a working directory named `$(…)`, `` `…` ``
  or `"; …; "` was run as shell on every save — by the watcher, unattended —
  whenever codex was open there. A subdirectory of a cloned repository can be
  named anything. The shipped rule now passes the home directory and the query
  as positional parameters, which the shell never parses. Affects 0.1.0
  through 0.1.3 — every release so far.
- **Configs copied from the README are protected too.** The README showed the
  same pattern to copy. `id_command` now refuses to substitute a directory name
  containing shell-active characters into script text — the argument after
  `sh -c`, `python -c`, `perl -e` and similar — and the rule falls back
  instead. Values in ordinary arguments are unaffected. If you copied the old
  codex example, replace it with the one now in the README.

### New

- **Snapshots say which format they are in.** Each snapshot now records a
  schema version and the hyprwake that wrote it. Older snapshots still load and
  restore unchanged — they are migrated in memory. A snapshot from a *newer*
  hyprwake is refused with a message naming its version; 0.1.3 would instead
  have restored it, silently dropping every field it did not understand.
  `hyprwake status` and `doctor` report the schema. Snapshots also record each
  monitor's make, model, serial, scale and position, so a display can later be
  recognised on a different port.
- **Providers.** Which mechanism captures and reopens a window class is now
  declared rather than implied by the order of an if-else chain. The
  mechanisms stay built in — `terminal`, `chromium`, `firefox`, `command`,
  and `external` below — and instances are declared as `[providers.<name>]`
  with a `kind` and the window `classes` they claim, including `prefix*`
  patterns. They add to the built-ins. A wrong `kind`, a missing required field, or an option on the
  wrong kind (`restore_last_session` on Firefox, a typo in `strip_args`) is a
  load error with its line number instead of a silent no-op. See
  [docs/PROVIDERS.md](docs/PROVIDERS.md).
- **Tiling layouts, recorded and optionally rebuilt.** Each save now records,
  per workspace, the split tree its tiled windows were in — read back from
  their geometry, since Hyprland does not expose it — with the layout kind
  (dwindle, or master and its side) and a confidence: 1.0 for a clean tiling,
  lowered by gaps that are not the configured ones, a window smaller than its
  tile (pseudotiled), or a split Hyprland would not keep. With
  `[restore] layout = "best-effort"`, a workspace read at or above
  `layout_min_confidence` (0.7) is rebuilt split by split: each window
  launched, focused, and split with `preselect` and `splitratio`, or for
  master, masters first with `addmaster`, the orientation and `mfact`. A
  workspace that cannot be — already populated, holding a window its program
  reopens itself, special, or under the bar — is launched in reading order,
  and a rebuild that goes wrong part way is abandoned without stopping the
  restore. `"strict"` rebuilds every recorded tree; `"off"` records nothing
  and waits for nothing. The default, `"reading-order"`, is the 0.1.x restore.
  Validated in a nested Hyprland: a four-window dwindle tree with three
  uneven ratios and a five-window tree came back within 2px across eight
  restores, a master layout exactly; a pseudotiled window scored 0.6 and its
  workspace fell back quietly. `doctor` reports the mode and warns when
  `dwindle:preserve_split` is off; `hyprwake diff` reports a workspace tiled
  differently from its snapshot.
- **External providers.** `kind = "external"` hands capture and relaunch of
  a window class to a helper program speaking JSON on stdin and stdout, under
  a contract versioned separately from the snapshot schema. The helper runs
  only if it is an executable you own that nobody else can write, in a
  directory nobody else can write; it gets three seconds; every argv token and
  workspace it returns is checked before use; and any failure falls back to
  replaying the window's own argv, with the reason in the log, the restore
  report and `doctor`. `restore --dry-run` shows each helper's reply verbatim.
  See [docs/PROVIDERS.md](docs/PROVIDERS.md).
- **`hyprwake providers`** lists every provider in effect and where it was
  declared; `hyprwake providers <class>` explains how one class resolves.
  Declaration problems — two providers claiming one class — are shown there
  and in `doctor`. Each saved window records which provider captured it.
- **`restore_last_session`** on a Chromium provider passes
  `--restore-last-session`, so each profile reopens its tabs whatever its own
  startup setting is. Off by default, and not yet validated across a running
  browser, an unclean exit or several profiles.
- **`hyprwake config --init`** writes the new form. Terminals already handled
  by a built-in provider are listed rather than repeated.
- **`hyprwake restore --dry-run --json`** prints the restore as a list of
  structured actions — launches, the sweep, workspace moves, focus — for
  scripts and bug reports. A dry run and a live restore are now two readings
  of one plan, rather than two code paths that each claimed to describe the
  same restore; a recorded fixture pins every dispatch a live restore makes.
- **Monitor topology.** `[monitors] missing = "remap"` sends a missing
  monitor's workspaces to the attached monitor most like it and scales its
  floating windows into it; `"collapse"` sends them to the focused monitor;
  `[monitors.remap]` names a replacement outright. Under either strategy a
  display is recognised by serial, then make and model, then connector, so the
  same monitor on another port keeps its workspaces and a different one on a
  familiar port does not inherit them. `restore --topology saved` refuses
  unless every saved monitor is attached, and rebuilds that layout. The
  default, `"preserve-numbers"`, is exactly the 0.1.x behaviour. The report
  says why any workspace went somewhere other than its own connector. Checked
  against recorded `hyprctl monitors -j` fixtures; not yet through the
  multi-monitor release gate, which is why none of it is on by default.
- **One way of telling windows apart, used everywhere.** The restore sweep,
  `--missing-only` and `hyprwake diff` now pair live windows with saved ones
  through the same ranking: class (or the class a window announced when it
  first mapped), then already being on the saved workspace, then a shared
  title, then saved order. Titles never decide whether a window matches —
  only which of several equally good candidates it is — which is what tells
  two windows of one VS Code process apart. `restore --verbose` says what
  decided each pairing, and the summary counts pairings that only saved order
  could decide. Snapshots record each window's executable and initial title
  for support reports; neither is used for matching, because within a class
  they are the same for every window.
- **`hyprwake diff` reports moved windows.** A saved window that is open on
  another workspace is listed as moved, as restore would treat it, instead of
  as one missing and one unexpected.
- **opencode conversations come back.** opencode is now reopened inside its
  terminal and resumed with `--session <id>`, looked up in its own database by
  directory. Subagent runs, which opencode records as child sessions in the
  same directory and which are usually the newest row, are never the one
  resumed. `--continue` is the fallback.

### Upgrading

Nothing to do. `[terminals]`, `[apps]` and `[browsers]` keep working exactly
as before — a test pins the argv they produce against 0.1.3 — and snapshots
from 0.1.x load and restore unchanged. If you copied the codex resume rule
from an earlier README, replace it with the current one (see Security).

### Fixes

- **`--missing-only` relaunched the wrong window.** It took the first saved
  window of a class to be the one already open: with a terminal open on
  workspace 2 out of three saved on 1, 2 and 3, it launched a second terminal
  onto workspace 2, none onto 1, and then reported one as never having
  appeared. The open window is now paired with the saved entry it is, the
  others are launched, and an open window is no longer waited for.
- **A terminal standing in a deleted directory is reopened somewhere real.**
  The kernel reports a removed working directory as `"<path> (deleted)"`, and
  hyprwake captured exactly that — a terminal restored into a directory that
  does not exist, suffix included. A build directory or a git worktree removed
  under an open shell was enough. Such a directory now counts as unknown and
  the terminal reopens where it was started, or at home.
- **`doctor` no longer calls the event socket reachable without trying it.**
  It reported "reachable; `watch` can run" whenever
  `HYPRLAND_INSTANCE_SIGNATURE` was set, so a stale signature, a compositor
  that had gone away or the wrong `XDG_RUNTIME_DIR` all passed. It now
  connects, and names the socket when that fails. Found by the agent-stack
  nested-compositor tests.
- **A Hyprland on a legacy `hyprland.conf` is named as the cause.** Every
  restore call is a Lua dispatcher, and the old config manager rejects them
  all as "Invalid dispatcher" — one baffling failure per window. The restore
  report now says once that the compositor is not running the Lua config
  manager, which is what needs fixing.
- **A nested Hyprland is never saved.** A nested compositor shows on the
  host as an `aquamarine` window (Hyprland's backend library); a watcher save
  while one was open would have relaunched a compositor inside the session at
  the next boot. `aquamarine` is now in the default `ignore_classes`.
- **Running the test suite no longer stops your watcher.** A service test
  installed a unit into a temporary directory and then uninstalled it — which
  ran `systemctl --user disable --now hyprwake-watch.service` against the real
  user systemd, stopping and disabling the live watcher on every `cargo test`.
  Every systemd call now goes through one module that records instead of
  running under test, and a test fails if any other module calls `systemctl`.
  Autosave's uninstall no longer disables a timer it did not install.
- **Tiled windows come back in the order they were saved.** Dwindle tiles
  windows in the order they *map*, and a restore launched a workspace's windows
  back to back — so a terminal launched second overtook a browser or an
  Electron app launched first, and every mixed pair came back mirrored. Found
  after a real reboot: VS Code and Chrome swapped sides with their terminals on
  every two-window workspace, while a pair of terminals, being equally fast,
  came back correct. A restore now waits for each tiled window to map before
  launching the next one onto the same workspace. Workspaces still restore in
  parallel with each other. `tiled_map_timeout_ms` (default 10000, 0 to turn
  the wait off) bounds the wait, so a program that never maps costs one timeout
  and the report says that pair's order is not guaranteed.
- **The session being restored is kept as `prerestore`.** `hyprwake watch`
  saves over `latest` seconds after the restored desktop settles, so a restore
  that came back wrong used to destroy the only record of what was right.
  `hyprwake restore prerestore` brings back what the last restore started from.

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
