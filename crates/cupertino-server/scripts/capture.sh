#!/bin/bash
# Headless screenshot of the full desktop (server, shell processes, apps) with a fixture home folder, so a capture
# shows only sample files and is the same on every machine.
#
# Usage: capture.sh OUT.png [server flags...]
#   capture.sh files.png --spawn cupertino-files
#   AFTER=6 capture.sh switcher.png --spawn cupertino-files --spawn cupertino-settings --hold-modifiers --key 5:Super+Tab
# The fixture home, XDG directories and session bus are temporary; the environment is cleared, so nothing from the
# caller's HOME or XDG directories is read. One asset pack is used: $CUPERTINO_ASSET_DIR, else the user pack under
# $XDG_DATA_HOME (or ~/.local/share), else none (built-in assets only; the system pack is not searched).
# Environment: AFTER (seconds before the shot, default 5), SIZE (default 1280x800), BIN_DIR (default
# target/release, else target/debug; build with: cargo build --release --workspace --all-features --bins).
# The shell processes are the ones the packages install as /etc/cupertino/compositor.json, found on PATH (BIN_DIR first).
# XDG_SESSION_ID is passed on: cupertino-login (the lock agent) needs the caller's logind session.
# Needs: dbus-run-session.
set -eu

[ $# -ge 1 ] || { sed -n '5,7p' "$0" >&2; exit 2; }
OUT=$(realpath -m "$1")
shift
ROOT=$(cd "$(dirname "$0")/../../.." && pwd)
BIN_DIR=${BIN_DIR:-$ROOT/target/release}
[ -x "$BIN_DIR/cupertino-server" ] || BIN_DIR=$ROOT/target/debug
[ -x "$BIN_DIR/cupertino-server" ] || { echo "capture.sh: no cupertino-server in target/release or target/debug" >&2; exit 1; }

PACK=${CUPERTINO_ASSET_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/cupertino/assets}
[ -d "$PACK" ] || PACK=

SCRATCH=$(mktemp -d "${TMPDIR:-/tmp}/cupertino-capture.XXXXXX")
trap 'rm -rf "$SCRATCH"' EXIT
FHOME=$SCRATCH/home
RUNTIME=$SCRATCH/runtime
mkdir -p "$FHOME"/{Desktop,Documents/Projects,Downloads,Music,Pictures,Videos} \
    "$FHOME"/.config "$FHOME"/.local/share "$FHOME"/.local/state "$FHOME"/.cache "$RUNTIME"
chmod 700 "$RUNTIME"

cat >"$FHOME/.config/user-dirs.dirs" <<'EOF'
XDG_DESKTOP_DIR="$HOME/Desktop"
XDG_DOCUMENTS_DIR="$HOME/Documents"
XDG_DOWNLOAD_DIR="$HOME/Downloads"
XDG_MUSIC_DIR="$HOME/Music"
XDG_PICTURES_DIR="$HOME/Pictures"
XDG_VIDEOS_DIR="$HOME/Videos"
EOF
printf 'Sample notes for screenshots.\n' >"$FHOME/Documents/Meeting Notes.txt"
printf 'item,amount\nrent,1200\nfood,400\n' >"$FHOME/Documents/Budget.csv"
printf '# Project plan\n\n1. Draft\n2. Review\n' >"$FHOME/Documents/Projects/Plan.md"
printf 'Welcome to the sample desktop.\n' >"$FHOME/Desktop/Welcome.txt"
printf 'Sample download.\n' >"$FHOME/Downloads/Release Notes.txt"
: >"$FHOME/Pictures/Holiday.png"
: >"$FHOME/Music/Song.ogg"
touch -d 2026-01-01T12:00:00 "$FHOME" "$FHOME"/* "$FHOME"/*/* "$FHOME"/*/*/* 2>/dev/null || true

mkdir -p "$(dirname "$OUT")"
rm -f "$OUT"
cd "$FHOME"
env -i \
    HOME="$FHOME" \
    PATH="$BIN_DIR:/usr/local/bin:/usr/bin:/bin" \
    LANG=C.UTF-8 \
    TZ=UTC \
    XDG_CONFIG_HOME="$FHOME/.config" \
    XDG_DATA_HOME="$FHOME/.local/share" \
    XDG_STATE_HOME="$FHOME/.local/state" \
    XDG_CACHE_HOME="$FHOME/.cache" \
    XDG_RUNTIME_DIR="$RUNTIME" \
    XDG_CURRENT_DESKTOP=Cupertino \
    XDG_SESSION_ID="${XDG_SESSION_ID:-}" \
    XDG_DATA_DIRS="$ROOT/crates/cupertino-files/data:$ROOT/crates/cupertino-settings/data:/usr/local/share:/usr/share" \
    CUPERTINO_ASSET_SEARCH_PATH="$PACK" \
    CUPERTINO_SERVER_CONFIG="$ROOT/packaging/files/etc/cupertino/compositor.json" \
    dbus-run-session -- "$BIN_DIR/cupertino-server" --headless --dbus --size "${SIZE:-1280x800}" \
    --screenshot "$OUT" --screenshot-after "${AFTER:-5}" --exit-after-screenshot "$@"
[ -s "$OUT" ] || { echo "capture.sh: no screenshot written" >&2; exit 1; }
echo "$OUT"
