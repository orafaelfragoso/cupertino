#!/bin/bash
# Headless screenshot of the full desktop in session.sh's fixture home (see there for the environment it reads).
#
# Usage: capture.sh OUT.png [server flags...]
#   capture.sh files.png --spawn cupertino-files
#   AFTER=6 capture.sh switcher.png --spawn cupertino-files --spawn cupertino-settings --hold-modifiers --key 5:Super+Tab
# AFTER: seconds before the shot, default 5.
set -eu

[ $# -ge 1 ] || { sed -n '4,6p' "$0" >&2; exit 2; }
OUT=$(realpath -m "$1")
shift
mkdir -p "$(dirname "$OUT")"
rm -f "$OUT"
"$(dirname "$0")/session.sh" --screenshot "$OUT" --screenshot-after "${AFTER:-5}" --exit-after-screenshot "$@"
[ -s "$OUT" ] || { echo "capture.sh: no screenshot written" >&2; exit 1; }
echo "$OUT"
