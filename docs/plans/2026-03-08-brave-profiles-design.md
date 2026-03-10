# Brave Profile Support — Design

**Date:** 2026-03-08
**Status:** Approved
**Scope:** v0.2

## Problem

Brave runs all windows/profiles in a single process. There is no way to determine which window belongs to which profile via PID or cmdline. All Brave windows share one PID and the `--profile-directory` flag is not present in the process arguments.

## Solution

### Capture

- Read `~/.config/BraveSoftware/Brave-Browser/Local State` (JSON)
- Extract active profiles from `profile.info_cache` (keys = directory names like `Default`, `Profile 1`; values contain `name` = human-readable name)
- Store active profile list in the session as metadata (new field on `Session`)
- Continue saving Brave windows as today (positions preserved for reference, but not used during restore)

### Restore

- Skip individual Brave window entries from the session (cannot map window to profile)
- For each saved active profile, launch `brave --profile-directory=X` once
- Move the resulting window to the workspace configured in `config.toml`
- Profiles without explicit workspace mapping go to `default_workspace`
- No pixel-perfect positioning (Brave manages its own window layout)

### Configuration

```toml
[apps.brave-browser]
binary = "brave"
profile_workspaces = { "Default" = 1, "Profile 1" = 6 }
default_workspace = 1
```

### Data Model

New field on `Session`:
```rust
pub struct Session {
    // ... existing fields ...
    pub brave_profiles: Vec<BraveProfile>,
}

pub struct BraveProfile {
    pub directory: String,  // "Default", "Profile 1", etc.
    pub name: String,       // "Credifit", "LinkPJ", etc.
}
```

### Constraints

- Only one window per profile on restore (Brave has built-in session restore for tabs)
- No attempt to position Brave windows pixel-perfect
- `Local State` path is hardcoded to `~/.config/BraveSoftware/Brave-Browser/Local State` (standard Linux path)
