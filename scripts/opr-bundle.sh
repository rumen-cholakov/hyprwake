#!/usr/bin/env bash
# opr-bundle.sh — build the package directory to submit to the Omarchy
# Package Repository.
#
#   scripts/opr-bundle.sh v0.1.0 [output-dir]
#
# Produces <output-dir>/hyprwake/{PKGBUILD,.omarchy/package.json} with the
# version and source checksum filled in, ready to copy into a pull request
# against https://github.com/omacom/omarchy-pkgs under pkgbuilds/.
#
# The release workflow attaches the same bundle to every GitHub release; this
# script is for producing one by hand.

set -euo pipefail

tag="${1:-}"
outdir="${2:-target/opr}"
if [[ -z $tag ]]; then
    echo "usage: $0 <tag> [output-dir]   e.g. $0 v0.1.0" >&2
    exit 1
fi
version="${tag#v}"

repo_root=$(git rev-parse --show-toplevel)
cd "$repo_root"

# The canonical PKGBUILD carries the project URL; derive the source from it
# rather than hardcoding an owner here.
url=$(sed -n 's/^url="\(.*\)"$/\1/p' PKGBUILD)
[[ -n $url ]] || { echo "could not read url= from PKGBUILD" >&2; exit 1; }
tarball="$url/archive/refs/tags/$tag.tar.gz"

echo "fetching $tarball"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
if ! curl -fsSL "$tarball" -o "$tmp/source.tar.gz"; then
    echo "could not download $tarball" >&2
    echo "push the tag first: git push origin $tag" >&2
    exit 1
fi
sha256=$(sha256sum "$tmp/source.tar.gz" | cut -d' ' -f1)

mkdir -p "$outdir/hyprwake/.omarchy"
sed -e "s/^pkgver=.*/pkgver=$version/" \
    -e "s/^pkgrel=.*/pkgrel=1/" \
    -e "s/^sha256sums=.*/sha256sums=('$sha256')/" \
    PKGBUILD > "$outdir/hyprwake/PKGBUILD"
cp packaging/opr/package.json "$outdir/hyprwake/.omarchy/package.json"

echo
echo "wrote $outdir/hyprwake"
echo "  pkgver     $version"
echo "  sha256sums $sha256"
echo
echo "To submit:"
echo "  git clone https://github.com/omacom/omarchy-pkgs"
echo "  cp -r $outdir/hyprwake omarchy-pkgs/pkgbuilds/"
echo "  # commit on a branch and open a pull request"
