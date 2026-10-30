#!/usr/bin/env bash
# Runs the headless golden tests in the pinned image, the only renderer their baselines are valid on.
# `ci/goldens.sh` compares; `ci/goldens.sh --update` re-records tests/fixtures/goldens (review the diff).
# Needs docker. CUPERTINO_GOLDENS_CACHE (default ~/.cache/cupertino-goldens) holds the image's cargo home and target.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cache="${CUPERTINO_GOLDENS_CACHE:-$HOME/.cache/cupertino-goldens}"
mkdir -p "$cache/cargo" "$cache/target"

update=0
if [ "${1:-}" = "--update" ]; then
  update=1
elif [ "$#" -ne 0 ]; then
  echo "usage: ci/goldens.sh [--update]" >&2
  exit 2
fi

# The session bus needs the caller's uid in the password database. The name is fixed: the lock screen draws it, and the
# baseline must not depend on who runs this.
passwd="$(mktemp)"
trap 'rm -f "$passwd"' EXIT
printf 'root:x:0:0:root:/root:/bin/sh\ngolden:x:%s:%s:Golden User:/tmp:/bin/sh\n' "$(id -u)" "$(id -g)" >"$passwd"

docker build --quiet --tag cupertino-goldens "$root/ci/goldens" >/dev/null
log="$(mktemp)"
trap 'rm -f "$passwd" "$log"' EXIT
docker run --rm \
  --user "$(id -u):$(id -g)" \
  --volume "$passwd:/etc/passwd:ro" --volume "$root:/work" --volume "$cache:/cache" --workdir /work \
  --env HOME=/tmp --env CARGO_HOME=/cache/cargo --env CARGO_TARGET_DIR=/cache/target \
  --env CARGO_INCREMENTAL=0 --env CARGO_PROFILE_DEV_DEBUG=0 \
  --env CUPERTINO_GOLDENS_REQUIRED=1 --env "CUPERTINO_GOLDEN_MUTATE_WINDOW=${CUPERTINO_GOLDEN_MUTATE_WINDOW:-}" \
  --env "CUPERTINO_UPDATE_GOLDENS=$update" \
  cupertino-goldens \
  cargo test -p cupertino-server --test goldens -- --include-ignored --skip headless_frame_and_memory_report \
  | tee "$log" || status=$?

# An update records the baselines and then fails each test that recorded, so it can never pass as a comparison; the
# log check tells those failures from any other and knows which tests must run (ci/goldens_log.py).
if [ "$update" -eq 1 ]; then
  python3 -I "$root/ci/goldens_log.py" --update "$log"
  echo "goldens: re-recorded; review the diff, then run ci/goldens.sh to compare" >&2
  exit 0
fi
python3 -I "$root/ci/goldens_log.py" "$log"
[ "${status:-0}" -eq 0 ] || exit "$status"
