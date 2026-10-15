# Providers

A *provider* decides how a window of a given class is captured and brought
back. `hyprwake providers` lists every provider in effect and where it was
declared; `hyprwake providers <class>` explains how one class resolves. Paste
that output into a compatibility report — it is most of what a maintainer
needs to know.

## The model

The **mechanisms** are a closed set, built into hyprwake:

| `kind` | What it does |
|---|---|
| `terminal` | Reopen the terminal at its shell's directory. If a program from `[tui] programs` was running in it, reopen that program inside it — resumed on its own session where `[tui.resume]` knows how. |
| `chromium` | A Chromium-family browser. Profiles are read from `Local State` and each mapped profile is reopened with `--profile-directory`. |
| `firefox` | A Firefox-family browser. Profiles are read from `profiles.ini` and each mapped profile is reopened by name with `-P`. |
| `command` | Replay the window's own argv from `/proc`, optionally with a different binary or some arguments stripped. |
| `external` | Hand capture and relaunch to a helper program you write, speaking JSON — see [External providers](#external-providers). |

The **instances** are yours, declared in `~/.config/hyprwake/config.toml`:

```toml
[providers.wezterm]
kind = "terminal"
classes = ["org.wezfurlong.wezterm"]
binary = "wezterm"
cwd_flag = "--cwd"              # a trailing "=" joins flag and value
exec_flag = "-e"                # what introduces a program, if anything
extra_args = []                 # e.g. an --app-id that pins the window class

[providers.brave]
kind = "chromium"
classes = ["brave-browser"]
binary = "brave"
local_state = "BraveSoftware/Brave-Browser/Local State"   # under ~/.config
profile_workspaces = { "Default" = "2", "Profile 1" = "6" }
restore_last_session = false    # see below

[providers.zen]
kind = "firefox"
classes = ["zen", "zen-alpha"]
binary = "zen-browser"
profiles_ini = ".zen/profiles.ini"                         # under ~
profile_workspaces = { "default" = "2" }

[providers.jetbrains]
kind = "command"
classes = ["jetbrains-*"]       # a trailing * matches a prefix
strip_args = ["-Dtmp.session"]  # dropped by prefix
spawn = false                   # record the window, never launch it
```

Every entry takes `classes` and optionally `spawn`. A `chromium`, `firefox` or
`command` entry with a key it does not know is a load error, with the line
number — so `restore_last_session` on a Firefox browser, or `strip_arg` for
`strip_args`, fails loudly instead of doing nothing. A `terminal` entry ignores
unknown keys, because it shares its fields with the legacy `[terminals]`
table and a config that loaded under 0.1.3 must still load.

A mechanism that is not in the table needs Rust, or an `external` helper. An
instance never does.

## Which provider wins

Resolution does not depend on the order of tables in the file:

1. an exact class claimed in `[providers]`
2. an exact class claimed anywhere else (built-in, `[terminals]`, `[apps]`,
   `[browsers]`)
3. the longest matching `*` pattern
4. `generic`, which replays the window's own argv

Two `[providers]` entries claiming the same exact class is reported, and the
first key in sort order wins. `hyprwake providers` and `hyprwake doctor` both
show such problems.

A `[providers]` entry adds to the built-ins rather than replacing them; one
whose key matches a built-in (say `[providers.foot]`) redefines it.

## The legacy tables

`[terminals]`, `[apps]` and `[browsers]` still work, unchanged, and are read
into the same set. They keep their 0.1.3 meaning exactly — a test pins the
argv they produce:

- Listing any `[terminals]` entry replaces the whole built-in terminal table.
- An `[apps]` entry for a terminal's class only contributes `no_spawn`.
- An `[apps]` entry for a browser's class still applies its `binary`,
  `strip_args` and `no_spawn` to that browser's window argv.

## Session managers own their contents

herdr, tmux, zellij and screen keep panes alive in a server of their own.
hyprwake restores the terminal and the attach; the session manager restores
everything inside it. hyprwake never reconstructs a pane itself, which would
duplicate it.

This falls out of how capture works: the search for a program inside a
terminal stops at the first match, which is the session manager's client, and
never descends into its panes.

## Resume rules

`[tui.resume.<program>]` recovers the session a program inside a terminal was
in, and asks it to reopen it. The key is the program's process name as the
kernel reports it (`/proc/<pid>/comm`).

- `fd_glob` reads the id out of a path the program holds open. Exact.
- `id_command` runs a helper that prints the id for a directory. `{cwd}`,
  `{cwd_sql}` (single quotes doubled), `{home}` and `{pid}` (the program's
  own process id) are substituted. It runs as
  you, at every save, with a 3-second timeout, and the id it prints is checked
  before it reaches a command line.

**Never write a placeholder inside shell script text.** A directory can be
named `$(…)`, and a script would run it. Run the helper directly where you
can, and when a shell is unavoidable pass values as positional parameters:

```toml
# Safe: the query is an ordinary argument, no shell involved.
id_command = ["sqlite3", "file:{home}/.x/db?mode=ro",
              "SELECT id FROM s WHERE dir = '{cwd_sql}'"]

# Safe: $1 and $2 are never parsed as shell.
id_command = ["sh", "-c", "exec sqlite3 \"$1\" \"$2\"",
              "sh", "{home}/.x/db", "SELECT id FROM s WHERE dir = '{cwd_sql}'"]

# UNSAFE, and refused: {cwd_sql} inside the script.
id_command = ["sh", "-c", "sqlite3 db \"SELECT … '{cwd_sql}'\""]
```

hyprwake refuses to substitute a directory containing `$`, `` ` ``, quotes,
`;`, `&`, `|`, `<`, `>`, parentheses, braces or control characters into the
script argument of `sh -c`, `bash -c`, `python -c`, `perl -e`, `node -e` and
similar; the rule falls back instead. 0.1.0 to 0.1.3 shipped a codex rule
that did exactly this — see the 0.2.0 changelog.

### Session managers: the tmux recipe

```toml
[tui]
programs = ["nvim", "yazi", "btop", "tmux: client"]

[tui.resume."tmux: client"]
# The session this client is on, asked of the server by the client's pid.
# {pid} is passed as a positional parameter; the script never sees it as text.
id_command = ["sh", "-c",
  "tmux list-clients -F '#{client_pid} #{session_name}' | awk -v p=\"$1\" '$1 == p { print $2; exit }'",
  "sh", "{pid}"]
args = ["new-session", "-A", "-s", "{id}"]
fallback = ["new-session"]
keep_flags = ["-S", "-L", "-f"]   # the client's socket and config, kept as they were
```

`{pid}` is the program's own process id, for asking a server which client is
which. `keep_flags` turns a rule from adding arguments into replacing them:
the program and those flags (each with its value) are kept, and `args` — or
`fallback` — follow. A session name with spaces fails the id check and falls
back to a new session. With a server on another socket (`-S`), point the
helper at it too, as `tmux -S <path> list-clients …`.

## What ships, and why

A shipped default has to keep working across upstream releases nobody here
controls, so the bar is: packaged for Arch or with a mainstream installer,
validated on a real machine, and degrading to today's behaviour when stale.
Anything else is a recipe below. "Validated" means against a real install,
with the version recorded.

### AI agent TUIs

| Program | Status | Notes |
|---|---|---|
| claude | shipped | `fd_glob` on the per-session directory it holds open; `--continue` fallback. |
| codex | shipped | `id_command` on its state database; `resume --last` fallback. Fixed in 0.2.0 not to splice the directory into a shell script. |
| opencode | shipped | Validated against 1.18.32. `id_command` on `~/.local/share/opencode/opencode.db`, no shell. Excludes child sessions, which opencode writes for every subagent run in the parent's directory and which are usually the newest row. `--continue` fallback, which inside a git repository is project-wide rather than per-directory. |
| crush | recipe | Resumes with `--session <id>`; `crush session last --cwd <dir>` is a candidate lookup. Not shipped: never used on the validation machine, so there are no sessions to check against, and `crush session` talks to a crush server socket — whether a lookup starts one is unconfirmed. |
| gemini | recipe | A Node program: the kernel calls it `node-MainThread` (Node 26), and since 0.2.0 hyprwake names an interpreted program by its script, so `gemini` in `[tui] programs` finds it and reopens it by name — which survives a mise upgrade moving its install. Not in the defaults, and no resume rule: it resumes by *index* (`--resume 5`), which shifts as sessions are added. |
| aider, goose, amp | recipe | Not installed on the validation machine. aider (Python, also `python -m aider`) and amp (Node) are found by script name like gemini; add them to `[tui] programs` to have them reopened. |

### Session managers

| Program | Status | Notes |
|---|---|---|
| herdr | shipped | Validated against 0.8.2. Needs nothing: `--session <name>` survives in the client's argv and is replayed, and both `herdr --session <name>` and `herdr session attach <name>` restart a stopped session with its workspaces. |
| tmux | recipe | Validated against tmux 3.7c in a nested compositor. The client's process name is `tmux: client`, and its server does not survive a reboot, so replaying `tmux attach -t work` would fail and close the terminal. The recipe below asks the server which session the client was on, by its pid, and comes back as `tmux new-session -A -s <name>` — attaching if tmux-resurrect restored the session, creating it otherwise — keeping any `-S`/`-L`/`-f`. Not a default: it changes what a terminal running tmux comes back as. |
| zellij, screen, abduco | recipe | Not installed on the validation machine. |

### Terminals

foot, the Omarchy terminal (foot with its own app-id), kitty, Alacritty and
Ghostty ship. wezterm, rio, contour, konsole, gnome-terminal, xterm and st are
not installed on the validation machine, so their flags have not been checked
against their own `--help`, which is the bar for shipping. Declare them with
`kind = "terminal"`. A terminal that sets its window class from a flag needs
that flag in `extra_args`, as the Omarchy terminal's `--app-id` does, or a
relaunched window will not be recognised as the saved one.

### Browsers

Chromium-family and Firefox-family browsers need no new mechanism. Google
Chrome and Chromium are confirmed on the validation machine; the other
entries `hyprwake config --init` knows about (Brave, Edge, Vivaldi, Firefox,
Zen, LibreWolf) are offered only when their profile file exists.

`restore_last_session = true` passes `--restore-last-session`. It is off by
default, because it overrides a preference you may have set on purpose.
Measured against Chrome 154 in a nested compositor, with throwaway profiles
and a control copy of each:

| Case | With the flag | Without |
|---|---|---|
| Cold start, startup setting "Open the New Tab page" | tabs restored | New Tab |
| After `kill -9` — the unclean exit a reboot can be | tabs restored | New Tab |
| Browser already running for that profile | one extra New Tab window; nothing duplicated | — |
| Two profiles, opened one after the other | only the **first** restored | — |

The last row is the one to know: the flag takes effect only when the browser
process starts, so a second profile, opened into the running browser,
ignores it and follows its own "On startup" setting. With several profiles
mapped, set each to "Continue where you left off"
(`session.restore_on_startup = 1`) — measured to restore a profile opened
second — and `doctor` names any mapped profile that would open a New Tab
page instead. Firefox has no equivalent switch — only its
`browser.startup.page` preference — so the option does not exist for
`kind = "firefox"`.

### Out of scope for 0.2.0

- **Editor sessions.** nvim, helix and emacs are reopened at the right
  directory but not at the right session (`Session.vim`, desktop.el). That is
  a third mechanism, neither `fd_glob` nor `id_command`.
- **Remote shells and containers.** A terminal running `ssh`, `docker exec`,
  `distrobox enter` or `toolbox enter` comes back as a local shell where the
  host shell stood — the remote or container shell is not a descendant of the
  terminal, so the local directory is what is captured, and it exists.
  Reconnecting automatically at boot is deliberately not done.

## External providers

When no built-in mechanism fits an application with a window class of its
own, `kind = "external"` hands its windows to a helper you write:

```toml
[providers.mail]
kind = "external"
classes = ["mailapp"]
capture = ["~/.config/hyprwake/providers/mail", "capture"]
relaunch = ["~/.config/hyprwake/providers/mail", "relaunch"]   # optional
```

The helper reads one JSON object on stdin and writes one on stdout. The
contract is versioned on its own — `contract`, currently `1` — independently
of the snapshot schema; a reply with another version is not believed.

**capture** runs at every save, once per window of a claimed class:

```json
{"contract": 1, "event": "capture", "class": "mailapp", "pid": 4242,
 "cwd": "/home/you", "argv": ["/usr/bin/mailapp", "--tray"]}
```

It answers with what to launch, opaque state of its own, or both:

```json
{"contract": 1, "argv": ["mailapp", "--folder", "Inbox"], "state": {"folder": "Inbox"}}
```

Without `argv` the window's own argv is replayed. `state` — at most 16 KiB of
JSON — is stored in the snapshot as the window's `provider_state` and handed
back, untouched, at restore.

**relaunch**, if configured, runs at restore before anything is launched,
once per saved window that provider captured:

```json
{"contract": 1, "event": "relaunch", "class": "mailapp", "workspace": "2",
 "argv": ["mailapp", "--folder", "Inbox"], "state": {"folder": "Inbox"}}
```

and answers with the argv to launch, and optionally another workspace:

```json
{"contract": 1, "argv": ["mailapp", "--open", "Inbox"], "workspace": "name:mail"}
```

`restore --dry-run` prints each relaunch reply verbatim, ahead of the launch
it decided.

### What a helper is trusted with

A helper runs as you, unattended, at every save; what it returns reaches a
command line. So hyprwake checks before it believes:

- **The helper must be yours.** An absolute path (a leading `~/` is
  expanded) to an executable file you own that neither group nor others can
  write, in a directory nobody else can write. Symbolic links are followed
  and the target is checked. Anything else is never run.
- **Three seconds**, the same budget as a resume `id_command`. A helper that
  takes longer is killed.
- **A failure is a failure.** Non-zero exit, a reply that is not JSON, an
  unknown key, a different contract version: the answer is discarded.
- **argv is checked.** Not empty; the first element is a program, not a flag;
  no more than 256 arguments of at most 4096 bytes; no control characters,
  so nothing can end or split a line. Shell syntax is fine — every argument
  is quoted on its way to the compositor, so `$(…)` arrives as a literal
  argument.
- **A workspace must be a selector** as Hyprland's dispatchers take it: `3`,
  `name:mail`, `special:magic`. `"3 silent"` or anything with a space is
  refused.

Whatever is refused, the window is still saved and restored — from its own
argv, exactly as if no provider claimed it — and the reason is in the log,
in the restore report, and in `doctor`. `hyprwake providers <class>` shows
whether each helper will run.

### Why not for session managers

tmux, zellij and herdr run *inside* a terminal; their window class is the
terminal's. A provider claimed by window class never sees them. The hook for
those is the resume layer above, which already runs helpers per program.
