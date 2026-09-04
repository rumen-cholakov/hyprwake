#!/usr/bin/env bash
# Wire the repository's git hooks into this clone.
#
# Hooks live in scripts/ so they are versioned and reviewable; .git/hooks is
# not part of the repository, so it gets symlinks.

set -euo pipefail

repo=$(git rev-parse --show-toplevel)
hooks_dir="$repo/.git/hooks"
mkdir -p "$hooks_dir"

for hook in pre-commit post-commit; do
    src="$repo/scripts/$hook"
    dest="$hooks_dir/$hook"
    chmod +x "$src"
    if [[ -e $dest && ! -L $dest ]]; then
        echo "refusing to replace an existing $dest; move it aside first" >&2
        exit 1
    fi
    ln -sfn "$src" "$dest"
    echo "linked $dest -> $src"
done

echo
echo "Before each commit: rustfmt and clippy, the two checks CI runs first."
echo
echo "After each commit touching src/, scripts/ or Cargo.*, hyprwake will"
echo "rebuild, reinstall, refresh its desktop hooks and restart the watcher."
echo "Progress is logged to \${XDG_STATE_HOME:-~/.local/state}/hyprwake/dev-install.log"
