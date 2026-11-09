#!/usr/bin/env bash
# Build, inspect and install-test the Arch packages in pinned containers.
#
#   packaging/ci/build-in-container.sh arch DIST_DIR [--test-only|--upgrade]
#
# Arch is the only distribution the packages target; the first argument names it so callers read the same everywhere.
# The source is the committed HEAD (git archive), so untracked or modified files never reach a package. A build
# container makes the runtime and data packages with makepkg (check() included) on the release source archive and the
# stable PKGBUILD, in a Cargo target of its own that is removed afterwards. A second container runs namcap, and
# release.py verifies each package against the manifest. A fresh container of the plain base installs them, checks the
# login-screen account, the unit state, the payload and conffile preservation across a reinstall, then removes them.
# Packages, file lists and the test log land in DIST_DIR. --test-only skips the build and install-tests the packages
# already in DIST_DIR. Needs Docker Buildx, Python 3 and git; nothing is installed on the host.
#
# Build slots (ci/worktree-build.py): the script holds this worktree for its whole run, every slot only while makepkg
# compiles and tests, one slot while the builder image builds, and none for the source archive, namcap, verification
# and the install test. A caller that already holds every slot (`locked` around this script) keeps them throughout.
#
# Compile cache: CUPERTINO_CONTAINER_CACHE names a per-user directory holding sccache's cache and Cargo's downloads for
# this build (default ~/.cache/cupertino-build/containers; the empty string, or running on GitHub Actions, turns it
# off). sccache reuses a compiler output only when the compiler, its arguments, environment and every source file it
# read hash the same, and the in-container paths never change, so a warm package is the same package. The source is
# still the clean archive, every crate is still checked, linked and tested, and no Cargo target is ever reused: a
# makepkg source tree carries the commit time on every file, so Cargo's timestamp checks cannot tell an old target
# from a new source.
#
# --upgrade packages the same build twice, as release 1 in DIST_DIR/rev-1 and as release 2 in DIST_DIR/rev-2 (no
# second compile), then, in a fresh container, installs release 1, enables the login service, edits login.conf,
# upgrades to release 2 and checks that the version rose and that the account, conffile, unit state and payload survived.
set -euo pipefail

usage="usage: build-in-container.sh arch DIST_DIR [--test-only|--upgrade]"
distro=${1:?$usage}
dist=${2:?$usage}
mode=${3:-build}
upgrade=0
[ "$mode" = --upgrade ] && upgrade=1
root=$(git -C "$(dirname "$0")" rev-parse --show-toplevel)
if [ "$mode" != build ] && [ "$mode" != --test-only ] && [ "$mode" != --upgrade ]; then
    echo "$usage" >&2
    exit 2
fi
if [ "$distro" != arch ]; then
    echo "unknown distribution: $distro (only arch is packaged)" >&2
    exit 2
fi
# Fresh install tests use the original distribution base, never the dependency-filled builder image.
test_image=docker.io/library/archlinux@sha256:3cbf69d2d5958d6a294f3722c5668e6187c4df8e00fa27f72352f0bf83e495e1
mkdir -p "$dist"
dist=$(cd "$dist" && pwd)
# Every mode stores active container ids under target/, so cleanup must hold the same lifetime lease.
if [ -z "${CUPERTINO_BUILD_LOCK_FD:-}" ]; then
    cd "$root"
    exec python3 ci/worktree-build.py locked --hold none -- bash packaging/ci/build-in-container.sh "$distro" "$dist" "$mode"
fi
cd "$root"
# Run one phase holding `every` build slot or `one`; inside a `locked` that already holds every slot, run it as it is.
phase() {
    local hold=$1
    shift
    if [ "${CUPERTINO_BUILD_LOCK_HOLD:-}" = none ]; then
        python3 "$root/ci/worktree-build.py" locked --hold "$hold" -- "$@"
    else
        "$@"
    fi
}
if [ -z "${CUPERTINO_CONTAINER_CACHE+set}" ] && [ "${GITHUB_ACTIONS:-}" != true ]; then
    container_cache="$HOME/.cache/cupertino-build/containers"
else
    container_cache=${CUPERTINO_CONTAINER_CACHE:-}
fi
# The package build's share of that cache; sccache evicts the least recently used outputs beyond it.
sccache_size=${CUPERTINO_PACKAGE_SCCACHE_SIZE:-12G}
# The source copy and the Arch build tree are large; they live under the gitignored target/, not in /tmp.
mkdir -p "$root/target"
work=$(mktemp -d "$root/target/container-build.XXXXXX")
cache_next=
cache_link=
active_job=
restore_ownership=0
cache_mounts=()
owned_cache=()
cleanup() {
    local status=$? cleanup_status=0 container cidfile
    trap - EXIT
    trap "" INT TERM
    # Containers are created stopped and recorded before compilation can start. Only this run's ids are removed.
    for cidfile in "$work/build.cid" "$work/inspect.cid" "$work/test.cid"; do
        if [ -s "$cidfile" ]; then
            container=$(cat "$cidfile")
            if ! docker rm --force "$container" >/dev/null; then
                cleanup_status=1
            fi
        fi
    done
    if [ -n "$active_job" ]; then
        if kill -0 "$active_job" 2>/dev/null; then
            kill -TERM "$active_job" || cleanup_status=1
        fi
        # Reap the interrupted streaming/build client; its failure must not replace the original signal status.
        if wait "$active_job"; then :; fi
    fi
    # A forced container stop cannot run its EXIT trap; repair only this run's bind-mounted outputs.
    if [ "$restore_ownership" = 1 ]; then
        if ! docker run --rm --network none -v "$work/src:/src" -v "$dist:/dist" \
            --mount "type=bind,src=$cargo_home,dst=/cargo-home" "${cache_mounts[@]}" \
            "$image_id" chown -R "$(id -u):$(id -g)" /dist /src /cargo-home "${owned_cache[@]}"; then
            cleanup_status=1
        fi
    fi
    rm -rf "$work" || cleanup_status=1
    if [ -n "$cache_next" ]; then rm -rf "$cache_next" || cleanup_status=1; fi
    if [ -n "$cache_link" ]; then rm -f "$cache_link" || cleanup_status=1; fi
    if [ "$status" = 0 ]; then status=$cleanup_status; fi
    exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

wait_job() {
    local status
    if wait "$active_job"; then status=0; else status=$?; fi
    active_job=
    return "$status"
}

# Start a created container and wait for it; HOLD is the build slots it needs, or none.
start_container() {
    local container=$1 log=$2 hold=$3
    if [ -n "$log" ]; then
        docker start --attach "$container" | tee "$log" &
    elif [ "$hold" != none ]; then
        phase "$hold" docker start --attach "$container" &
    else
        docker start --attach "$container" &
    fi
    active_job=$!
    wait_job
}
source_commit=$(git -C "$root" rev-parse HEAD)
git -C "$root" archive --format=tar --prefix=src/ "$source_commit" | tar -x -C "$work"
if [ "$mode" != --test-only ]; then
    mkdir "$work/src/arch-build"
    archive=$(python3 "$root/packaging/release.py" source --commit "$source_commit" --output "$work/src/arch-build" ${RELEASE_TAG:+--tag "$RELEASE_TAG"})
    python3 "$root/packaging/release.py" pkgbuild --commit "$source_commit" ${RELEASE_TAG:+--tag "$RELEASE_TAG"} "$archive" "$work/src/arch-build/PKGBUILD" >/dev/null
    if [ -n "${CUPERTINO_PKGREL:-}" ]; then
        sed -i "s/^pkgrel=.*/pkgrel=$CUPERTINO_PKGREL/" "$work/src/arch-build/PKGBUILD"
    fi
fi

owner="$(id -u):$(id -g)"
# The compile container: everything that compiles or runs tests, under every build slot.
build_script=$(cat <<EOF
set -euo pipefail
# Stop sccache (printing its counters, also after a failure) before handing the cache back to the caller.
trap 'if [ -n "\${SCCACHE_DIR:-}" ]; then su builder -c "sccache --stop-server" || true; fi; chown -R $owner /dist /src /cargo-home\${SCCACHE_DIR:+ /cache}' EXIT
cd /src
# Packages are stripped; generating debug information only consumes build storage.
export CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/src/target CUPERTINO_THEME_DIR=/usr/share/cupertino/theme CARGO_PROFILE_RELEASE_DEBUG=0
if [ -d /cache/sccache ]; then
    export RUSTC_WRAPPER=/usr/bin/sccache SCCACHE_DIR=/cache/sccache SCCACHE_CACHE_SIZE=$sccache_size SCCACHE_IDLE_TIMEOUT=0
    chown -R builder /cache
fi
mkdir /src/target
chown -R builder /src/arch-build /src/target /cargo-home
su builder -c 'cd /src/arch-build && makepkg --noconfirm'
df -h /src | tee /dist/disk-after-makepkg.log
EOF
)

# The inspection container: namcap, verification and the second release's repackaging compile nothing.
inspect_script=$(cat <<EOF
set -euo pipefail
trap 'chown -R $owner /dist /src' EXIT
cd /src
export CARGO_TARGET_DIR=/src/target
upgrade=$upgrade
namcap arch-build/PKGBUILD arch-build/*.pkg.tar.zst | tee /dist/namcap.log
publish() {
    mkdir -p "\$1"
    cp arch-build/*.pkg.tar.zst arch-build/PKGBUILD "\$1/"
    python3 packaging/release.py verify --format arch --package runtime --bindir /src/target/release "\$1"/cupertino-0*.pkg.tar.zst
    python3 packaging/release.py verify --format arch --package data "\$1"/cupertino-data-*.pkg.tar.zst
    for p in "\$1"/*.pkg.tar.zst; do pacman -Qip "\$p" > "\$p.info"; pacman -Qlp "\$p" > "\$p.contents"; done
}
if [ "\$upgrade" = 1 ]; then
    publish /dist/rev-1
    rm arch-build/*.pkg.tar.zst
    sed -i 's/^pkgrel=1\$/pkgrel=2/' arch-build/PKGBUILD
    chown -R builder /src/arch-build
    su builder -c 'cd /src/arch-build && makepkg --noconfirm --repackage'
    publish /dist/rev-2
else
    publish /dist
fi
EOF
)

test_script=$(cat <<'EOF'
set -euo pipefail
check() { local label=$1; shift; "$@" || { echo "FAIL: $label"; exit 1; }; echo "$label"; }
absent() { for p in "$@"; do [ ! -e "$p" ] || return 1; done; }
pacman -Syu --noconfirm -q >/dev/null
pacman -U --noconfirm /dist/cupertino-data-*.pkg.tar.zst /dist/cupertino-0*.pkg.tar.zst >/dev/null
reinstall() { pacman -U --noconfirm /dist/cupertino-0*.pkg.tar.zst >/dev/null 2>&1; }
remove() { pacman -Rns --noconfirm cupertino cupertino-data >/dev/null; }
echo "installed: $(ls /usr/bin/cupertino-* | wc -l) files in /usr/bin"
getent passwd cupertino-greeter
for u in cupertino-login cupertino-netd cupertino-btd cupertino-powerd; do
    state=$(systemctl is-enabled "$u.service" 2>/dev/null || true)
    echo "$u.service: $state"
    [ "$state" = disabled ] || [ "$state" = static ]
done
for b in /usr/bin/cupertino /usr/bin/cupertino-*; do
    [ "$b" = /usr/bin/cupertino-session ] && continue
    if ldd "$b" | grep -q 'not found'; then ldd "$b"; exit 1; fi
done
echo 'all binaries resolve their libraries'
# the shell processes' test clock exists only in debug builds (cupertino-shell-client test_clock): a packaged binary that
# holds its variable's name was built with debug assertions
for b in /usr/bin/cupertino-login /usr/bin/cupertino-dock /usr/bin/cupertino-launcher; do
    if grep -q -a CUPERTINO_TEST_CLOCK "$b"; then echo "$b holds the test clock: a debug build was packaged"; exit 1; fi
done
echo 'no packaged shell process reads the test clock'
/usr/bin/cupertino-server --help >/dev/null
echo 'cupertino-server --help: ok'
# The cupertino command as installed: run from /, as a user without a session, with nothing of the build's environment.
cli_checks() {
    local user=cupertino-cli-test out recorded pkgver status
    pacman -S --noconfirm --needed -q man-db bash-completion jq >/dev/null
    useradd --no-create-home --shell /usr/bin/nologin "$user"
    install -d -o "$user" -g "$user" -m 700 /tmp/u /tmp/u/runtime
    as_user() { (cd / && exec setpriv --reuid="$user" --regid="$user" --clear-groups env -i PATH=/usr/bin HOME=/tmp/u "$@"); }
    out=$(as_user cupertino --version)
    check "cupertino --version: $out" grep -Eq '^cupertino [0-9]+\.[0-9]+\.[0-9]+ \([0-9a-f]{12}\)$' <<<"$out"
    as_user cupertino --format json version > /tmp/u/version.json
    for part in runtime-package:cupertino data-package:cupertino-data; do
        recorded=$(jq -er --arg n "${part%%:*}" '.data.components[] | select(.name == $n and .status == "ok") | .version' /tmp/u/version.json)
        pkgver=$(pacman -Q "${part#*:}" | cut -d' ' -f2)
        pkgver=${pkgver%-*}
        check "${part%%:*} $recorded is ${part#*:} $pkgver" test "$recorded" = "${pkgver%%rc*}"
    done
    check 'the runtime package records the commit cupertino was built from' \
        jq -e '[.data.components[] | select(.name == "cli" or .name == "runtime-package") | .commit] | .[0] == .[1] and (.[0] | test("^[0-9a-f]{40}$"))' /tmp/u/version.json
    as_user cupertino help > /tmp/u/help.txt
    check 'cupertino help lists desktop start' grep -q '^  desktop start' /tmp/u/help.txt
    status=0
    as_user cupertino --format jsonl diagnose > /tmp/u/diagnose.jsonl || status=$?
    check "cupertino --format jsonl diagnose: exit $status, end last" \
        jq -se --argjson s "$status" '$s == 0 and .[-1].type == "end" and .[-1].ok and ([.[] | select(.type == "end")] | length) == 1' /tmp/u/diagnose.jsonl
    # desktop start runs until stopped: started in the background, stopped with SIGTERM once the compositor is ready
    as_user XDG_RUNTIME_DIR=/tmp/u/runtime cupertino --format jsonl desktop start --backend headless --spawn true \
        > /tmp/u/desktop.jsonl 2> /tmp/u/desktop.err &
    local pid=$!
    # grep reads a process substitution, so the test does not wait for tail, which follows the file until the command ends
    if timeout 120 grep -m1 -q '"event":"ready"' <(tail -n +1 --pid="$pid" -f /tmp/u/desktop.jsonl); then
        kill -TERM "$pid"
        status=0
        wait "$pid" || status=$?
        check 'cupertino desktop start --backend headless: ready, stopped by SIGTERM, exit 0' \
            jq -se --argjson s "$status" '$s == 0 and .[-1].reason == "stopped" and (map(.event) | index("ready")) != null' /tmp/u/desktop.jsonl
        check 'the stopped compositor left no runtime file' sh -c '! ls /tmp/u/runtime/cupertino/server-*.json 2>/dev/null' 
    else
        kill -TERM "$pid" 2>/dev/null || true
        wait "$pid" || true
        cat /tmp/u/desktop.jsonl /tmp/u/desktop.err
        if grep -q 'no usable EGL device' /tmp/u/desktop.err; then
            echo 'BLOCKED B5: the container has no EGL device for the headless compositor; run `cupertino desktop start --backend headless --spawn true` on a machine with a GPU'
        else
            echo 'FAIL: cupertino desktop start --backend headless did not get ready'
            exit 1
        fi
    fi
    check "man -w cupertino: $(man -w cupertino)" test "$(man -w cupertino)" = /usr/share/man/man1/cupertino.1
    check 'man -l renders cupertino.1' sh -c 'man -l /usr/share/man/man1/cupertino.1 2>/dev/null | head -5 | grep -q cupertino'
    for page in /usr/share/man/man1/cupertino-*.1; do man -l "$page" >/dev/null 2>&1 || { echo "FAIL: man -l $page"; exit 1; }; done
    out=$(as_user bash -ic 'for f in _comp_load __load_completion _completion_loader; do
        declare -F "$f" >/dev/null && { "$f" cupertino; break; }; done; complete -p cupertino' 2>/dev/null)
    check "bash completes cupertino: $out" test "$out" = 'complete -F _cupertino cupertino'
    userdel "$user"
}
cli_checks
test -f /usr/share/cupertino/theme/tokens.json && test -f /usr/share/cupertino/assets/manifest.json
echo '# local edit' >> /etc/cupertino/login.conf
reinstall
check 'login.conf kept across reinstall' grep -q '^# local edit' /etc/cupertino/login.conf
remove
check 'removed cleanly' absent /usr/bin/cupertino-server /usr/share/cupertino/assets/manifest.json
EOF
)

upgrade_script=$(cat <<'EOF'
set -euo pipefail
check() { local label=$1; shift; "$@" || { echo "FAIL: $label"; exit 1; }; echo "$label"; }
absent() { for p in "$@"; do [ ! -e "$p" ] || return 1; done; }
pacman -Syu --noconfirm -q >/dev/null
install() { pacman -U --noconfirm "$1"/cupertino-data-*.pkg.tar.zst "$1"/cupertino-0*.pkg.tar.zst >/dev/null; }
version() { pacman -Q cupertino | cut -d' ' -f2; }
newer() { [ "$(vercmp "$1" "$2")" = 1 ]; }
files() { pacman -Qql cupertino cupertino-data; }
altered() { pacman -Qkk cupertino cupertino-data 2>&1 | grep '^warning' | grep -v '/etc/cupertino/login.conf' || true; }
remove() { pacman -Rns --noconfirm cupertino cupertino-data >/dev/null; }
units() { for u in login netd btd powerd; do echo "cupertino-$u.service: $(systemctl is-enabled "cupertino-$u.service" 2>/dev/null || true)"; done; }
install /dist/rev-1
old=$(version)
echo "installed: $old"
systemctl enable cupertino-login.service >/dev/null 2>&1
[ "$(systemctl is-enabled cupertino-login.service)" = enabled ]
echo '# local edit' >> /etc/cupertino/login.conf
conf=$(sha256sum /etc/cupertino/login.conf)
account=$(getent passwd cupertino-greeter)
units_before=$(units)
sha256sum /usr/bin/cupertino-server > /tmp/server.before
install /dist/rev-2
new=$(version)
echo "upgraded: $old -> $new"
newer "$new" "$old"
check 'login.conf kept across upgrade' [ "$(sha256sum /etc/cupertino/login.conf)" = "$conf" ]
check 'cupertino-greeter account unchanged' [ "$(getent passwd cupertino-greeter)" = "$account" ]
units_after=$(units)
echo "$units_after"
check 'unit state unchanged across upgrade' [ "$units_after" = "$units_before" ]
check 'cupertino-server payload identical' sha256sum --check --status /tmp/server.before
changed=$(altered)
[ -z "$changed" ] || { echo "$changed"; exit 1; }
echo 'package verification clean'
files | while read -r f; do [ -z "$f" ] || [ -e "$f" ] || [ -L "$f" ] || { echo "missing: $f"; exit 1; }; done
echo 'every packaged path present'
for b in /usr/bin/cupertino /usr/bin/cupertino-*; do
    [ "$b" = /usr/bin/cupertino-session ] && continue
    if ldd "$b" | grep -q 'not found'; then ldd "$b"; exit 1; fi
done
/usr/bin/cupertino-server --help >/dev/null
echo 'cupertino-server --help: ok'
remove
check 'removed cleanly' absent /usr/bin/cupertino-server /usr/share/cupertino/assets/manifest.json
check 'login enablement removed' absent /etc/systemd/system/multi-user.target.wants/cupertino-login.service
EOF
)

if [ "$mode" != --test-only ]; then
    if [ -n "$container_cache" ]; then
        mkdir -p "$container_cache/package/sccache" "$container_cache/package/cargo-home"
        container_cache=$(cd "$container_cache" && pwd)
        cargo_home="$container_cache/package/cargo-home"
        cache_mounts=(--mount "type=bind,src=$container_cache/package/sccache,dst=/cache/sccache")
        owned_cache=(/cache)
    else
        # Without the cache, Cargo's downloads stay in this worktree; they are checked against Cargo.lock either way.
        cargo_home="$root/target/package-build/$distro/cargo-home"
        mkdir -p "$cargo_home"
    fi
    image_key=$(sha256sum "$root/packaging/ci/Dockerfile" | cut -c1-16)
    build_image="cupertino-package-builder:$distro-$image_key"
    # Bound compiler parallelism in both image tool installs and the workspace build, which holds every build slot.
    jobs=${CARGO_BUILD_JOBS:-$(nproc)}
    build=(docker buildx build --load --target "$distro" --tag "$build_image" --build-arg "CARGO_BUILD_JOBS=$jobs")
    if [ -n "${CUPERTINO_BUILD_CACHE_DIR:-}" ]; then
        mkdir -p "$CUPERTINO_BUILD_CACHE_DIR/$distro"
        cache_root=$(cd "$CUPERTINO_BUILD_CACHE_DIR/$distro" && pwd)
        cache_next=$(mktemp -d "$cache_root/generation.XXXXXX")
        if [ -L "$cache_root/current" ]; then
            if [ ! -f "$cache_root/current/index.json" ]; then
                echo "builder cache is incomplete: $cache_root/current" >&2
                exit 1
            fi
            build+=(--cache-from "type=local,src=$cache_root/current")
        elif [ -e "$cache_root/current" ]; then
            echo "builder cache current must be a symlink: $cache_root/current" >&2
            exit 1
        fi
        build+=(--cache-to "type=local,dest=$cache_next,mode=max")
    fi
    phase one "${build[@]}" "$root/packaging/ci" &
    active_job=$!
    wait_job
    image_id=$(docker image inspect --format '{{.Id}}' "$build_image")
    if [[ ! "$image_id" =~ ^sha256:[0-9a-f]{64}$ ]]; then
        echo "builder image has an invalid content identity: $image_id" >&2
        exit 1
    fi
    if [ -n "$cache_next" ]; then
        if [ ! -f "$cache_next/index.json" ]; then
            echo "builder cache export is incomplete: $cache_next" >&2
            exit 1
        fi
        previous=
        if [ -L "$cache_root/current" ]; then
            previous=$(readlink "$cache_root/current")
        fi
        cache_link="$cache_root/current.new.$$"
        ln -s "$(basename "$cache_next")" "$cache_link"
        mv -Tf "$cache_link" "$cache_root/current"
        cache_link=
        cache_next=
        # Delete only a generation owned by this cache, after the successful export was atomically published.
        if [[ "$previous" == generation.* && "$previous" != */* ]]; then
            rm -rf "${cache_root:?}/${previous:?}"
        fi
    fi
    container=$(docker create --cidfile "$work/build.cid" --name "cupertino-$distro-$(basename "$work")-build" \
        --env "CARGO_BUILD_JOBS=$jobs" --env CARGO_HOME=/cargo-home -v "$work/src:/src" -v "$dist:/dist" \
        --mount "type=bind,src=$cargo_home,dst=/cargo-home" "${cache_mounts[@]}" "$image_id" bash -c "$build_script")
    restore_ownership=1
    start_container "$container" "" every
    container=$(docker create --cidfile "$work/inspect.cid" --name "cupertino-$distro-$(basename "$work")-inspect" \
        --network none -v "$work/src:/src" -v "$dist:/dist" "$image_id" bash -c "$inspect_script")
    start_container "$container" "" none
    restore_ownership=0
fi
if [ "$upgrade" = 1 ]; then
    container=$(docker create --cidfile "$work/test.cid" --name "cupertino-$distro-$(basename "$work")-test" \
        -v "$dist:/dist:ro" "$test_image" bash -c "$upgrade_script")
    start_container "$container" "$dist/upgrade-test-$distro.log" none
else
    container=$(docker create --cidfile "$work/test.cid" --name "cupertino-$distro-$(basename "$work")-test" \
        -v "$dist:/dist:ro" "$test_image" bash -c "$test_script")
    start_container "$container" "$dist/install-test-$distro.log" none
fi
if [ "$mode" = build ]; then
    python3 "$root/packaging/release.py" record-packages "$dist" --commit "$source_commit" ${RELEASE_TAG:+--tag "$RELEASE_TAG"}
fi
