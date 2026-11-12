#!/usr/bin/env bash
# Build the Arch packages of the committed HEAD of this checkout for the machine it runs on, without the test suite.
#
#   scripts/package-local.sh [OUT_DIR]      OUT_DIR defaults to ~/.cache/cupertino-pkg
#
# It writes cupertino, cupertino-data and cupertino-kit with the stable recipe (packaging/arch/cupertino) and a release
# number from the current time, so every build is newer than the one installed before it. Install them together:
#
#   sudo pacman -U OUT_DIR/*.pkg.tar.zst
#
# Uncommitted changes are not in the packages. The Cargo target directory is kept between runs
# (CARGO_TARGET_DIR, default ~/.cache/cupertino-build/pkg), so a later build compiles only what changed.
set -euo pipefail

root=$(git -C "$(dirname "$0")" rev-parse --show-toplevel)
out=${1:-$HOME/.cache/cupertino-pkg}
[ "$#" -le 1 ] || { echo 'usage: package-local.sh [OUT_DIR]' >&2; exit 2; }
command -v makepkg >/dev/null || { echo 'package-local.sh: makepkg is missing (Arch Linux only)' >&2; exit 1; }
if [ -n "$(git -C "$root" status --porcelain --untracked-files=no)" ]; then
    echo 'package-local.sh: uncommitted changes are left out; the packages hold HEAD' >&2
fi

mkdir -p "$out"
work=$(mktemp -d "$out/.build.XXXXXX")
trap 'rm -rf "$work"' EXIT
archive=$(cd "$root" && python3 packaging/release.py source --output "$work")
(cd "$root" && python3 packaging/release.py pkgbuild "$archive" "$work/PKGBUILD") >/dev/null
sed -i "s/^pkgrel=.*/pkgrel=$(date +%Y%m%d%H%M)/" "$work/PKGBUILD"

export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$HOME/.cache/cupertino-build/pkg}
(cd "$work" && BUILDDIR="$work/build" PKGDEST="$work" makepkg --force --nocheck --noconfirm)

rm -f "$out"/cupertino-*.pkg.tar.zst
mv "$work"/cupertino-*.pkg.tar.zst "$out"/
ls -1 "$out"/cupertino-*.pkg.tar.zst
