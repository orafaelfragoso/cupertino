#!/usr/bin/env bash
# Draws one surface of Settings, Files or the toolkit to PNGs for look checks and merge screenshots: the app's own
# painter on sample data, without a display, a session bus or the server. Each app's `<app>_capture` example draws;
# the flags are `cupertino-capture`'s (crates/cupertino-capture/README.md).
# `ci/capture.sh settings/accessibility --login saving --scale 2`; `ci/capture.sh settings` lists the surfaces.
# Needs an asset pack (CUPERTINO_ASSET_DIR, else the user or system pack).
set -euo pipefail

app="${1:-}"
app="${app%%/*}"
case "$app" in
  settings|files|kit) ;;
  *) echo "usage: ci/capture.sh settings|files|kit[/SURFACE] [flags]" >&2; exit 2 ;;
esac
root="$(cd "$(dirname "$0")/.." && pwd)"
exec cargo run -q --release --manifest-path "$root/Cargo.toml" -p "cupertino-$app" --example "${app}_capture" -- "$@"
