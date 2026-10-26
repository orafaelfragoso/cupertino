#!/usr/bin/env bash
# Build, inspect and install-test the native packages of one distribution in pinned containers.
#
#   packaging/ci/build-in-container.sh {debian|fedora} DIST_DIR
#
# The source is the committed HEAD (git archive), so untracked or modified files never reach a package. A build
# container compiles the workspace and makes the runtime and data packages; release.py verifies each against the
# manifest. A second, fresh container of the same base installs them, checks the login-screen account, the unit
# state, the payload and conffile preservation across a reinstall, then removes them. Packages, file lists and the
# test log land in DIST_DIR. Needs docker and git; nothing is installed on the host.
set -euo pipefail

usage="usage: build-in-container.sh debian|fedora DIST_DIR"
distro=${1:?$usage}
dist=${2:?$usage}
root=$(git -C "$(dirname "$0")" rev-parse --show-toplevel)

# Images by digest; the Rust toolchain is fixed, as are the packaging tools.
RUST_TOOLCHAIN=1.99.1
CARGO_DEB=3.8.0
CARGO_GENERATE_RPM=0.21.1
case $distro in
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
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
git -C "$root" archive --format=tar --prefix=src/ HEAD | tar -x -C "$work"

build_script=$(cat <<EOF
set -euo pipefail
trap 'chown -R $(id -u):$(id -g) /dist' EXIT
cd /src
case $distro in
debian)
    apt-get update -qq
    apt-get install -y -qq --no-install-recommends pkg-config libgbm-dev libpam0g-dev dpkg-dev >/dev/null
    cargo install --locked --quiet --version $CARGO_DEB cargo-deb
    ;;
fedora)
    dnf install -y -q gcc pkgconf-pkg-config mesa-libgbm-devel pam-devel rustup rpm-build cpio python3 binutils >/dev/null
    rustup-init -y -q --profile minimal --default-toolchain $RUST_TOOLCHAIN >/dev/null
    . "\$HOME/.cargo/env"
    cargo install --locked --quiet --version $CARGO_GENERATE_RPM cargo-generate-rpm
    ;;
esac
rustc --version
export CARGO_INCREMENTAL=0 CUPERTINO_THEME_DIR=/usr/share/cupertino/theme
export RUSTFLAGS="--remap-path-prefix=/src=/usr/src/cupertino --remap-path-prefix=\${CARGO_HOME:-\$HOME/.cargo}=/cargo"
cargo build --locked --release --workspace --all-features --bins
mkdir -p /dist
case $distro in
debian)
    cargo deb -p cupertino-server --no-build --output /dist
    cargo deb -p cupertino-assets --no-build --output /dist
    python3 packaging/release.py verify --format deb --package runtime /dist/cupertino_*_amd64.deb
    python3 packaging/release.py verify --format deb --package data /dist/cupertino-data_*.deb
    for p in /dist/*.deb; do dpkg-deb --info "\$p" > "\$p.info"; dpkg-deb --contents "\$p" > "\$p.contents"; done
    ;;
fedora)
    strip --strip-unneeded \$(python3 -c 'import sys; sys.path.insert(0, "packaging"); import release; print(" ".join("target/release/" + b for b in release.BINARIES))')
    cargo generate-rpm -p crates/cupertino-server -o /dist
    cargo generate-rpm -p crates/cupertino-assets -o /dist
    python3 packaging/release.py verify --format rpm --package runtime /dist/cupertino-0*.rpm
    python3 packaging/release.py verify --format rpm --package data /dist/cupertino-data-*.rpm
    for p in /dist/*.rpm; do rpm -qip "\$p" > "\$p.info"; rpm -qlvp "\$p" > "\$p.contents"; rpm -qpR "\$p" > "\$p.requires"; done
    ;;
esac
EOF
)

test_script=$(cat <<'EOF'
set -euo pipefail
case $1 in
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
/usr/bin/cupertino-server --help >/dev/null && echo 'cupertino-server --help: ok'
test -f /usr/share/cupertino/theme/tokens.json && test -f /usr/share/cupertino/assets/manifest.json
echo '# local edit' >> /etc/cupertino/login.conf
reinstall
grep -q '^# local edit' /etc/cupertino/login.conf && echo 'login.conf kept across reinstall'
remove
test ! -e /usr/bin/cupertino-server && test ! -e /usr/share/cupertino/assets/manifest.json && echo 'removed cleanly'
EOF
)

# The source copy is private to this run; the target volume keeps the build between local runs.
docker run --rm -v "$work/src:/src" -v "$dist:/dist" \
    --mount "type=volume,src=cupertino-$distro-target,dst=/src/target" "$build_image" bash -c "$build_script"
docker run --rm -v "$dist:/dist:ro" "$test_image" bash -c "$test_script" bash "$distro" | tee "$dist/install-test-$distro.log"
