#!/usr/bin/env bash
# Build, inspect and install-test the Arch packages in pinned containers.
#
#   packaging/ci/build-in-container.sh arch DIST_DIR [--test-only|--upgrade]
#
# Arch is the only distribution the packages target; the first argument names it so callers read the same everywhere.
# The source is the committed HEAD (git archive), so untracked or modified files never reach a package. A build
# container makes the runtime, data and SDK packages with makepkg (check() included) on the release source archive and
# the stable PKGBUILD, then runs namcap; release.py verifies each against the manifest. A second, fresh container of the same base installs them, checks the login-screen account, the unit
# state, the payload and conffile preservation across a reinstall, then removes them. It then installs the SDK package
# alone and builds and runs the C samples (the source's crates/cupertino-ui-abi, mounted read-only at /abi) against
# /usr. Packages, file lists and the test log land in DIST_DIR. --test-only skips the build and install-tests the packages already in DIST_DIR. Needs Docker Buildx, Python 3 and git; nothing is installed on the host.
#
# --upgrade packages the same build twice, as release 1 in DIST_DIR/rev-1 and as release 2 in DIST_DIR/rev-2 (no
# second compile), then, in a fresh container, installs release 1, enables the login service, edits login.conf,
# upgrades to release 2 and checks that the version rose and that the account, conffile, unit state and payload (the
# SDK's library and links too) survived.
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
    exec python3 ci/worktree-build.py locked -- bash packaging/ci/build-in-container.sh "$distro" "$dist" "$mode"
fi
# The source copy and the Arch build tree are large; they live under the gitignored target/, not in /tmp.
mkdir -p "$root/target"
work=$(mktemp -d "$root/target/container-build.XXXXXX")
cache_next=
cache_link=
active_job=
restore_ownership=0
cleanup() {
    local status=$? cleanup_status=0 container cidfile
    trap - EXIT
    trap "" INT TERM
    # Containers are created stopped and recorded before compilation can start. Only this run's ids are removed.
    for cidfile in "$work/build.cid" "$work/test.cid"; do
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
            --mount "type=bind,src=$target,dst=/src/target" \
            --mount "type=bind,src=$cargo_home,dst=/cargo-home" \
            "$image_id" chown -R "$(id -u):$(id -g)" /dist /src /cargo-home; then
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

start_container() {
    local container=$1 log=$2
    if [ -n "$log" ]; then
        docker start --attach "$container" | tee "$log" &
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

build_script=$(cat <<EOF
set -euo pipefail
trap 'chown -R $(id -u):$(id -g) /dist /src /cargo-home' EXIT
cd /src
# Packages are stripped; generating debug information only consumes build storage.
export CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/src/target CUPERTINO_THEME_DIR=/usr/share/cupertino/theme CARGO_PROFILE_RELEASE_DEBUG=0
upgrade=$upgrade
chown -R builder /src/arch-build /src/target
chown builder /cargo-home
su builder -c 'cd /src/arch-build && makepkg --noconfirm'
df -h /src | tee /dist/disk-after-makepkg.log
namcap arch-build/PKGBUILD arch-build/*.pkg.tar.zst | tee /dist/namcap.log
publish() {
    mkdir -p "\$1"
    cp arch-build/*.pkg.tar.zst arch-build/PKGBUILD "\$1/"
    python3 packaging/release.py verify --format arch --package runtime "\$1"/cupertino-0*.pkg.tar.zst
    python3 packaging/release.py verify --format arch --package data "\$1"/cupertino-data-*.pkg.tar.zst
    python3 packaging/release.py verify --format arch --package sdk "\$1"/cupertino-kit-*.pkg.tar.zst
    for p in "\$1"/*.pkg.tar.zst; do pacman -Qip "\$p" > "\$p.info"; pacman -Qlp "\$p" > "\$p.contents"; done
}
if [ "\$upgrade" = 1 ]; then
    publish /dist/rev-1
    rm arch-build/*.pkg.tar.zst
    sed -i 's/^pkgrel=1\$/pkgrel=2/' arch-build/PKGBUILD
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
for b in /usr/bin/cupertino-*; do
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
test -f /usr/share/cupertino/theme/tokens.json && test -f /usr/share/cupertino/assets/manifest.json
echo '# local edit' >> /etc/cupertino/login.conf
reinstall
check 'login.conf kept across reinstall' grep -q '^# local edit' /etc/cupertino/login.conf
remove
check 'removed cleanly' absent /usr/bin/cupertino-server /usr/share/cupertino/assets/manifest.json
# The C SDK as a developer gets it: the cupertino-kit package alone, then C programs built from / by a user without a
# session, with only /usr/bin on PATH, against the installed header, library and pkg-config file.
sdk_checks() {
    local user=cupertino-sdk-test home=/tmp/sdk major minor version
    pacman -S --noconfirm --needed -q gcc make pkgconf >/dev/null
    pacman -U --noconfirm /dist/cupertino-kit-*.pkg.tar.zst >/dev/null
    check 'cupertino-kit installs without cupertino' sh -c '! pacman -Q cupertino >/dev/null 2>&1'
    major=$(sed -n 's/^#define CUPERTINOKIT_ABI_MAJOR \([0-9]*\)$/\1/p' /usr/include/cupertinokit-1/cupertinokit.h)
    minor=$(sed -n 's/^#define CUPERTINOKIT_ABI_MINOR \([0-9]*\)$/\1/p' /usr/include/cupertinokit-1/cupertinokit.h)
    version=$(pkg-config --modversion cupertinokit)
    check "pkg-config --modversion cupertinokit: $version, the header's ABI $major.$minor" test "$version" = "$major.$minor.0"
    check 'libcupertinokit.so.1 is the soname' sh -c "readelf -d /usr/lib/libcupertinokit.so.1 | grep -q 'SONAME.*libcupertinokit.so.1'"
    check "libcupertinokit.so.1 -> $(readlink /usr/lib/libcupertinokit.so.1)" \
        test "$(readlink /usr/lib/libcupertinokit.so.1)" = "libcupertinokit.so.1.$minor.0"
    check 'libcupertinokit.so -> libcupertinokit.so.1' test "$(readlink /usr/lib/libcupertinokit.so)" = libcupertinokit.so.1
    if ldd /usr/lib/libcupertinokit.so.1 | grep -q 'not found'; then ldd /usr/lib/libcupertinokit.so.1; exit 1; fi
    echo 'libcupertinokit.so.1 resolves its libraries'
    useradd --no-create-home --shell /usr/bin/nologin "$user"
    install -d -o "$user" -g "$user" -m 700 "$home" "$home/make"
    install -o "$user" -g "$user" -m 644 /abi/examples/c/headless.c /abi/examples/c/headless.expected /abi/tests/c/layout.c \
        /abi/abi/layout-1.txt "$home/"
    install -o "$user" -g "$user" -m 644 /abi/examples/c/headless.c /abi/examples/c/Makefile "$home/make/"
    sdk_user() { (cd "$home" && exec setpriv --reuid="$user" --regid="$user" --clear-groups env -i PATH=/usr/bin HOME="$home" sh -c "$1"); }
    check 'the headless sample compiles against /usr with -Werror' sdk_user \
        'cc -std=c11 -Wall -Wextra -Werror -pthread $(pkg-config --cflags cupertinokit) headless.c -o headless $(pkg-config --libs cupertinokit)'
    check 'it compiles as C++ too' sdk_user \
        'c++ -x c++ -std=c++17 -Wall -Wextra -Werror -pthread $(pkg-config --cflags cupertinokit) headless.c -o headless-cxx $(pkg-config --libs cupertinokit)'
    check 'the headless sample prints its trace and exits 0' sdk_user './headless > headless.out && cmp headless.out headless.expected'
    check 'the C++ build prints the same trace' sdk_user './headless-cxx > headless-cxx.out && cmp headless-cxx.out headless.expected'
    check 'the Makefile builds it with pkg-config alone' sdk_user 'make -C make && ./make/headless > make.out && cmp make.out headless.expected'
    check 'the C compiler sees the struct layouts of layout-1.txt' sdk_user \
        'cc -std=c11 -Wall -Wextra -Werror $(pkg-config --cflags cupertinokit) layout.c -o layout && ./layout > layout.out && diff layout.out layout-1.txt'
    # with the desktop installed too, the cupertino command reports and checks the SDK, and starts a project on it
    pacman -U --noconfirm /dist/cupertino-data-*.pkg.tar.zst /dist/cupertino-0*.pkg.tar.zst >/dev/null
    check 'cupertino sdk info --require: the SDK is installed' sdk_user 'cupertino sdk info --require'
    check 'cupertino sdk check compiles and runs its probe' sdk_user 'cupertino sdk check'
    check 'cupertino sdk new --lang c demo && make -C demo: the starter builds and runs' sdk_user \
        'cupertino sdk new --lang c demo && make -C demo && ./demo/demo'
    pacman -Rns --noconfirm cupertino cupertino-data >/dev/null
    pacman -Rns --noconfirm cupertino-kit >/dev/null
    check 'cupertino-kit removed cleanly' absent /usr/lib/libcupertinokit.so.1 /usr/lib/libcupertinokit.so \
        /usr/include/cupertinokit-1 /usr/lib/pkgconfig/cupertinokit.pc /usr/share/cupertino-kit
    check 'no libcupertinokit file is left' sh -c '! ls /usr/lib/libcupertinokit.so* 2>/dev/null'
    userdel "$user"
}
sdk_checks
EOF
)

upgrade_script=$(cat <<'EOF'
set -euo pipefail
check() { local label=$1; shift; "$@" || { echo "FAIL: $label"; exit 1; }; echo "$label"; }
absent() { for p in "$@"; do [ ! -e "$p" ] || return 1; done; }
pacman -Syu --noconfirm -q >/dev/null
install() { pacman -U --noconfirm "$1"/cupertino-data-*.pkg.tar.zst "$1"/cupertino-0*.pkg.tar.zst "$1"/cupertino-kit-*.pkg.tar.zst >/dev/null; }
version() { pacman -Q cupertino | cut -d' ' -f2; }
newer() { [ "$(vercmp "$1" "$2")" = 1 ]; }
files() { pacman -Qql cupertino cupertino-data cupertino-kit; }
altered() { pacman -Qkk cupertino cupertino-data cupertino-kit 2>&1 | grep '^warning' | grep -v '/etc/cupertino/login.conf' || true; }
remove() { pacman -Rns --noconfirm cupertino cupertino-data cupertino-kit >/dev/null; }
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
sha256sum "$(readlink -f /usr/lib/libcupertinokit.so)" > /tmp/sdk.before
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
check 'the SDK library behind libcupertinokit.so identical' sha256sum --check --status /tmp/sdk.before
check 'kit upgraded with the runtime' test "$(pacman -Q cupertino-kit | cut -d' ' -f2)" = "$new"
changed=$(altered)
[ -z "$changed" ] || { echo "$changed"; exit 1; }
echo 'package verification clean'
files | while read -r f; do [ -z "$f" ] || [ -e "$f" ] || [ -L "$f" ] || { echo "missing: $f"; exit 1; }; done
echo 'every packaged path present'
for b in /usr/bin/cupertino-*; do
    [ "$b" = /usr/bin/cupertino-session ] && continue
    if ldd "$b" | grep -q 'not found'; then ldd "$b"; exit 1; fi
done
/usr/bin/cupertino-server --help >/dev/null
echo 'cupertino-server --help: ok'
remove
check 'removed cleanly' absent /usr/bin/cupertino-server /usr/share/cupertino/assets/manifest.json
check 'login enablement removed' absent /etc/systemd/system/multi-user.target.wants/cupertino-login.service
check 'SDK removed' absent /usr/lib/libcupertinokit.so.1 /usr/include/cupertinokit-1
EOF
)

# Each worktree owns its Cargo outputs; no global volume can mix incompatible builds.
if [ "$mode" != --test-only ]; then
    target_root="$root/target/package-build/$distro"
    cargo_home="$target_root/cargo-home"
    mkdir -p "$cargo_home"
    image_key=$(sha256sum "$root/packaging/ci/Dockerfile" | cut -c1-16)
    build_image="cupertino-package-builder:$distro-$image_key"
    # Bound compiler parallelism in both image tool installs and the subsequent workspace build.
    jobs=${CARGO_BUILD_JOBS:-4}
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
    "${build[@]}" "$root/packaging/ci" &
    active_job=$!
    wait_job
    image_id=$(docker image inspect --format '{{.Id}}' "$build_image")
    if [[ ! "$image_id" =~ ^sha256:[0-9a-f]{64}$ ]]; then
        echo "builder image has an invalid content identity: $image_id" >&2
        exit 1
    fi
    target="$target_root/targets/${image_id#sha256:}"
    mkdir -p "$target"
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
            rm -rf "$cache_root/$previous"
        fi
    fi
    container=$(docker create --cidfile "$work/build.cid" --name "cupertino-$distro-$(basename "$work")-build" \
        --env "CARGO_BUILD_JOBS=$jobs" --env CARGO_HOME=/cargo-home -v "$work/src:/src" -v "$dist:/dist" \
        --mount "type=bind,src=$target,dst=/src/target" \
        --mount "type=bind,src=$cargo_home,dst=/cargo-home" "$image_id" bash -c "$build_script")
    restore_ownership=1
    start_container "$container" ""
    restore_ownership=0
fi
if [ "$upgrade" = 1 ]; then
    container=$(docker create --cidfile "$work/test.cid" --name "cupertino-$distro-$(basename "$work")-test" \
        -v "$dist:/dist:ro" "$test_image" bash -c "$upgrade_script")
    start_container "$container" "$dist/upgrade-test-$distro.log"
else
    container=$(docker create --cidfile "$work/test.cid" --name "cupertino-$distro-$(basename "$work")-test" \
        -v "$dist:/dist:ro" -v "$work/src/crates/cupertino-ui-abi:/abi:ro" "$test_image" bash -c "$test_script")
    start_container "$container" "$dist/install-test-$distro.log"
fi
if [ "$mode" = build ]; then
    python3 "$root/packaging/release.py" record-packages "$dist" --commit "$source_commit" ${RELEASE_TAG:+--tag "$RELEASE_TAG"}
fi
