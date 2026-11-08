#!/bin/bash
# Captures the overlay screenshots in output/ (headless, deterministic fixtures):
#   launcher-open.png (through the bus with gdbus), notification-banners.png (through the bus with gdbus),
#   notification-centre.png, input-switcher.png, accent-picker.png
# Needs: a built compositor, cupertino-dock and cupertino-launcher (built with --features app) and
# cupertino-notifications (target/release or target/debug), foot, dbus-daemon, gdbus.
set -eu
HERE=$(cd "$(dirname "$0")/.." && pwd)
ROOT=$(cd "$HERE/../.." && pwd)
BIN=${BIN:-$ROOT/target/release/cupertino-server}
[ -x "$BIN" ] || BIN=$ROOT/target/debug/cupertino-server
DOCK_BIN=${DOCK_BIN:-$(dirname "$BIN")/cupertino-dock}
LAUNCHER_BIN=${LAUNCHER_BIN:-$(dirname "$BIN")/cupertino-launcher}
NOTIFY_BIN=${NOTIFY_BIN:-$(dirname "$BIN")/cupertino-notifications}
OUT=${OUT:-$HERE/output}
FIX=$HERE/tests/fixtures
SCRATCH=$(mktemp -d "${TMPDIR:-/tmp}/cupertino-shots.XXXXXX")
trap 'kill $(jobs -p) 2>/dev/null || true; rm -rf "$SCRATCH"' EXIT
mkdir -p "$OUT"

# applications come from the fixture directory only (neutral names); icons from the host theme
mkdir -p "$SCRATCH/data" "$SCRATCH/home"
ln -s /usr/share/icons "$SCRATCH/data/icons"
mkdir -p "$SCRATCH/home"
ln -s "$FIX/applications" "$SCRATCH/home/applications"
export XDG_DATA_DIRS="$SCRATCH/data" XDG_DATA_HOME="$SCRATCH/home"
export CUPERTINO_ICON_THEME=${CUPERTINO_ICON_THEME:-Yaru}
export CUPERTINO_SHORTCUTS=/dev/null
# the dock process pins foot only
echo foot > "$SCRATCH/dock.conf"
export CUPERTINO_DOCK_CONFIG="$SCRATCH/dock.conf"
# every capture runs the dock process; the others add theirs to this entry
DOCK_ENTRY=$(printf '{"name": "dock", "command": ["%s"]}' "$DOCK_BIN")
printf '{"shell": [%s]}' "$DOCK_ENTRY" > "$SCRATCH/dock.json"
printf 'input_sources = us, de, fr(nodeadkeys)\naccent_table = %s/accents.json\n' "$FIX" > "$SCRATCH/keyboard.conf"
export CUPERTINO_KEYBOARD_CONFIG="$SCRATCH/keyboard.conf"
COMMON=(--headless --size 1280x800)

# the launcher and notification processes talk to the server and the apps over a private session bus
dbus-daemon --session --nofork --print-address=3 3>"$SCRATCH/bus.addr" >/dev/null 2>&1 &
sleep 0.5
export DBUS_SESSION_BUS_ADDRESS=$(cat "$SCRATCH/bus.addr")

echo "launcher-open.png (the shortcut, through the bus)"
# the panel comes from the launcher process, started and supervised by the server; the launcher answers only the
# server, so the shortcut opens it rather than a call from this script
printf '{"shell": [%s, {"name": "launcher", "command": ["%s", "--no-files"]}]}' "$DOCK_ENTRY" "$LAUNCHER_BIN" > "$SCRATCH/launcher.json"
CUPERTINO_SERVER_CONFIG="$SCRATCH/launcher.json" "$BIN" "${COMMON[@]}" --dbus --key 2.5:Super+space --type 3.4:te \
    --screenshot "$OUT/launcher-open.png" --screenshot-after 4.5 --exit-after-screenshot >/dev/null 2>&1

echo "input-switcher.png"
CUPERTINO_SERVER_CONFIG="$SCRATCH/dock.json" CUPERTINO_NO_DBUS=1 "$BIN" "${COMMON[@]}" --hold-modifiers --key 1:Ctrl+space --key 1.4:Ctrl+space \
    --screenshot "$OUT/input-switcher.png" --screenshot-after 2 --exit-after-screenshot >/dev/null 2>&1

echo "accent-picker.png"
CUPERTINO_SERVER_CONFIG="$SCRATCH/dock.json" CUPERTINO_NO_DBUS=1 "$BIN" "${COMMON[@]}" --spawn "foot -T Notes -e sh" --accent 2.5:e --key 3:Tab --key 3.2:Right \
    --screenshot "$OUT/accent-picker.png" --screenshot-after 4 --exit-after-screenshot >/dev/null 2>&1

echo "notification-banners.png / notification-centre.png (through the session bus)"
# the banners come from the notification process, started and supervised by the server
printf '{"shell": [%s, {"name": "notifications", "command": ["%s"]}]}' "$DOCK_ENTRY" "$NOTIFY_BIN" > "$SCRATCH/server.json"
notify() {
    gdbus call --session --dest org.freedesktop.Notifications --object-path /org/freedesktop/Notifications \
        --method org.freedesktop.Notifications.Notify "$1" 0 "$2" "$3" "$4" "[]" "{}" 5000 >/dev/null
}
CUPERTINO_SERVER_CONFIG="$SCRATCH/server.json" "$BIN" "${COMMON[@]}" --dbus \
    --shot "5:$OUT/notification-banners.png" \
    --action 14:notification-center --screenshot "$OUT/notification-centre.png" --screenshot-after 14.6 --exit-after-screenshot >/dev/null 2>&1 &
COMP=$!
sleep 2.5
notify "Calendar" appointment-soon "Design review in 10 minutes" "Room 4B, bring the updated spec sheets."
sleep 0.3
notify "Mail" mail-unread "New message from Alex" "Are we still on for lunch at noon? I booked the table by the window."
sleep 0.3
notify "System" dialog-warning "Battery low" "12% remaining. Connect the charger."
wait $COMP
echo "done: $OUT"
