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
| Window state | pinned, fullscreen, maximized |
| Browser profiles | one window per profile, each on its own workspace — Chromium family via `Local State`, Firefox family via `profiles.ini` |
| Focus | never stolen — everything is placed silently, in the background |

## How it works

Windows are not launched and then chased around. Each program is started
through Hyprland's own `exec_cmd` dispatcher carrying window rules, so the
compositor places the window the instant it maps — right workspace, right
geometry, no polling, no race, no focus theft.

That covers every program the compositor can attribute to the process it
spawned. Single-instance programs — browsers, Electron apps, Obsidian — hand
the request to an already-running process instead, and their window arrives
unattributed. A sweep afterwards pairs those strays with their saved entry by
window class and places them explicitly.

Saving runs off Hyprland's event socket. `hyprwake watch` saves a couple of
seconds after the layout stops changing, so a crash or a power cut costs you
nothing, while an idle desktop writes nothing at all.

## Install

```sh
cargo install --git https://github.com/rumen-cholakov/hyprwake --tag v0.1.2
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

## Usage

```sh
hyprwake save [NAME]            # snapshot; NAME defaults to "latest"
hyprwake restore [NAME]         # reopen it
hyprwake restore --dry-run      # print every dispatch instead of running it
hyprwake restore --max-age 7d   # skip a session older than this
hyprwake restore --force        # restore even though windows are open
hyprwake restore --missing-only # open only what is missing, leave the rest
hyprwake list -v                # saved sessions, and what is in them
hyprwake delete NAME

hyprwake watch                  # save when the layout settles (event-driven)
hyprwake watch --replace        # take over from a watcher already running
hyprwake daemon                 # save on a timer instead
hyprwake autosave --install     # timestamped snapshots via a systemd timer
hyprwake autosave --now         # snapshot and rotate

hyprwake doctor                 # check the whole pipeline
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

## Configuration

Optional; `~/.config/hyprwake/config.toml`. `hyprwake config --init` writes one
describing what is actually installed here.

```toml
[general]
sweep_timeout_secs = 20      # how long to wait for late windows
debounce_ms = 3000           # quiet period before an event-driven save

[launch]
use_uwsm = true              # launch through uwsm-app (systemd scopes)

[filters]
ignore_classes = ["waybar", "wofi", "mako"]

[terminals.foot]             # listing any terminal replaces the built-in table
binary = "foot"
cwd_flag = "-D"              # a trailing "=" joins flag and value
extra_args = []              # e.g. an --app-id that pins the window class

[tui]
programs = ["nvim", "yazi", "btop"]   # relaunched inside their terminal

# Programs that can reopen a specific session. Two strategies, tried in
# order; both rules below ship by default.
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
# is validated before it reaches a command line.
[tui.resume.codex]
id_command = ["sh", "-c", "... SELECT id FROM threads WHERE cwd = '{cwd_sql}' ..."]
args = ["resume", "{id}"]
fallback = ["resume", "--last"]
strip_flags = ["resume", "--last"]

[apps.Spotify]
no_spawn = true              # record the window, never launch it

[browsers.google-chrome]
binary = "google-chrome-stable"
kind = "chromium"                     # profiles from Local State
local_state = "google-chrome/Local State"
profile_workspaces = { "Default" = "2", "Work" = "6" }

[browsers.zen]
binary = "zen-browser"
kind = "firefox"                      # profiles from profiles.ini, by name
profiles_ini = ".zen/profiles.ini"
profile_workspaces = { "default" = "2", "work" = "6" }
```

## What it cannot do

- **Window groups.** Membership is recorded, but the members come back as
  separate windows on the right workspace rather than regrouped, and the
  restore report says which groups those were. Hyprland 0.56 has no way to
  put a window into an existing group by address: `into_group` acts on the
  focused window and takes a direction — which does not merge adjacent groups
  in practice — and the `group` spawn rule joins whatever is focused, which a
  silent restore never is. The data is captured so this becomes a small change
  the day a targeted dispatcher exists.
- **Exact tiling layout.** Hyprland does not expose the dwindle/master split
  tree. Windows return to the right workspace in their old reading order and
  re-tile from there — close, but not split-for-split identical. Floating
  windows *are* exact.
- **In-app state.** Scrollback, unsaved edits, scroll positions — except
  where a program can reopen its own session, which is what `[tui.resume]`
  is for. Content
  restoration rides on each app's own persistence, which is better than you
  would expect: browsers restore their tabs, editors their sessions, document
  viewers their last file. True per-window state restoration needs the Wayland
  `xdg-session-management` protocol, which Hyprland does not implement yet.
- **D-Bus-activated apps** whose `/proc` cmdline says nothing useful.
- **A different kernel.** Nothing here is a memory image; that is the point.

## Development

```sh
cargo test        # 201 tests, no compositor required
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
scripts/opr-bundle.sh v0.1.2
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
