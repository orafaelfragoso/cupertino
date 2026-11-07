#!/usr/bin/env bash
# Draws one surface of Files, the toolkit or the overview to PNGs for look checks and merge screenshots.
# Files and the toolkit: the app's own painter on sample data, without a display, a session bus or the server;
# each app's `<app>_capture` example draws, with the flags of `cupertino-capture` (crates/cupertino-capture/README.md).
# The overview: the server headless with test windows (ci/capture-overview.sh).
# `ci/capture.sh files/sidebar --devices removable --scale 2`; `ci/capture.sh files` lists the surfaces.
# Needs an asset pack (CUPERTINO_ASSET_DIR, else the user or system pack).
set -euo pipefail

app="${1:-}"
app="${app%%/*}"
root="$(cd "$(dirname "$0")/.." && pwd)"
out="${CARGO_TARGET_DIR:-$root/target}/captures"
case "$app" in
  files|kit) ;;
  overview) exec "$root/ci/capture-overview.sh" "$1" --out "$out" "${@:2}" ;;
  *) echo "usage: ci/capture.sh files|kit|overview[/SURFACE] [flags]" >&2; exit 2 ;;
esac
exec cargo run -q --release --manifest-path "$root/Cargo.toml" -p "cupertino-$app" --example "${app}_capture" -- \
  "$1" --out "$out" "${@:2}"
