#!/usr/bin/env bash
# Records the demo video: the full desktop of session.sh (headless, the sample home folder of ci/demo-video/home)
# driven by the scripted input of ci/demo_video.py, every frame of the output at 60 per second, encoded by ffmpeg.
# Nothing is composited or edited afterwards: each video frame is a frame the server drew, and a frame that came due
# while the encoder was still busy repeats the previous one (the summary line says how many were drawn).
#
# Usage: ci/demo-video.sh OUT.mp4          record and encode
#        ci/demo-video.sh --frames DIR     the same run without recording: one PNG per shot, for a look check
# Environment: BIN_DIR (the release binaries and examples/forms: cargo build --release --workspace --all-features
# --bins, cargo build --release -p cupertino-kit --example forms), SIZE (output pixels, default 2560x1440), SCALE
# (default 2), ENCODER (ffmpeg video encoder; default h264_nvenc when ffmpeg has it, else libx264),
# CUPERTINO_ASSET_DIR (the asset pack, as session.sh reads it).
# Needs: ffmpeg, python3, unshare (util-linux, unprivileged user namespaces), and what session.sh needs.
set -euo pipefail

usage() { sed -n '7,8p' "$0" >&2; exit 2; }
root="$(cd "$(dirname "$0")/.." && pwd)"
case "${1:-}" in
  --frames) [ $# -eq 2 ] || usage; frames="$(realpath -m "$2")" ;;
  ""|-*) usage ;;
  *) [ $# -eq 1 ] || usage; out="$(realpath -m "$1")" ;;
esac
: "${BIN_DIR:?set BIN_DIR to the release binaries}"
forms="$BIN_DIR/examples/forms"
[ -x "$forms" ] || { echo "demo-video.sh: no $forms" >&2; exit 1; }
size="${SIZE:-2560x1440}"

scratch="$(mktemp -d "${TMPDIR:-/tmp}/cupertino-demo.XXXXXX")"
trap 'kill "${encoding:-}" 2>/dev/null || true; rm -rf "$scratch"' EXIT
cp -r "$root/ci/demo-video/home" "$scratch/home"
printf 'Super+k = exec:%s\n' "$forms" >"$scratch/home/.config/cupertino/shortcuts.conf"
# the system's file types and icon themes, but not its applications: the launcher shows only Cupertino's
mkdir -p "$scratch/share"
for dir in mime icons; do [ -d "/usr/share/$dir" ] && ln -s "/usr/share/$dir" "$scratch/share/$dir"; done

# the session runs in namespaces of its own where the machine is named "cupertino" (the host name, /etc/hostname, and
# an empty /etc/machine-info), so About This Computer and the Files sidebar do not show the recording machine's name
echo cupertino >"$scratch/hostname"
: >"$scratch/machine-info"
session() {
  FIXTURE_DIR="$scratch/home" HOME_NAME=avery SYSTEM_DATA_DIRS="$scratch/share" SIZE="$size" NAMES="$scratch" \
    unshare --map-root-user --uts --mount sh -c 'hostname cupertino &&
      mount --bind "$NAMES/hostname" /etc/hostname &&
      { [ ! -e /etc/machine-info ] || mount --bind "$NAMES/machine-info" /etc/machine-info; } &&
      exec "$0" "$@"' \
    "$root/crates/cupertino-server/scripts/session.sh" --scale "${SCALE:-2}" "$@"
}

if [ -n "${frames:-}" ]; then
  mkdir -p "$frames"
  mapfile -t script < <(python3 -I "$root/ci/demo_video.py" --frames "$frames")
  session "${script[@]}" >"$scratch/server.log" 2>&1 || { tail -20 "$scratch/server.log" >&2; exit 1; }
  ls "$frames"
  exit 0
fi

if [ -z "${ENCODER:-}" ] && ffmpeg -hide_banner -encoders 2>/dev/null | grep -qw h264_nvenc; then
  ENCODER=h264_nvenc
fi
case "${ENCODER:-libx264}" in
  h264_nvenc) codec=(-c:v h264_nvenc -preset p4 -rc vbr -cq 19 -b:v 0) ;;
  *) codec=(-c:v "${ENCODER:-libx264}" -preset veryfast -crf 18 -pix_fmt yuv420p) ;;
esac
mapfile -t script < <(python3 -I "$root/ci/demo_video.py")
mkfifo "$scratch/frames"
mkdir -p "$(dirname "$out")"
ffmpeg -hide_banner -loglevel warning -y \
  -f rawvideo -pixel_format rgba -video_size "$size" -framerate 60 -i "$scratch/frames" \
  "${codec[@]}" -movflags +faststart "$out" &
encoding=$!
session --record "$scratch/frames" "${script[@]}" >"$scratch/server.log" 2>&1 \
  || { tail -20 "$scratch/server.log" >&2; exit 1; }
wait "$encoding"
encoding=
grep -o 'recording written.*' "$scratch/server.log" | sed 's/\x1b\[[0-9;]*m//g' >&2
echo "$out"
