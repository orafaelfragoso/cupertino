#!/usr/bin/env bash
# Build, inspect and install-test the native packages of one distribution in pinned containers.
#
#   packaging/ci/build-in-container.sh {arch|debian|fedora} DIST_DIR [--test-only|--upgrade]
#
# The source is the committed HEAD (git archive), so untracked or modified files never reach a package. A build
# container compiles the workspace and makes the runtime and data packages (Arch: makepkg with check() on the release
# source archive and the stable PKGBUILD, then namcap); release.py verifies each against the manifest. A second, fresh container of the same base installs them, checks the login-screen account, the unit
# state, the payload and conffile preservation across a reinstall, then removes them. Packages, file lists and the
# test log land in DIST_DIR. --test-only skips the build and install-tests the packages already in DIST_DIR. Needs Docker Buildx, Python 3 and git; nothing is installed on the host.
#
# --upgrade packages the same build twice, as release 1 in DIST_DIR/rev-1 and as release 2 in DIST_DIR/rev-2 (no
# second compile), then, in a fresh container, installs release 1, enables the login service, edits login.conf,
# upgrades to release 2 and checks that the version rose and that the account, conffile, unit state and payload survived.
set -euo pipefail

usage="usage: build-in-container.sh arch|debian|fedora DIST_DIR [--test-only|--upgrade]"
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
# Fresh install tests retain their original distribution bases, never the dependency-filled builder images.
case $distro in
arch)
    test_image=docker.io/library/archlinux@sha256:3cbf69d2d5958d6a294f3722c5668e6187c4df8e00fa27f72352f0bf83e495e1
    ;;
debian)
    test_image=docker.io/library/debian@sha256:4c25275446ca371766b8b093c696782eabce06263fccacb8d5d813b3dc44b308
    ;;
fedora)
    test_image=docker.io/library/fedora@sha256:43b29f65a41eb9c35e1cd5323e3bdf3b655c2357a9f4f1ff2f9c2798e5045d80
    ;;
*)
    echo "unknown distribution: $distro" >&2
    exit 2
    ;;
esac
mkdir -p "$dist"
dist=$(cd "$dist" && pwd)
# Hold the same lifetime lease as local Cargo work before building an image or touching a package target.
if [ "$mode" != --test-only ] && [ -z "${CUPERTINO_BUILD_LOCK_FD:-}" ]; then
    cd "$root"
    exec python3 ci/worktree-build.py locked -- bash packaging/ci/build-in-container.sh "$distro" "$dist" "$mode"
fi
# The source copy and the Arch build tree are large; they live under the gitignored target/, not in /tmp.
mkdir -p "$root/target"
work=$(mktemp -d "$root/target/container-build.XXXXXX")
cache_next=
cache_link=
active_job=
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
if [ "$distro" = arch ] && [ "$mode" != --test-only ]; then
    mkdir "$work/src/arch-build"
    archive=$(python3 "$root/packaging/release.py" source --commit "$source_commit" --output "$work/src/arch-build" ${RELEASE_TAG:+--tag "$RELEASE_TAG"})
    python3 "$root/packaging/release.py" pkgbuild --commit "$source_commit" ${RELEASE_TAG:+--tag "$RELEASE_TAG"} "$archive" "$work/src/arch-build/PKGBUILD" >/dev/null
    if [ -n "${CUPERTINO_PKGREL:-}" ]; then
        sed -i "s/^pkgrel=.*/pkgrel=$CUPERTINO_PKGREL/" "$work/src/arch-build/PKGBUILD"
    fi
fi

build_script=$(cat <<EOF
set -euo pipefail
trap 'chown -R $(id -u):$(id -g) /dist /src' EXIT
cd /src
export CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/src/target CUPERTINO_THEME_DIR=/usr/share/cupertino/theme
upgrade=$upgrade
case $distro in
arch)
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
    exit 0
    ;;
debian)
    export CUPERTINO_ADMIN_GROUPS=sudo
    ;;
esac
rustc --version
export RUSTFLAGS="--remap-path-prefix=/src=/usr/src/cupertino --remap-path-prefix=\${CARGO_HOME:-\$HOME/.cargo}=/cargo"
cargo build --locked --release --workspace --all-features --bins
mkdir -p /dist
case $distro in
debian|fedora)
    strip --strip-unneeded \$(python3 -c 'import sys; sys.path.insert(0, "packaging"); import release; print(" ".join("target/release/" + b for b in release.BINARIES))')
    ;;
esac
case $distro in
debian)
    package() {
        mkdir -p "\$1"
        cargo deb -p cupertino-server --no-build --output "\$1" "\${@:2}"
        cargo deb -p cupertino-assets --no-build --output "\$1" "\${@:2}"
        python3 packaging/release.py verify --format deb --package runtime "\$1"/cupertino_*_amd64.deb
        python3 packaging/release.py verify --format deb --package data "\$1"/cupertino-data_*.deb
        for p in "\$1"/*.deb; do dpkg-deb --info "\$p" > "\$p.info"; dpkg-deb --contents "\$p" > "\$p.contents"; done
    }
    if [ "\$upgrade" = 1 ]; then package /dist/rev-1; package /dist/rev-2 --deb-revision 2; else package /dist; fi
    ;;
fedora)
    package() {
        mkdir -p "\$1"
        cargo generate-rpm -p crates/cupertino-server -o "\$1" "\${@:2}"
        cargo generate-rpm -p crates/cupertino-assets -o "\$1" "\${@:2}"
        python3 packaging/release.py verify --format rpm --package runtime "\$1"/cupertino-0*.rpm
        python3 packaging/release.py verify --format rpm --package data "\$1"/cupertino-data-*.rpm
        for p in "\$1"/*.rpm; do rpm -qip "\$p" > "\$p.info"; rpm -qlvp "\$p" > "\$p.contents"; rpm -qpR "\$p" > "\$p.requires"; done
    }
    if [ "\$upgrade" = 1 ]; then package /dist/rev-1; package /dist/rev-2 --set-metadata 'release = "2"'; else package /dist; fi
    ;;
esac
EOF
)

test_script=$(cat <<'EOF'
set -euo pipefail
check() { local label=$1; shift; "$@" || { echo "FAIL: $label"; exit 1; }; echo "$label"; }
absent() { for p in "$@"; do [ ! -e "$p" ] || return 1; done; }
case $1 in
arch)
    pacman -Syu --noconfirm -q >/dev/null
    pacman -U --noconfirm /dist/cupertino-data-*.pkg.tar.zst /dist/cupertino-0*.pkg.tar.zst >/dev/null
    reinstall() { pacman -U --noconfirm /dist/cupertino-0*.pkg.tar.zst >/dev/null 2>&1; }
    remove() { pacman -Rns --noconfirm cupertino cupertino-data >/dev/null; }
    ;;
debian)
    apt-get update -qq
    apt-get install -y -qq /dist/cupertino-data_*.deb /dist/cupertino_*_amd64.deb >/dev/null
    reinstall() { apt-get install -y -qq --reinstall -o Dpkg::Options::=--force-confold /dist/cupertino_*_amd64.deb >/dev/null; }
    remove() { apt-get purge -y -qq cupertino cupertino-data >/dev/null; }
    ;;
fedora)
    dnf install -y -q /dist/cupertino-data-*.rpm /dist/cupertino-0*.rpm >/dev/null
    reinstall() { dnf reinstall -y -q /dist/cupertino-0*.rpm >/dev/null; }
    remove() { dnf remove -y -q cupertino cupertino-data >/dev/null; }
    ;;
esac
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
EOF
)

upgrade_script=$(cat <<'EOF'
set -euo pipefail
check() { local label=$1; shift; "$@" || { echo "FAIL: $label"; exit 1; }; echo "$label"; }
absent() { for p in "$@"; do [ ! -e "$p" ] || return 1; done; }
case $1 in
arch)
    pacman -Syu --noconfirm -q >/dev/null
    install() { pacman -U --noconfirm "$1"/cupertino-data-*.pkg.tar.zst "$1"/cupertino-0*.pkg.tar.zst >/dev/null; }
    version() { pacman -Q cupertino | cut -d' ' -f2; }
    newer() { [ "$(vercmp "$1" "$2")" = 1 ]; }
    files() { pacman -Qql cupertino cupertino-data; }
    altered() { pacman -Qkk cupertino cupertino-data 2>&1 | grep '^warning' | grep -v '/etc/cupertino/login.conf' || true; }
    remove() { pacman -Rns --noconfirm cupertino cupertino-data >/dev/null; }
    ;;
debian)
    apt-get update -qq
    install() { apt-get install -y -qq -o Dpkg::Options::=--force-confold "$1"/cupertino-data_*.deb "$1"/cupertino_*_amd64.deb >/dev/null; }
    version() { dpkg-query -W -f='${Version}' cupertino; }
    newer() { dpkg --compare-versions "$1" gt "$2"; }
    files() { dpkg -L cupertino cupertino-data; }
    altered() { dpkg --verify cupertino cupertino-data | grep -v ' c /etc/cupertino/login.conf' || true; }
    remove() { apt-get purge -y -qq cupertino cupertino-data >/dev/null; }
    ;;
fedora)
    install() { dnf install -y -q "$1"/cupertino-data-*.rpm "$1"/cupertino-0*.rpm >/dev/null; }
    version() { rpm -q --qf '%{VERSION}-%{RELEASE}' cupertino; }
    newer() { [ "$(rpm --eval "%{lua: print(rpm.vercmp('$1', '$2'))}")" = 1 ]; }
    files() { rpm -ql cupertino cupertino-data; }
    altered() { rpm -V cupertino cupertino-data | grep -v ' c /etc/cupertino/login.conf' || true; }
    remove() { dnf remove -y -q cupertino cupertino-data >/dev/null; }
    ;;
esac
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
for b in /usr/bin/cupertino-*; do
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

# Each worktree and distribution owns its Cargo outputs; no global volume can mix incompatible builds.
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
    start_container "$container" ""
fi
if [ "$upgrade" = 1 ]; then
    container=$(docker create --cidfile "$work/test.cid" --name "cupertino-$distro-$(basename "$work")-test" \
        -v "$dist:/dist:ro" "$test_image" bash -c "$upgrade_script" bash "$distro")
    start_container "$container" "$dist/upgrade-test-$distro.log"
else
    container=$(docker create --cidfile "$work/test.cid" --name "cupertino-$distro-$(basename "$work")-test" \
        -v "$dist:/dist:ro" "$test_image" bash -c "$test_script" bash "$distro")
    start_container "$container" "$dist/install-test-$distro.log"
fi
if [ "$distro" = arch ] && [ "$mode" = build ]; then
    python3 "$root/packaging/release.py" record-packages "$dist" --commit "$source_commit" ${RELEASE_TAG:+--tag "$RELEASE_TAG"}
fi
