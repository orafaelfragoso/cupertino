#!/bin/sh
# Start the full Cupertino desktop (server, shell processes, Files and Settings) from a text console, straight from
# this checkout. No system or service changes; the session manager and login screen are not used.
#
# Usage, on a free console (Ctrl+Alt+F3):  ./run-cupertino.sh [server flags]
#   ./run-cupertino.sh --run-for 600   stop by itself after 10 minutes
# Ctrl+Alt+Backspace quits; Ctrl+Alt+F1..F12 switch console. The server's log is ~/.local/state/cupertino/server.log (the
# previous run's is server.log.1); what the processes print goes to ~/cupertino-tty.log. It stops after
# $CUPERTINO_RUN_FOR seconds (default 900, 0 for no limit).
set -eu

root=$(cd "$(dirname "$0")" && pwd)

if [ -n "${WAYLAND_DISPLAY:-}${DISPLAY:-}" ]; then
    echo "run-cupertino.sh: run this from a text console (Ctrl+Alt+F3), not inside a desktop session" >&2
    exit 1
fi

: "${XDG_RUNTIME_DIR:?XDG_RUNTIME_DIR is not set; log in on the console first}"
log="$HOME/cupertino-tty.log"

cargo build --release --workspace --all-features --bins --manifest-path "$root/Cargo.toml"

export PATH="$root/target/release:$PATH"
cupertino-server --check-hardware || {
    echo "run-cupertino.sh: fix the problems above, then run this again" >&2
    exit 1
}
run_for=${CUPERTINO_RUN_FOR:-900}
case " $* " in
*" --run-for "*) ;;
*) [ "$run_for" -gt 0 ] && set -- --run-for "$run_for" "$@" ;;
esac
export XDG_CURRENT_DESKTOP=Cupertino
export XDG_DATA_DIRS="$root/crates/cupertino-files/data:$root/crates/cupertino-settings/data:${XDG_DATA_DIRS:-/usr/local/share:/usr/share}"
# The shell processes are the ones the packages install as /etc/cupertino/compositor.json; --dev-shell lets this checkout name
# programs found on PATH instead of under /usr.
CUPERTINO_SERVER_CONFIG=$(mktemp)
sed "s|\"/usr/bin/|\"|" "$root/packaging/files/etc/cupertino/compositor.json" >"$CUPERTINO_SERVER_CONFIG"
export CUPERTINO_SERVER_CONFIG
trap 'rm -f "$CUPERTINO_SERVER_CONFIG"' EXIT

# A fresh session bus: the notification centre needs org.freedesktop.Notifications to itself.
echo "Starting Cupertino; log: $log"
status=0
dbus-run-session -- cupertino-server --udev --dev-shell \
    --spawn cupertino-files --spawn cupertino-settings "$@" >"$log" 2>&1 || status=$?
if [ "$status" -ne 0 ]; then
    echo "Cupertino exited with status $status; last lines of $log:" >&2
    tail -n 20 "$log" >&2
fi
exit "$status"
