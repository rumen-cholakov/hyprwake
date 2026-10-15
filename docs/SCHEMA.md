# Snapshot schema

A snapshot is one JSON file per session in `~/.local/share/hyprwake/sessions/`.
This is what it contains, by version. It records window titles, command lines,
executable paths and working directories — see the README's Privacy section
before sharing one.

## Versioning

`schema_version` says which format a snapshot is in. Snapshots written before
0.2.0 carry no version and are read as **v1**.

- An **older** snapshot is migrated in memory when it is loaded, one version at
  a time, and restores unchanged. The file on disk is not rewritten until the
  next save. `hyprwake status` and `doctor` say when a snapshot was migrated.
- A **newer** snapshot — written by a later hyprwake — is refused with a
  message naming its version, rather than restored with every field this build
  does not understand silently dropped.
- The version changes once per release that changes what a field means, never
  once per field. Additions that older builds can safely ignore stay within a
  version.

| Version | Written by | Change |
|---|---|---|
| 1 | 0.1.0 – 0.1.3 | The original format. No `schema_version` field. |
| 2 | 0.2.0 | `schema_version`, `created_by`, per-workspace records with the inferred split tree, monitor identity, window identity, provider attribution. Every addition defaults when absent, so v1 → v2 only stamps the version. |

The external provider contract (`docs/PROVIDERS.md`) is versioned separately,
by its own `contract` number.

## v2

### Session

| Field | Type | Since | Meaning |
|---|---|---|---|
| `schema_version` | integer | v2 | `2`. Absent means 1. |
| `created_by` | string | v2 | The hyprwake version that wrote it; empty after migrating a v1 snapshot. |
| `name` | string | v1 | The session's name; also the file name. |
| `created_at` | RFC 3339 timestamp | v1 | When it was saved, UTC. |
| `hyprland_version` | string | v1 | The compositor that was running. |
| `monitors` | [Monitor] | v1 | Every attached monitor. |
| `workspaces` | [Workspace] | v2 | One per workspace that held a window. |
| `clients` | [Client] | v1 | Every saved window, in the order the compositor listed them. Indices into this list are how other fields refer to a window. |
| `browser_profiles` | [BrowserProfile] | v1 | Browser profiles to reopen, one window each. |

### Monitor

| Field | Type | Since | Meaning |
|---|---|---|---|
| `name` | string | v1 | Connector (`eDP-1`, `DP-2`). Where the display was plugged in, not which display it was. |
| `width`, `height` | integer | v1 | Mode, in physical pixels. |
| `transform` | integer | v1 | Hyprland's 0–7 rotation and flip. |
| `active_workspace` | WorkspaceRef \| null | v1 | What it was showing. |
| `focused` | boolean | v1 | Whether it held the focus. |
| `make`, `model`, `serial` | string | v2 | Panel identity, so the same display is recognised on another port. `serial` is often empty on built-in panels. |
| `scale` | number | v2 | Default `1.0`. |
| `position` | [x, y] | v2 | In the layout, logical pixels. Floating geometry is only refitted onto another monitor when this is known, so v1 snapshots are never refitted. |

### Workspace

| Field | Type | Since | Meaning |
|---|---|---|---|
| `workspace` | WorkspaceRef | v2 | Which workspace. |
| `monitor` | string | v2 | The connector it was on. |
| `layout` | LayoutNode \| null | v2 | The split tree its tiled windows were in, read back from their geometry. `null` when none could be read, or `[restore] layout = "off"`. |
| `layout_kind` | string \| null | v2 | `dwindle`, or `master-left`, `master-right`, `master-top`, `master-bottom`. `null` for a layout that is not inferred (scrolling, centred master). |
| `layout_confidence` | number \| null | v2 | 0.0–1.0: how cleanly the windows tiled the workspace. `0.0` when no tree was read. `best-effort` restores rebuild trees at or above `layout_min_confidence`. |

### LayoutNode

Tagged by `node`:

- `{"node": "leaf", "client": 3}` — the window at `clients[3]`.
- `{"node": "split", "axis": "horizontal" | "vertical", "ratio": 0.6, "a": …, "b": …}`
  — `horizontal` puts `a` left of `b`, `vertical` puts `a` above `b`; `ratio`
  is the share of the box `a` takes. For master layouts the tree is
  geometric too: the masters are the side `layout_kind` names.

### Client

| Field | Type | Since | Meaning |
|---|---|---|---|
| `class` | string | v1 | Window class. |
| `initial_class` | string | v1 | The class it announced when it first mapped; found again after an application renames itself. |
| `title` | string | v1 | Window title. Orders otherwise equal candidates when matching; never decides whether a window matches. |
| `workspace` | WorkspaceRef | v1 | Where it was. |
| `monitor` | string | v1 | The connector it was on. |
| `at`, `size` | [x, y], [w, h] | v1 | Geometry, layout pixels, excluding the border. Exact for floating windows; the source of the inferred tree for tiled ones. |
| `floating`, `pinned` | boolean | v1 | |
| `fullscreen` | integer | v1 | Hyprland's fullscreen state: bit 1 fullscreen, bit 0 maximised. |
| `focus_history_id` | integer | v1 | 0 is the window that had focus. |
| `group` | integer \| null | v1 | Windows sharing a value were one group. Rebuilt as a group with `[restore] groups = "regroup"`, separate windows otherwise. |
| `group_position` | integer \| null | v2 | Its tab in that group, 0 first, as the group bar ordered them. |
| `identity` | Identity | v2 | Diagnostics; see below. |
| `launch` | Launch | v1 | How to bring it back. |

### Identity (v2)

What else was known about the window. Recorded for support reports; nothing
here takes part in matching, because within a class these are the same for
every window on real desktops.

| Field | Type | Meaning |
|---|---|---|
| `executable` | path \| null | `/proc/<pid>/exe`, with the kernel's `(deleted)` suffix removed. |
| `initial_title` | string \| null | The title it mapped with. |
| `provider` | string \| null | The provider that captured it (same as `launch.provider`). |
| `desktop_file`, `app_id`, `profile` | string \| null | Reserved; not populated, because none can be read per window without guessing. |

### Launch

| Field | Type | Since | Meaning |
|---|---|---|---|
| `argv` | [string] | v1 | What restore launches, already resolved: a terminal's own command line with its directory and the program it was running, a browser's argv, or whatever an external provider returned. |
| `spawn` | boolean | v1 | `false` for windows restore does not launch: a program's second window, which it reopens itself, or a provider with `spawn = false`. Default `true`. |
| `provider` | string \| null | v2 | Config key of the provider that captured it; `generic` when nothing claimed the class. |
| `provider_state` | JSON \| null | v2 | An external provider's own state, at most 16 KiB, handed back to it at restore. Never interpreted by hyprwake. |

### BrowserProfile

| Field | Type | Meaning |
|---|---|---|
| `class` | string | The browser's window class. |
| `directory` | string | Profile directory (Chromium) or profile name (Firefox). |
| `name` | string | Display name. |
| `workspace` | string | Where its window goes. |

### WorkspaceRef

`{"id": 3, "name": "3"}`. Compared by dispatcher selector, not by id: named
workspaces are `name:<name>`, special ones `special:<name>`, whose ids differ
between sessions.

## v1

The v2 tables without the fields marked v2. No `schema_version`, `created_by`
or `workspaces`; monitors without `make`, `model`, `serial`, `scale` and
`position`; clients without `identity` and `group_position`; `launch` without `provider` and
`provider_state`. `tests/fixtures/session-v1.json` is a real 0.1.3 snapshot,
kept so that this keeps loading.
