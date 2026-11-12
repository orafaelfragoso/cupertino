#!/bin/bash
# The full desktop (server, shell processes, apps) headless with a fixture home folder, so what it draws shows only
# sample files and is the same on every machine; capture.sh (one screenshot) and ci/demo-video.sh (a video) run it.
#
# Usage: session.sh [server flags...]
#   session.sh --spawn cupertino-files --shot 5:/tmp/files.png --run-for 6
# The fixture home, XDG directories and session bus are temporary; the environment is cleared, so nothing from the
# caller's HOME or XDG directories is read. Asset packs: $CUPERTINO_ASSET_DIR alone when set (an empty value selects
# the built-in fallback); else the user pack under $XDG_DATA_HOME (or ~/.local/share) when there is one, then the
# checkout's original icon pack, per name in that order. System packs are not searched.
# Environment: SIZE (default 1280x800), BIN_DIR (default
# target/release, else target/debug; build with: cargo build --release --workspace --all-features --bins).
# FIXTURE_DIR: a folder whose contents are copied into the fixture home first (sample pictures, say); links are followed,
# so a link in it is copied as the file it points to, never kept as a link into the caller's files.
# CONFIG_JSON: a JSON object whose keys are set in the server's compositor.json, where `@HOME@` stands for the fixture
# home (CONFIG_JSON='{"wallpaper_folders": ["@HOME@/Art"]}').
# The shell processes are the ones the packages install as /etc/cupertino/compositor.json, found on PATH (BIN_DIR first; the
# server runs with --dev-shell, which lets a checkout name programs outside /usr),
# without cupertino-login: it works on the system bus and the caller's logind session, which a capture must not touch.
# HOME_NAME: the fixture home folder's name, which Files shows as its title (default home).
# SYSTEM_DATA_DIRS: the data directories after the checkout's (default /usr/local/share:/usr/share; the applications installed
# there show in the launcher).
# CUPERTINO_DOCK_FIXTURE_UNRESPONSIVE: application ids (comma separated) the dock shows as not responding; the script builds
# a dock for it into target/capture-fixtures.
# The volume is a stand-in `wpctl` at a fixed level (no sound device or PipeWire of the caller's is read).
# CUPERTINO_ACCESSIBILITY_CONF: an accessibility.conf the server reads its display options from (increase_contrast = on,
# say); without it the run has the defaults.
# Needs: dbus-run-session, python3.
set -eu

ROOT=$(cd "$(dirname "$0")/../../.." && pwd)
BIN_DIR=${BIN_DIR:-$ROOT/target/release}
[ -x "$BIN_DIR/cupertino-server" ] || BIN_DIR=$ROOT/target/debug
[ -x "$BIN_DIR/cupertino-server" ] || { echo "session.sh: no cupertino-server in target/release or target/debug" >&2; exit 1; }

if [ -n "${CUPERTINO_ASSET_DIR+set}" ]; then
    PACK=$CUPERTINO_ASSET_DIR
    [ -d "$PACK" ] || PACK=
else
    PACK=
    for dir in "${XDG_DATA_HOME:-$HOME/.local/share}/cupertino/assets" "$ROOT/data/assets/cupertino"; do
        [ -d "$dir" ] && PACK=${PACK:+$PACK:}$dir
    done
fi

SCRATCH=$(mktemp -d "${TMPDIR:-/tmp}/cupertino-capture.XXXXXX")
trap 'fusermount3 -u "$RUNTIME/doc" 2>/dev/null || true; rm -rf "$SCRATCH"' EXIT
if [ -n "${CUPERTINO_DOCK_FIXTURE_UNRESPONSIVE:-}" ]; then
    # a dock that marks the listed application ids unresponsive, built apart so no packaged binary can contain it
    (cd "$ROOT" && RUSTFLAGS="--cfg cupertino_capture_fixtures" cargo build -q --release -p cupertino-dock --features app \
        --target-dir "$ROOT/target/capture-fixtures")
    mkdir -p "$SCRATCH/bin"
    ln -s "$BIN_DIR"/* "$SCRATCH/bin/"
    ln -sf "$ROOT/target/capture-fixtures/release/cupertino-dock" "$SCRATCH/bin/cupertino-dock"
    BIN_DIR=$SCRATCH/bin
fi
FHOME=$SCRATCH/${HOME_NAME:-home}
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
mkdir -p "$FHOME/Desktop/Projects"
printf '# Ideas\n\n- A garden\n' >"$FHOME/Desktop/Ideas.md"
printf 'name,score\nada,9\n' >"$FHOME/Desktop/Scores.csv"
printf 'Sample download.\n' >"$FHOME/Downloads/Release Notes.txt"
: >"$FHOME/Pictures/Holiday.png"
: >"$FHOME/Music/Song.ogg"
[ -z "${FIXTURE_DIR:-}" ] || cp -rL "$FIXTURE_DIR"/. "$FHOME"/

# A stand-in `wpctl` first on PATH: one output and two inputs, the default sink and source at a fixed level and the chosen
# input kept in the scratch folder, so the volume the Control Center, the volume overlay and Settings show is the same on
# every run, an input can be chosen and no sound device is read.
AUDIO=$SCRATCH/audio
mkdir -p "$AUDIO/bin"
echo "0.50 0" >"$AUDIO/sink"
echo "0.50 0" >"$AUDIO/source"
echo 51 >"$AUDIO/default-source"
cat >"$AUDIO/bin/wpctl" <<'EOF'
#!/bin/sh
state=$(dirname "$0")/..
case "$1" in
status)
    read -r chosen <"$state/default-source"
    mark() { if [ "$1" = "$chosen" ]; then printf '*'; else printf ' '; fi; }
    printf 'Audio\n ├─ Sinks:\n │  *   55. Speakers [vol: 0.50]\n │\n ├─ Sources:\n'
    printf ' │  %s   51. Built-in Microphone [vol: 0.50]\n' "$(mark 51)"
    printf ' │  %s   57. Headset Microphone [vol: 0.50]\n │\n' "$(mark 57)"
    exit 0 ;;
set-default) echo "$2" >"$state/default-source"; exit 0 ;;
esac
case "$2" in
@DEFAULT_AUDIO_SINK@) node=$state/sink ;;
@DEFAULT_AUDIO_SOURCE@) node=$state/source ;;
*) echo "Translate ID error: '$2' is not a valid ID" >&2; exit 1 ;;
esac
read -r level muted <"$node"
case "$1" in
get-volume) if [ "$muted" = 1 ]; then printf 'Volume: %.2f [MUTED]\n' "$level"; else printf 'Volume: %.2f\n' "$level"; fi ;;
set-volume) echo "$3 $muted" >"$node" ;;
set-mute) echo "$level $3" >"$node" ;;
*) exit 1 ;;
esac
EOF
chmod +x "$AUDIO/bin/wpctl"
# the shell's prompt and the terminal title it sets name neither the user nor the machine, so a terminal looks the same
# on every machine
cat >"$FHOME/.bashrc" <<'EOF'
PS1='\W \$ '
unset PROMPT_COMMAND
EOF
touch -d 2026-01-01T12:00:00 "$FHOME" "$FHOME"/* "$FHOME"/*/* "$FHOME"/*/*/* 2>/dev/null || true

python3 -I - "$ROOT/packaging/files/etc/cupertino/compositor.json" "$SCRATCH/server.json" "$FHOME" "${CONFIG_JSON:-{\}}" <<'EOF'
import json, sys
config = json.load(open(sys.argv[1]))
config["shell"] = [dict(p, command=[c.removeprefix("/usr/bin/") if i == 0 else c for i, c in enumerate(p["command"])]) for p in config["shell"] if p["name"] != "login"]
config.update(json.loads(sys.argv[4].replace("@HOME@", sys.argv[3])))
json.dump(config, open(sys.argv[2], "w"))
EOF

cd "$FHOME"
env -i \
    HOME="$FHOME" \
    PATH="$AUDIO/bin:$BIN_DIR:/usr/local/bin:/usr/bin:/bin" \
    LANG=C.UTF-8 \
    TZ=UTC \
    XDG_CONFIG_HOME="$FHOME/.config" \
    XDG_DATA_HOME="$FHOME/.local/share" \
    XDG_STATE_HOME="$FHOME/.local/state" \
    XDG_CACHE_HOME="$FHOME/.cache" \
    XDG_RUNTIME_DIR="$RUNTIME" \
    XDG_CURRENT_DESKTOP=Cupertino \
    XDG_DATA_DIRS="$ROOT/crates/cupertino-files/data:$ROOT/crates/cupertino-settings/data:${SYSTEM_DATA_DIRS:-/usr/local/share:/usr/share}" \
    CUPERTINO_ASSET_SEARCH_PATH="$PACK" \
    CUPERTINO_SERVER_CONFIG="$SCRATCH/server.json" \
    ${CUPERTINO_DOCK_FIXTURE_UNRESPONSIVE:+CUPERTINO_DOCK_FIXTURE_UNRESPONSIVE="$CUPERTINO_DOCK_FIXTURE_UNRESPONSIVE"} \
    ${CUPERTINO_ACCESSIBILITY_CONF:+CUPERTINO_ACCESSIBILITY_CONF="$CUPERTINO_ACCESSIBILITY_CONF"} \
    DBUS_SYSTEM_BUS_ADDRESS="unix:path=$SCRATCH/no-system-bus" \
    dbus-run-session -- "$BIN_DIR/cupertino-server" --headless --dbus --size "${SIZE:-1280x800}" \
    --dev-shell "$@"
