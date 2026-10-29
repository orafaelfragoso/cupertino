#!/bin/sh
# Start the full Cupertino desktop (server, shell processes, Files and Settings) from a text console, straight from
# this checkout. No system or service changes; the session manager and login screen are not used.
#
# Usage, on a free console (Ctrl+Alt+F3):  ./run-cupertino.sh [server flags]
#   ./run-cupertino.sh --run-for 600   stop by itself after 10 minutes
# Ctrl+Alt+Backspace quits; Ctrl+Alt+F1..F12 switch console. The server's log is ~/.local/state/cupertino/server.log (the
# previous run's is server.log.1); what the processes print goes to ~/cupertino-tty.log. It stops after
# $CUPERTINO_RUN_FOR seconds (default 900), and an outside watchdog (`timeout`) ends the whole process tree 30 s later if the
# server (and what it started) hangs instead of stopping. No limit (0) needs CUPERTINO_ALLOW_NO_TIMEOUT=1.
set -eu

root=$(cd "$(dirname "$0")" && pwd)

if [ -n "${WAYLAND_DISPLAY:-}${DISPLAY:-}" ]; then
    echo "run-cupertino.sh: run this from a text console (Ctrl+Alt+F3), not inside a desktop session" >&2
    exit 1
fi

: "${XDG_RUNTIME_DIR:?XDG_RUNTIME_DIR is not set; log in on the console first}"
log="$HOME/cupertino-tty.log"

run_for=${CUPERTINO_RUN_FOR:-900}
case $run_for in
'' | *[!0-9]*)
    echo "run-cupertino.sh: CUPERTINO_RUN_FOR must be a whole number of seconds (0 for no limit), got '$run_for'" >&2
    exit 1
    ;;
esac
# The limit that applies is the one in the arguments when there is one.
limit=$run_for
has_run_for=
want_value=
for arg in "$@"; do
    if [ -n "$want_value" ]; then
        limit=$arg
        want_value=
        continue
    fi
    case $arg in
    --run-for)
        has_run_for=1
        want_value=1
        ;;
    --run-for=*)
        has_run_for=1
        limit=${arg#--run-for=}
        ;;
    esac
done
case $limit in
'' | *[!0-9.]* | *.*.* | .)
    echo "run-cupertino.sh: --run-for must be a number of seconds, got '$limit'" >&2
    exit 1
    ;;
esac
if [ -z "$has_run_for" ] && [ "$run_for" -gt 0 ]; then
    set -- --run-for "$run_for" "$@"
fi
# Without a limit a hung server leaves the console unusable (logind mutes the kernel keyboard), so it takes a decision.
case $limit in
*[1-9]*)
    seconds=${limit%%.*}
    while [ "${#seconds}" -gt 1 ] && [ "${seconds#0}" != "$seconds" ]; do
        seconds=${seconds#0}
    done
    watchdog="timeout -k 10 $((${seconds:-0} + 30))"
    ;;
*)
    if [ "${CUPERTINO_ALLOW_NO_TIMEOUT:-}" != 1 ]; then
        echo "run-cupertino.sh: no time limit on a real console can leave it unusable if the server hangs; set CUPERTINO_ALLOW_NO_TIMEOUT=1 to accept that" >&2
        exit 1
    fi
    watchdog=
    ;;
esac

cargo build --release --workspace --all-features --bins --manifest-path "$root/Cargo.toml"

# The binaries are the ones just built, wherever cargo puts them (CARGO_TARGET_DIR, or the checkout's own target).
target_dir=${CARGO_TARGET_DIR:-$(cargo metadata --no-deps --format-version 1 --manifest-path "$root/Cargo.toml" | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')}
bin_dir="$target_dir/release"
if [ -z "$target_dir" ] || [ ! -x "$bin_dir/cupertino-server" ]; then
    echo "run-cupertino.sh: no cupertino-server in '$bin_dir'" >&2
    exit 1
fi
export PATH="$bin_dir:$PATH"
echo "Using $bin_dir/cupertino-server, built $(date -r "$bin_dir/cupertino-server" '+%Y-%m-%d %H:%M:%S')"
cupertino-server --check-hardware || {
    echo "run-cupertino.sh: fix the problems above, then run this again" >&2
    exit 1
}
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
# shellcheck disable=SC2086 # $watchdog is a command and its options, or nothing
dbus-run-session -- $watchdog cupertino-server --udev --dev-shell \
    --spawn cupertino-files --spawn cupertino-settings "$@" >"$log" 2>&1 || status=$?
if [ "$status" -eq 124 ] || [ "$status" -eq 137 ]; then
    echo "Cupertino did not stop in time and was ended by the watchdog (status $status)" >&2
fi
if [ "$status" -ne 0 ]; then
    echo "Cupertino exited with status $status; last lines of $log:" >&2
    tail -n 20 "$log" >&2
fi
exit "$status"
