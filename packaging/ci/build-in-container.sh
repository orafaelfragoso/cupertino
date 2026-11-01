#!/usr/bin/env bash
# Build, inspect and install-test the native packages of one distribution in pinned containers.
#
#   packaging/ci/build-in-container.sh {arch|debian|fedora} DIST_DIR [--test-only|--upgrade]
#
# The source is the committed HEAD (git archive), so untracked or modified files never reach a package. A build
# container compiles the workspace and makes the runtime and data packages (Arch: makepkg with check() on the release
# source archive and the stable PKGBUILD, then namcap); release.py verifies each against the manifest. A second, fresh container of the same base installs them, checks the login-screen account, the unit
# state, the payload and conffile preservation across a reinstall, then removes them. Packages, file lists and the
# test log land in DIST_DIR. --test-only skips the build and install-tests the packages already in DIST_DIR. Needs docker and git; nothing is installed on the host.
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

# Images by digest; the Rust toolchain is fixed (Arch: the rolling distribution's rust), as are the packaging tools.
RUST_TOOLCHAIN=1.99.1
CARGO_DEB=3.8.0
CARGO_GENERATE_RPM=0.21.1
case $distro in
arch)
    build_image=docker.io/library/archlinux@sha256:3cbf69d2d5958d6a294f3722c5668e6187c4df8e00fa27f72352f0bf83e495e1 # base-devel
    test_image=$build_image
    ;;
debian)
    build_image=docker.io/library/rust@sha256:931c2c0531a8b143e044f0f3914dfe802f15bb3d1955f0b657e9359327ac2ffc # 1.99.1-trixie
    test_image=docker.io/library/debian@sha256:4c25275446ca371766b8b093c696782eabce06263fccacb8d5d813b3dc44b308 # trixie
    ;;
fedora)
    build_image=docker.io/library/fedora@sha256:43b29f65a41eb9c35e1cd5323e3bdf3b655c2357a9f4f1ff2f9c2798e5045d80 # 44
    test_image=$build_image
    ;;
*)
    echo "unknown distribution: $distro" >&2
    exit 2
    ;;
esac

mkdir -p "$dist"
dist=$(cd "$dist" && pwd)
# The source copy and the Arch build tree are large; they live under the gitignored target/, not in /tmp.
mkdir -p "$root/target"
work=$(mktemp -d "$root/target/container-build.XXXXXX")
trap 'rm -rf "$work"' EXIT
git -C "$root" archive --format=tar --prefix=src/ HEAD | tar -x -C "$work"
if [ "$distro" = arch ]; then
    mkdir "$work/src/arch-build"
    archive=$(python3 "$root/packaging/release.py" source --output "$work/src/arch-build" ${RELEASE_TAG:+--tag "$RELEASE_TAG"})
    python3 "$root/packaging/release.py" pkgbuild ${RELEASE_TAG:+--tag "$RELEASE_TAG"} "$archive" "$work/src/arch-build/PKGBUILD" >/dev/null
fi

build_script=$(cat <<EOF
set -euo pipefail
trap 'chown -R $(id -u):$(id -g) /dist /src' EXIT
cd /src
export CARGO_INCREMENTAL=0 CUPERTINO_THEME_DIR=/usr/share/cupertino/theme
upgrade=$upgrade
case $distro in
arch)
    pacman -Syu --noconfirm --needed -q rust python mesa libglvnd pam dbus xkeyboard-config shared-mime-info gcc-libs systemd adwaita-icon-theme clang libpipewire pipewire namcap >/dev/null
    useradd -m builder
    chown -R builder /src/arch-build
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
    apt-get update -qq
    apt-get install -y -qq --no-install-recommends pkg-config libgbm-dev libpam0g-dev clang libclang-dev libpipewire-0.3-dev dpkg-dev >/dev/null
    cargo install --locked --quiet --version $CARGO_DEB cargo-deb
    export CUPERTINO_ADMIN_GROUPS=sudo
    ;;
fedora)
    dnf install -y -q gcc pkgconf-pkg-config mesa-libgbm-devel pam-devel clang pipewire-devel rustup rpm-build cpio python3 binutils >/dev/null
    rustup-init -y -q --profile minimal --default-toolchain $RUST_TOOLCHAIN >/dev/null
    . "\$HOME/.cargo/env"
    cargo install --locked --quiet --version $CARGO_GENERATE_RPM cargo-generate-rpm
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

# The source copy is private to this run; the target volume keeps the build between local runs.
if [ "$mode" != --test-only ]; then
    docker run --rm -v "$work/src:/src" -v "$dist:/dist" \
        --mount "type=volume,src=cupertino-$distro-target,dst=/src/target" "$build_image" bash -c "$build_script"
fi
if [ "$upgrade" = 1 ]; then
    docker run --rm -v "$dist:/dist:ro" "$test_image" bash -c "$upgrade_script" bash "$distro" | tee "$dist/upgrade-test-$distro.log"
else
    docker run --rm -v "$dist:/dist:ro" "$test_image" bash -c "$test_script" bash "$distro" | tee "$dist/install-test-$distro.log"
fi
