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

# The session bus needs the caller's uid in the password database.
passwd="$(mktemp)"
trap 'rm -f "$passwd"' EXIT
printf 'root:x:0:0:root:/root:/bin/sh\n%s:x:%s:%s::/tmp:/bin/sh\n' "$(id -un)" "$(id -u)" "$(id -g)" >"$passwd"

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
  | tee "$log"

# A renamed or removed test must not turn into a job that runs nothing and passes.
for name in pixel_comparison_fails_on_a_change_at_unchanged_geometry desktop_goldens_and_input_replay shell_process_goldens; do
  grep -qx "test $name ... ok" "$log" || { echo "goldens: expected test $name did not run and pass" >&2; exit 1; }
done
