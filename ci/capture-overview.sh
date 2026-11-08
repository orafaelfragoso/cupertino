#!/usr/bin/env bash
# `ci/capture.sh overview/all`: the overview of N test windows, drawn by the server headless (session.sh) and written as
# PNGs under the names of `cupertino-capture` (overview-all-windows-<N>-<light|dark>[-hc][-rt]@<scale>x.png).
# Flags: --windows N (1 to 6, default 2), --scale N (default 1), --appearance light|dark|both (default both),
# --increase-contrast, --reduce-transparency, --size WxH (points, default 1280x800), --out DIR (ci/capture.sh passes its default).
# Builds the release binaries of the workspace unless BIN_DIR names them.
set -euo pipefail

usage() {
  echo "usage: ci/capture.sh overview/all [--windows N] [--scale N] [--appearance light|dark|both]" \
    "[--increase-contrast] [--reduce-transparency] [--size WxH] [--out DIR]" >&2
  exit 2
}
fail() { echo "capture: $1" >&2; exit 2; }

[ "${1:-}" = overview/all ] || { [ "${1:-}" = overview ] && { echo overview/all; exit 0; }; usage; }
shift
windows=2 scale=1 appearances="light dark" contrast="" transparency="" size=1280x800 out=target/captures
while [ $# -gt 0 ]; do
  [ $# -ge 2 ] || case "$1" in --increase-contrast|--reduce-transparency) ;; *) fail "$1 needs a value" ;; esac
  case "$1" in
    --windows) windows=$2; shift ;;
    --scale) scale=$2; shift ;;
    --appearance)
      case "$2" in light|dark) appearances=$2 ;; both) appearances="light dark" ;;
        *) fail "--appearance $2: light, dark or both" ;; esac
      shift ;;
    --increase-contrast) contrast=-hc ;;
    --reduce-transparency) transparency=-rt ;;
    --size) size=$2; shift ;;
    --out) out=$2; shift ;;
    *) fail "overview has no flag $1" ;;
  esac
  shift
done
[[ $windows =~ ^[1-6]$ ]] || fail "--windows $windows: 1 to 6"
[[ $scale =~ ^[0-9]+(\.[0-9]+)?$ ]] && [ "$scale" != 0 ] || fail "--scale $scale: not a positive number"
[[ $size =~ ^([0-9]+)x([0-9]+)$ ]] || fail "--size $size: WIDTHxHEIGHT in points"
pixels=$(python3 -I -c 'import sys; w, h, s = map(float, sys.argv[1:]); print(f"{round(w * s)}x{round(h * s)}")' \
  "${BASH_REMATCH[1]}" "${BASH_REMATCH[2]}" "$scale")
# the pointer rests on the bottom-right corner, on no window or bar (no hot corner acts by default), as in capture.sh
park="$((BASH_REMATCH[1] - 1)),$((BASH_REMATCH[2] - 1))"

root="$(cd "$(dirname "$0")/.." && pwd)"
if [ -z "${BIN_DIR:-}" ]; then
  cargo build -q --release --manifest-path "$root/Cargo.toml" --workspace --all-features --bins
  target=$(cargo metadata -q --format-version 1 --no-deps --manifest-path "$root/Cargo.toml" |
    python3 -I -c 'import json, sys; print(json.load(sys.stdin)["target_directory"])')
  BIN_DIR=$target/release
fi
[[ $BIN_DIR != *[[:space:]]* ]] || fail "BIN_DIR $BIN_DIR: the server splits --spawn on whitespace, so the path cannot hold any"
[ -x "$BIN_DIR/cupertino-testclient" ] || fail "no cupertino-testclient in $BIN_DIR"
mkdir -p "$out"
out=$(realpath "$out")
scratch=$(mktemp -d "${TMPDIR:-/tmp}/cupertino-overview.XXXXXX")
trap 'rm -rf "$scratch"' EXIT
conf="$scratch/accessibility.conf"
: >"$conf"
[ -z "$contrast" ] || echo "increase_contrast = on" >>"$conf"
[ -z "$transparency" ] || echo "reduce_transparency = on" >>"$conf"

names=(One Two Three Four Five Six)
spawn=()
for name in "${names[@]:0:$windows}"; do
  spawn+=(--spawn "$BIN_DIR/cupertino-testclient --app-id org.cupertino.examples.$name --title $name --output /dev/null")
done
for appearance in $appearances; do
  png="$out/overview-all-windows-$windows-$appearance$contrast$transparency@${scale}x.png"
  BIN_DIR=$BIN_DIR SIZE=$pixels CUPERTINO_ACCESSIBILITY_CONF="$conf" CONFIG_JSON="{\"appearance\": \"$appearance\"}" \
    "$root/crates/cupertino-server/scripts/session.sh" --scale "$scale" "${spawn[@]}" \
    --pointer "0:$park" --action 3:overview --shot "5:$png" --run-for 5.5 >"$scratch/server.log" 2>&1 \
    || { tail -20 "$scratch/server.log" >&2; exit 1; }
  [ -s "$png" ] || { tail -20 "$scratch/server.log" >&2; fail "the server wrote no $png"; }
  echo "$png"
done
