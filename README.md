# hyprwake

[![CI](https://github.com/rumen-cholakov/hyprwake/actions/workflows/ci.yml/badge.svg)](https://github.com/rumen-cholakov/hyprwake/actions/workflows/ci.yml)
[![Latest release](https://img.shields.io/github/v/release/rumen-cholakov/hyprwake?display_name=tag&sort=semver)](https://github.com/rumen-cholakov/hyprwake/releases/latest)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Reopen your Hyprland session after a reboot: the same windows, on the same
workspaces, with each terminal back in the directory it was in and supported
TUIs resumed where they can be.

macOS calls this "reopen windows when logging back in". It is not a memory
image — those are bound to the kernel that wrote them and die at the next
kernel update. It is a record of what was open and how to open it again.

```
hyprwake save                 # snapshot now
hyprwake restore              # bring it back
hyprwake install              # do both automatically, forever
hyprwake doctor               # what would happen if you did
```

## What it restores

| | |
|---|---|
| Windows | relaunched from the argv recorded in `/proc`, not guessed from the window class |
| Workspaces | including named workspaces and scratchpads (`special:*`) |
| Terminals | reopened in the shell's working directory |
| TUIs | `nvim`, `yazi`, `btop`, `lazygit`… relaunched inside the terminal, in their own directory |
| Sessions | a program that can reopen its own session is asked to — Claude Code and codex come back on the same conversation, not a blank one |
| Floating windows | pixel-exact position and size |
| Tiling | opt-in: each workspace's dwindle or master split tree, read back from window geometry and rebuilt split by split when it was read cleanly; reading order otherwise |
| Window state | pinned, fullscreen, maximized |
| Window groups | opt-in: tabbed groups rebuilt as groups, tabs in their saved order; separate windows otherwise |
| Browser profiles | one window per profile, each on its own workspace — Chromium family via `Local State`, Firefox family via `profiles.ini` |
| Focus | never stolen — everything is placed silently, in the background; a layout rebuild focuses windows to split them, and hands focus back where it was saved |

## How it works

Windows are not launched and then chased around. Each program is started
through Hyprland's own `exec_cmd` dispatcher carrying window rules, so the
compositor places the window the instant it maps — right workspace, right
geometry, no polling, no race, no focus theft.

That covers every program the compositor can attribute to the process it
spawned. Single-instance programs — browsers, Electron apps, Obsidian — hand
the request to an already-running process instead, and their window arrives
unattributed. A sweep afterwards pairs those strays with their saved entry by
window class — preferring an entry already on the window's workspace, then one
whose title it shares, so two VS Code windows from one process each find
their own folder's workspace — and places them explicitly. `--verbose` says
what decided each pairing, and the report counts any that only saved order
could decide.

Saving runs off Hyprland's event socket. `hyprwake watch` saves a couple of
seconds after the layout stops changing, so a crash or a power cut costs you
nothing, while an idle desktop writes nothing at all.

## Install

```sh
cargo install --git https://github.com/rumen-cholakov/hyprwake --tag v0.1.3
```

Or take the binary from a [release](https://github.com/rumen-cholakov/hyprwake/releases)
and put it on your PATH, or build a package from the `PKGBUILD` in this repo:

```sh
makepkg -si
```

Then wire it into the desktop:

```sh
hyprwake install
```

That installs a systemd user unit for the watcher — bound to
`graphical-session.target`, `Restart=always`, so a watcher that dies comes
back instead of leaving you unprotected until the next reboot.

On Omarchy it also drops three hooks: restore and the watcher into
`post-boot.d`, and an exact snapshot into `post-update.d` so the
update-then-reboot path always has a fresh session. On plain Hyprland the same
command prints the `autostart.lua` snippet to paste instead.

Take a first snapshot so there is something to come back to:

```sh
hyprwake save
```

### First Omarchy reboot

Before trusting an automatic restore, check the plan it will use:

```sh
hyprwake save
hyprwake restore --dry-run
hyprwake install
hyprwake doctor
```

When `doctor` reports no failures, reboot. Remove the hooks and watcher again
at any time with `hyprwake uninstall`.

## Help test Hyprwake

If it works (or fails) on your desktop, [open a compatibility report](https://github.com/rumen-cholakov/hyprwake/issues/new?template=compatibility_report.md).
Include your Omarchy and Hyprland versions, terminal, browser, monitor setup,
and the result after a reboot. Please remove private paths, window titles, and
command-line arguments before posting logs or snapshots. Confirmed setups are
tracked in [the compatibility matrix](docs/COMPATIBILITY.md).

## Usage

```sh
hyprwake save [NAME]            # snapshot; NAME defaults to "latest"
hyprwake restore [NAME]         # reopen it
hyprwake restore --dry-run      # print every dispatch instead of running it
hyprwake restore --dry-run --json  # the same plan, as structured actions
hyprwake restore --max-age 7d   # skip a session older than this
hyprwake restore --force        # restore even though windows are open
hyprwake restore --missing-only # open only what is missing, leave the rest
hyprwake restore --topology saved  # only onto the monitors it was saved on
hyprwake diff                   # what is missing, unexpected or moved, by class
hyprwake list -v                # saved sessions, and what is in them
hyprwake delete NAME

hyprwake watch                  # save when the layout settles (event-driven)
hyprwake watch --replace        # take over from a watcher already running
hyprwake daemon                 # save on a timer instead
hyprwake autosave --install     # timestamped snapshots via a systemd timer
hyprwake autosave --now         # snapshot and rotate

hyprwake status                 # concise restore/watcher health summary
hyprwake doctor                 # check the whole pipeline
hyprwake doctor --json          # same checks, structured for local scripts
hyprwake providers [CLASS]      # which provider handles each window class
hyprwake support-bundle         # sanitized diagnostic report for an issue
hyprwake config --init          # write a config seeded from this machine
```

Named sessions are ordinary snapshots you can keep: `hyprwake save work`, then
`hyprwake restore work` whenever you want that layout back.

## Safety rails

Unattended saving is only safe with guards, and the ones here were learned the
hard way:

- **An empty save never overwrites a populated session.** Logout and reboot
  close every window before the session ends; a periodic save landing in that
  gap would otherwise destroy the snapshot exactly when it is needed.
- **Nor does a collapsing one.** Losing most of the windows seconds after a
  full snapshot is a session being torn down, not rearranged — the same gap,
  caught before it reaches zero. A desktop genuinely emptied over minutes is
  a real change and is recorded. `--force` overrides, and
  `save_drop_fraction = 0.0` turns it off.
- **Restore refuses a populated desktop.** More than a few windows open means
  this is not a fresh login, and restoring would duplicate everything.
  `--force` overrides it; `--missing-only` is usually what you actually
  wanted — it opens what is absent and leaves everything else alone.
- **Sessions age out.** The boot hook passes `--max-age`, so a machine that
  has been off for a week does not reopen last week's work.
- **Everything is logged** to `~/.local/state/hyprwake/hyprwake.log`, because
  a restore that runs from a boot hook has nowhere else to complain.

## Privacy

Snapshots in `~/.local/share/hyprwake/sessions/` record window titles (current
and initial), application command lines and executable paths, working
directories, workspace names, and browser profile names so they can be
restored. The log lives in
`~/.local/state/hyprwake/`. Treat both directories as private desktop state:
do not share a snapshot or log without reviewing it, and avoid placing secrets
in command-line arguments.

`hyprwake support-bundle` writes a new owner-only JSON report with platform
metadata and redacted `doctor` checks. It deliberately excludes snapshots,
logs, configuration, command arguments, working directories, and window
titles, so it is the preferred starting point for an issue report.

`hyprwake doctor --json` is intended for local scripts, not for sharing
unchanged: its checks can contain local paths. `hyprwake diff` pairs windows
the way restore does — titles included — but prints only counts by
application class and workspace: what is missing, what is unexpected, and
what is open on the wrong workspace.

## Configuration

Optional; `~/.config/hyprwake/config.toml`. `hyprwake config --init` writes one
describing what is actually installed here.

```toml
[general]
sweep_timeout_secs = 20      # how long to wait for late windows
debounce_ms = 3000           # quiet period before an event-driven save
recheck_secs = 300           # watch: re-capture when quiet, save if changed
                             # (resizing a tiled window emits no event)

[launch]
use_uwsm = true              # launch through uwsm-app (systemd scopes)

[filters]
ignore_classes = ["waybar", "wofi", "mako"]

# Providers decide how each window class is captured and reopened. The
# built-ins cover foot, the Omarchy terminal, kitty, Alacritty and Ghostty;
# anything else replays its own argv. These add to the built-ins.
# `hyprwake providers` lists what is in effect; docs/PROVIDERS.md has the rest.
[providers.wezterm]
kind = "terminal"
classes = ["org.wezfurlong.wezterm"]
binary = "wezterm"
cwd_flag = "--cwd"           # a trailing "=" joins flag and value
extra_args = []              # e.g. an --app-id that pins the window class

[tui]
programs = ["nvim", "yazi", "btop"]   # relaunched inside their terminal; a
                                      # Node or Python program by its script
                                      # name ("gemini", "aider")

# Programs that can reopen a specific session. Two strategies, tried in
# order; every rule below ships by default.
#
# fd_glob reads the id out of a path the running program holds open — exact,
# so several sessions in one directory each come back as themselves.
[tui.resume.claude]
fd_glob = "/tmp/claude-*/*/{id}/*"    # one segment carries {id}
args = ["--resume", "{id}"]
fallback = ["--continue"]             # when no id could be recovered
strip_flags = ["--resume", "-c", "--continue"]

# id_command suits programs that keep sessions in a database and expose
# nothing per-process. It runs as you, at save time, with a 3s timeout;
# {cwd}, {cwd_sql} (quotes doubled) and {home} are substituted, and the id
# is validated before it reaches a command line. Run the helper directly
# where you can:
[tui.resume.opencode]
id_command = ["sqlite3", "file:{home}/.local/share/opencode/opencode.db?mode=ro",
              "SELECT id FROM session WHERE directory = '{cwd_sql}' AND parent_id IS NULL ..."]
args = ["--session", "{id}"]
fallback = ["--continue"]

# When a shell is unavoidable, hand values to it as positional parameters —
# never write a placeholder inside the script text. A directory can be named
# `$(...)`, and a script would run it. hyprwake refuses to splice a directory
# name containing shell-active characters into a script, and falls back.
[tui.resume.codex]
id_command = ["sh", "-c", "db=$(ls -t \"$1\"/.codex/state_*.sqlite | head -1); exec sqlite3 \"file:$db?mode=ro\" \"$2\"",
              "sh", "{home}", "SELECT id FROM threads WHERE cwd = '{cwd_sql}' ..."]
args = ["resume", "{id}"]
fallback = ["resume", "--last"]
strip_flags = ["resume", "--last"]

[providers.spotify]
kind = "command"             # replay the window's own argv
classes = ["Spotify"]
spawn = false                # record the window, never launch it

[providers.chrome]
kind = "chromium"            # profiles from Local State
classes = ["google-chrome"]
binary = "google-chrome-stable"
local_state = "google-chrome/Local State"
profile_workspaces = { "Default" = "2", "Work" = "6" }

[providers.zen]
kind = "firefox"             # profiles from profiles.ini, by name
classes = ["zen", "zen-alpha"]
binary = "zen-browser"
profiles_ini = ".zen/profiles.ini"
profile_workspaces = { "default" = "2", "work" = "6" }
```

Monitors are matched by connector name unless you ask for more. A snapshot
also records each display's make, model and serial, so a laptop that docks
somewhere else can be told apart from one that docked here:

```toml
[monitors]
missing = "preserve-numbers" # default: a missing monitor's workspaces land
                             # wherever Hyprland puts them, as before 0.2.0
# missing = "remap"          # onto the attached monitor most like it, with
                             # floating windows scaled into it
# missing = "collapse"       # onto the focused monitor

[monitors.remap]             # explicit, whatever `missing` says
"DP-1" = "eDP-1"
```

With `remap` or `collapse`, a display is recognised by serial, then make and
model, then connector — so the same monitor on another port keeps its
workspaces, and a different monitor on a familiar port does not inherit them.

Tiling is recorded at every save: for each workspace, the split tree its
tiled windows were in, read back from where they sit, and how cleanly they
tiled. Rebuilding it is opt-in:

```toml
[restore]
layout = "reading-order"     # default: launch tiled windows in their saved
                             # reading order, each after the one before maps
# layout = "best-effort"     # rebuild each workspace's tree split by split
                             # where it was read with enough confidence
# layout = "strict"          # rebuild every recorded tree, however unsure
# layout = "off"             # record nothing, wait for nothing
layout_min_confidence = 0.7
```

A rebuild launches a workspace's windows one at a time, focusing each to
split it with `preselect` and setting the ratio with `splitratio`, so it is
slower than reading order and moves focus while it runs. It needs Hyprland's
dwindle or master layout. With `dwindle:preserve_split` off, Hyprland derives
every split's direction from its shape, so a rebuilt tree reflows when a
window is resized — `doctor` warns about it. A workspace is launched in
reading order instead, and `--verbose` says why, when its tree was read below
the confidence bar (a pseudotiled window, gaps that are not the configured
ones), when it already has windows open, when one of its windows is reopened
by its own program rather than launched, or when it is a special workspace.
`hyprwake diff` reports a workspace whose windows are all open but tiled
differently from the snapshot.

Window groups come back as separate windows unless you ask for them:

```toml
[restore]
groups = "regroup"           # rebuild each group, tabs in their saved order
```

A group is rebuilt by launching its first window, making it a group, and
launching the others while the group has focus, so Hyprland's
`group:auto_group` (on by default) takes each one in; every member is checked
after it opens. A group whose windows a program reopens itself — a browser's
second window — or that spans workspaces comes back as separate windows, and
the report says why.

For an application none of the built-in kinds fits, `kind = "external"`
hands capture and relaunch to a helper program you write, speaking JSON;
hyprwake runs it only if nobody but you can change it, and checks what it
returns. The contract is in docs/PROVIDERS.md.

The `[terminals]`, `[apps]` and `[browsers]` tables from earlier releases
still work unchanged — including `[terminals]` replacing the built-in terminal
table when you list any — and are read into the same providers.

## What it cannot do

- **Groups of windows hyprwake does not launch.** `groups = "regroup"`
  rebuilds a group by launching its members into it, one at a time, while it
  has focus — Hyprland 0.56.2 cannot put an existing window into a group by
  address (`window.move { into_group }` ignores the window it is given and
  moves the focused one). Members a program reopens on its own arrive when
  they arrive, so their group comes back as separate windows. Which tab of a
  group was showing is not recorded; the restore ends on the window that had
  focus.
- **Every tiling, every time.** The split tree is inferred from window
  geometry — Hyprland does not expose it — and rebuilt only where it was read
  cleanly, with `[restore] layout = "best-effort"`, which is not the default.
  Windows reopened by their own program (a browser's second window, an
  Electron app's) arrive when they arrive, so a workspace holding one comes
  back in reading order; so do fullscreen and grouped windows' workspaces,
  special workspaces, and layouts other than dwindle and master (scrolling,
  centred master, a plugin's). Floating windows are always exact.
- **In-app state.** Scrollback, unsaved edits, scroll positions — except
  where a program can reopen its own session, which is what `[tui.resume]`
  is for. Content
  restoration rides on each app's own persistence, which is better than you
  would expect: browsers restore their tabs, editors their sessions, document
  viewers their last file. True per-window state restoration needs the Wayland
  `xdg-session-management` protocol, which Hyprland does not implement yet.
- **D-Bus-activated apps** whose `/proc` cmdline says nothing useful.
- **A different kernel.** Nothing here is a memory image; that is the point.

## Where it is going

[docs/ROADMAP.md](docs/ROADMAP.md): finishing 0.2 on real hardware, then a
snapshot timeline with `undo`, editor sessions, and the native session
protocol when Hyprland grows it.

## Development

```sh
cargo test        # 363 tests, no compositor required
cargo clippy --all-targets
```

The compositor and `/proc` both sit behind traits with test doubles, so
capture, restore, matching and the sweep are all exercised without a running
Hyprland.

### Keeping the installed copy current

```sh
scripts/install-dev-hooks.sh
```

After any commit touching `src/`, `scripts/` or `Cargo.*`, a post-commit hook
rebuilds, reinstalls, refreshes the desktop hooks so they point at the new
binary, restarts the watcher on it and takes a fresh snapshot — detached, so
the commit itself returns immediately. It reports through a desktop
notification and logs to `~/.local/state/hyprwake/dev-install.log`. A failed
build leaves the previously installed binary alone.

The watcher is single-instance: a second `hyprwake watch` refuses to start,
and `--replace` takes over from the running one, so "make sure it is running"
is safe to say twice. That is also what lets the systemd unit take over from
a watcher you started by hand.

## Packaging

Releases are cut by pushing a version tag. CI runs the suite, builds, publishes
a GitHub release with the binary and its checksum, and attaches a package
directory with the version and source checksum already filled in.

Arch users are served by the `PKGBUILD` here. For Omarchy there is the
**Omarchy Package Repository** — the `[omarchy]` pacman repo at
`pkgs.omarchy.org` that every Omarchy machine already has enabled, carrying
around 200 packages across three channels (`edge` → `rc` → `stable`).
Packages are defined in [omacom/omarchy-pkgs](https://github.com/omacom/omarchy-pkgs)
under `pkgbuilds/<name>/`, each with a `PKGBUILD` and an `.omarchy/package.json`;
`{"source": "local"}` means "build from this PKGBUILD" rather than mirroring
the AUR. Its metadata also declares the project's `v<version>` Git tags and
source tarballs, so OPR can detect a later release and prepare the checksum
bump automatically. Submitting is a pull request there, and acceptance is the
Omarchy team's call.

To produce the submission directory for a tag:

```sh
scripts/opr-bundle.sh v0.1.3
```

## Credits

hyprwake is a derivative work and carries the git history of the project it
started from. It is MIT-licensed, as are all three of its ancestors; see
[LICENSE](LICENSE) for the full attribution.

- [**hyprflow**](https://github.com/isorensen/hyprflow) by iSorensen — the
  crate this is forked from: its layout, the trait-based abstractions, named
  sessions, dry-run, max-age and autosave rotation.
- [**hypr-session-restore**](https://github.com/UpayanChatterjee/hypr-session-restore)
  by Upayan Chatterjee — the restore model ported here: `/proc`-derived launch
  commands, terminal and TUI detection, exec-rule spawning, the sweep pass.
- [**hypr-session-restore (Omarchy fork)**](https://github.com/SotoAugusto/hypr-session-restore)
  by SotoAugusto — uwsm launching, the empty-save guard, magic-workspace
  matching, dispatch-payload checking and the hook wiring.

Two bugs were found while porting, and are fixed here: a browser's argv
arrives from `/proc` as a single space-separated blob, which replayed as one
unusable argument; and a window that has to be floated before being positioned
is re-centred by the resize, so the position has to be applied last and
absolutely.

## License

MIT
