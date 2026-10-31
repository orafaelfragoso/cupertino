#!/bin/bash
set -uo pipefail
umask 077
logs="$HOME/.cache/cupertino-vm/logs"
mkdir -p "$logs"
child=
finish() {
    code=$?
    trap - EXIT HUP INT TERM
    if [[ -n "$child" ]] && kill -0 "$child" 2>/dev/null; then
        kill -TERM "$child"
        wait "$child" || true
    fi
    python3 - "$logs" "$code" <<'PY'
import json
from pathlib import Path
import sys
logs = Path(sys.argv[1])
result = {"state": "exited", "exit_status": int(sys.argv[2]),
          "stderr": (logs / "stderr.log").read_text()}
if (logs / "build.log").exists():
    result["mkosi_output"] = (logs / "build.log").read_text()
(logs / "status.json").write_text(json.dumps(result) + "\n")
PY
    cat "$logs/stderr.log" >&2
    if [[ "$code" != 0 && -f "$logs/build.log" ]]; then
        tail -30 "$logs/build.log" >&2
    fi
    exit "$code"
}
trap finish EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM
: >"$logs/stderr.log"
if [[ -f "$logs/build.log" ]]; then
    mv -- "$logs/build.log" "$logs/previous-build.log"
fi
: >"$logs/build.log"
printf '%s\n' '{"state":"waiting_for_gate","exit_status":null,"stderr":""}' >"$logs/status.json"
flock --no-fork "$HOME/OpenBot/Shared/cupertino/gate.lock" \
    timeout --signal=TERM --kill-after=15s 900s \
    python3 "$(dirname -- "$0")/build_image.py" "$@" 2>"$logs/stderr.log" &
child=$!
wait "$child"
exit $?
