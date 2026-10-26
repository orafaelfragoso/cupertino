#!/bin/sh
# Start the full Cupertino desktop (server, shell processes, Files and Settings) from a text console, straight from
# this checkout. No system or service changes; the session manager and login screen are not used.
#
# Usage, on a free console (Ctrl+Alt+F3):  ./run-cupertino.sh [server flags]
#   ./run-cupertino.sh --run-for 600   stop by itself after 10 minutes
# Ctrl+Alt+Backspace quits. The log is written to ~/cupertino-tty.log.
set -eu

root=$(cd "$(dirname "$0")" && pwd)

if [ -n "${WAYLAND_DISPLAY:-}${DISPLAY:-}" ]; then
    echo "run-cupertino.sh: run this from a text console (Ctrl+Alt+F3), not inside a desktop session" >&2
    exit 1
fi

cargo build --release --workspace --all-features --bins --manifest-path "$root/Cargo.toml"

config="${XDG_RUNTIME_DIR:?XDG_RUNTIME_DIR is not set; log in on the console first}/cupertino-compositor.json"
cat >"$config" <<'EOF'
{"shell": [
  {"name": "dock",          "command": ["cupertino-dock"]},
  {"name": "menubar",       "command": ["cupertino-menubar"]},
  {"name": "launcher",      "command": ["cupertino-launcher"]},
  {"name": "notifications", "command": ["cupertino-notifications"]},
  {"name": "controlcenter", "command": ["cupertino-controlcenter"]},
  {"name": "osd",           "command": ["cupertino-osd"]},
  {"name": "windowmanager", "command": ["cupertino-windowmanager"]}
]}
EOF

export PATH="$root/target/release:$PATH"
export XDG_CURRENT_DESKTOP=Cupertino
export XDG_DATA_DIRS="$root/crates/cupertino-files/data:$root/crates/cupertino-settings/data:${XDG_DATA_DIRS:-/usr/local/share:/usr/share}"
export CUPERTINO_SERVER_CONFIG="$config"

# A fresh session bus: the notification centre needs org.freedesktop.Notifications to itself.
echo "Starting Cupertino; log: $HOME/cupertino-tty.log"
exec dbus-run-session -- cupertino-server --udev \
    --spawn cupertino-files --spawn cupertino-settings "$@" >"$HOME/cupertino-tty.log" 2>&1
