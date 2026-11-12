#!/usr/bin/env bash
# Runs the headless golden tests in the pinned image, the only renderer their baselines are valid on.
# `ci/goldens.sh` compares; `ci/goldens.sh --update` re-records tests/fixtures/goldens (review the diff).
# Needs docker. CUPERTINO_GOLDENS_CACHE defaults to target/package-build/goldens in this checkout.
# Downloads are shared; compiled targets are separated by image content identity.
# CUPERTINO_CONTAINER_CACHE (default ~/.cache/cupertino-build/containers; empty, or on GitHub Actions, off) adds a
# per-user sccache and Cargo download directory shared by every checkout. sccache reuses a compiler output only when
# its compiler, arguments, environment and source files hash the same; the container paths are the same in every
# checkout. The Cargo target itself stays in this checkout.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"

update=0
if [ "${1:-}" = "--update" ]; then
  update=1
elif [ "$#" -ne 0 ]; then
  echo "usage: ci/goldens.sh [--update]" >&2
  exit 2
fi

cd "$root"
cache="${CUPERTINO_GOLDENS_CACHE:-$root/target/package-build/goldens}"
downloads="$cache/cargo"
compile_cache=()
if [ -z "${CUPERTINO_CONTAINER_CACHE+set}" ] && [ "${GITHUB_ACTIONS:-}" != true ]; then
  container_cache="$HOME/.cache/cupertino-build/containers"
else
  container_cache=${CUPERTINO_CONTAINER_CACHE:-}
fi
if [ -n "$container_cache" ]; then
  mkdir -p "$container_cache/goldens/sccache" "$container_cache/goldens/cargo"
  container_cache=$(cd "$container_cache" && pwd)
  downloads="$container_cache/goldens/cargo"
  compile_cache=(--volume "$container_cache/goldens/sccache:/cache/sccache"
    --env RUSTC_WRAPPER=/usr/bin/sccache --env SCCACHE_DIR=/cache/sccache
    --env "SCCACHE_CACHE_SIZE=${CUPERTINO_GOLDENS_SCCACHE_SIZE:-6G}" --env SCCACHE_IDLE_TIMEOUT=0)
fi
mkdir -p "$cache" "$downloads"
jobs=()
if [ -n "${CARGO_BUILD_JOBS:-}" ]; then
  jobs=(--env "CARGO_BUILD_JOBS=$CARGO_BUILD_JOBS")
fi

# The session bus needs the caller's uid in the password database. The name is fixed: the lock screen draws it, and the
# baseline must not depend on who runs this.
# Docker writes the control files too; keep them on the disk checked by the build guard.
scratch="$(mktemp -d "$cache/control.XXXXXX")"
passwd="$scratch/passwd"
log="$scratch/log"
cidfile="$scratch/container.cid"
cleanup() {
  cleanup_status=$?
  if [ -f "$cidfile" ]; then
    docker rm --force "$(cat "$cidfile")" >/dev/null
  fi
  rm -r "$scratch"
  return "$cleanup_status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
printf 'root:x:0:0:root:/root:/bin/sh\ngolden:x:%s:%s:Golden User:/tmp:/bin/sh\n' "$(id -u)" "$(id -g)" >"$passwd"

definition="$(sha256sum "$root/ci/goldens/Dockerfile" | cut -d ' ' -f 1)"
image="${CUPERTINO_GOLDENS_IMAGE:-cupertino-goldens:$definition}"
if [ -z "${CUPERTINO_GOLDENS_IMAGE:-}" ]; then
  docker build --quiet --tag "$image" --label "io.cupertino.goldens.definition=$definition" "$root/ci/goldens" >/dev/null
fi
actual="$(docker image inspect --format '{{ index .Config.Labels "io.cupertino.goldens.definition" }}' "$image")"
if [ "$actual" != "$definition" ]; then
  echo "goldens: image definition does not match ci/goldens/Dockerfile" >&2
  exit 1
fi
image_id=$(docker image inspect --format '{{.Id}}' "$image")
if [[ ! "$image_id" =~ ^sha256:[0-9a-f]{64}$ ]]; then
  echo "goldens: image has an invalid content identity: $image_id" >&2
  exit 1
fi
target="$cache/targets/${image_id#sha256:}"
mkdir -p "$target"
docker create --cidfile "$cidfile" \
  --user "$(id -u):$(id -g)" \
  --volume "$passwd:/etc/passwd:ro" --volume "$root:/work" \
  --volume "$downloads:/cache/cargo" --volume "$target:/cache/target" "${compile_cache[@]}" --workdir /work \
  --env HOME=/tmp --env CARGO_HOME=/cache/cargo --env CARGO_TARGET_DIR=/cache/target \
  --env CARGO_INCREMENTAL=0 --env CARGO_PROFILE_DEV_DEBUG=0 \
  "${jobs[@]}" \
  --env CUPERTINO_GOLDENS_REQUIRED=1 --env "CUPERTINO_GOLDEN_MUTATE_WINDOW=${CUPERTINO_GOLDEN_MUTATE_WINDOW:-}" \
  --env "CUPERTINO_UPDATE_GOLDENS=$update" \
  "$image_id" \
  python3 ci/build-test-fixtures.py --locked -- \
  cargo test --locked -p cupertino-server --test suite -- goldens:: --include-ignored --skip headless_frame_and_memory_report \
  >/dev/null
# Waiting on an asynchronous attachment lets TERM/INT run cleanup immediately, even if the client is unresponsive.
docker start --attach "$(cat "$cidfile")" | tee "$log" &
attachment=$!
wait "$attachment" || status=$?

# An update records the baselines and then fails each test that recorded, so it can never pass as a comparison; the
# log check tells those failures from any other and knows which tests must run (ci/goldens_log.py). The run must also
# end with cargo test's own failure status: a container killed after the log was complete exits otherwise.
if [ "$update" -eq 1 ]; then
  python3 -I "$root/ci/goldens_log.py" --update "$log"
  if [ "${status:-0}" -ne 101 ]; then
    echo "goldens: the update run exited ${status:-0}, not with cargo test's failure status 101" >&2
    exit 1
  fi
  echo "goldens: re-recorded; review the diff, then run ci/goldens.sh to compare" >&2
  exit 0
fi
python3 -I "$root/ci/goldens_log.py" "$log"
[ "${status:-0}" -eq 0 ] || exit "$status"
