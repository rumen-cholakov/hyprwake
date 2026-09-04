# Compatibility matrix

This tracks environments confirmed by a real reboot, rather than assuming
that a package build or a unit test proves a desktop setup works. Please file a
[compatibility report](https://github.com/rumen-cholakov/hyprwake/issues/new?template=compatibility_report.md)
with the requested details; maintainers add confirmed reports here.

| Omarchy | Hyprland | Architecture | Terminal | Browser | Monitor setup | Result |
|---|---|---|---|---|---|---|

## Automated coverage

| Coverage | Architectures | What it establishes |
|---|---|---|
| Unit and CLI tests | x86_64, aarch64 | Capture, matching, restore planning, and package build logic work without a compositor |
| Arch package validation | x86_64 | The PKGBUILD passes `makepkg --printsrcinfo` and `namcap` |

