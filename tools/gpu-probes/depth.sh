#!/bin/sh
# The 10-bit gradient probe in one command: an 8-bit and a 10-bit gradient on the primary plane, the driver's dithering
# off where it can be set. It needs the display to itself, so it runs from a text console with the graphical session
# stopped:
#   1. Log out of the desktop, switch to a text console (Ctrl+Alt+F3) and log in there.
#   2. From the checkout: tools/gpu-probes/depth.sh            (PROBE_SECONDS=30 shows each picture longer; default 15)
#   3. Watch the screen. Three pictures, one after the other: the bands in an 8-bit plane; the same in a 10-bit plane;
#      then one picture whose every band shows the 8-bit values in its upper half and the 10-bit values in its lower
#      half. The bands, top to bottom: grey over the darkest quarter, grey over the whole range, a dark blue, a warm
#      tone. The screen goes back to the console afterwards.
#   4. Note for each band whether the 10-bit half is smoother (fewer or no visible steps) than the 8-bit half, and add
#      that to the plan's measurements with target/gpu-probes/depth.jsonl, which records the plane formats, the link
#      depth and dithering the driver accepted.
# The probe is built here, outside the workspace, and is never part of the packages.
set -eu
cd "$(dirname "$0")/../.."
if pgrep -f 'cupertino-server --udev' >/dev/null 2>&1; then
    echo "depth.sh: the graphical session is running; log out and run this from a text console" >&2
    exit 1
fi
cargo build --release --offline --manifest-path tools/gpu-probes/Cargo.toml --target-dir target/gpu-probes
target/gpu-probes/release/gpu-probes depth --seconds "${PROBE_SECONDS:-15}" | tee target/gpu-probes/depth.jsonl
