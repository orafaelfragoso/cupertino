# The one Arch image of CI and of the package builds. Arch is pinned by digest (archlinux:base-devel-20261101.0.620205),
# and every package comes from one Arch Linux Archive date, so the compilers and libraries do not move until this file
# does. Change the digest and the date together. The toolchain matches rust-toolchain.toml.
#
# Jobs run their steps as the unprivileged user `ci` (the tests and makepkg must not run as root); the setup action
# (.github/actions/arch-setup) gives it the uid of the runner's own user and hands it the checkout and the caches.
FROM docker.io/library/archlinux:base-devel@sha256:2de80105abb49f813a85393c6758a4334deb6bcec4a2411bd2f059c0bee6ee4c

ARG ARCHIVE=2026/11/01
ARG RUST_VERSION=1.100.0
ARG NEXTEST_VERSION=0.9.151
ARG NEXTEST_SHA256=6b284d6fde9670dfb6ee10ea9d9abbc85f95643d3f592472d420d82e257b3a6c
# the version of the gungraun library in Cargo.lock; the microbench job installs another one when the lock moves
ARG GUNGRAUN_RUNNER_VERSION=0.20.0
# the uid of the runner's user on the hosted runners; the setup action changes it when a runner differs
ARG CI_UID=1001

# The archive date is the only package source: the mirror list holds that one server, pacman.conf names no server of its
# own and every repository includes the mirror list. -Syyuu moves every installed package to that date, down too. The
# downloaded packages are removed by hand: `pacman -Scc --noconfirm` answers no to removing them.
#
# Packages: the PKGBUILD's makedepends (cargo is rustup's), the tools the builds and checks start (git, jq, zstd, mold,
# sccache), what the workspace's tests start (the session bus, PipeWire, the keyring daemon and secret-tool, the shells
# and mandoc of the command's completions and manual pages), the signing and release tools (gnupg, namcap, archiso) and
# Valgrind for the instruction counts.
RUN printf 'Server = https://archive.archlinux.org/repos/%s/$repo/os/$arch\n' "$ARCHIVE" > /etc/pacman.d/mirrorlist \
 && [ "$(grep -c '^[[:space:]]*Server' /etc/pacman.d/mirrorlist)" = 1 ] \
 && grep -qxF "Server = https://archive.archlinux.org/repos/$ARCHIVE/\$repo/os/\$arch" /etc/pacman.d/mirrorlist \
 && ! grep -q '^[[:space:]]*Server' /etc/pacman.conf \
 && [ -z "$(grep '^[[:space:]]*Include' /etc/pacman.conf | grep -v '= /etc/pacman.d/mirrorlist$')" ] \
 && pacman -Syyuu --noconfirm --needed \
      base-devel python mesa libglvnd pam gcc-libs systemd dbus xkeyboard-config shared-mime-info \
      adwaita-icon-theme clang libpipewire rustup git jq zstd mold sccache cargo-about \
      pipewire gnome-keyring libsecret bash-completion zsh fish mandoc \
      gnupg namcap archiso valgrind \
 && rm -rf /var/cache/pacman/pkg/*

# The toolchain lives outside any home so that every user reads the same one; `ci` owns it (and the cargo home, where
# the registry is downloaded) once the setup action has run. Arch's rustup puts the proxies in /usr/bin. A checkout's
# rust-toolchain.toml picks the toolchain; one it names that the image lacks is installed on first use.
ENV RUSTUP_HOME=/opt/rustup CARGO_HOME=/opt/cargo
RUN rustup toolchain install "$RUST_VERSION" --profile minimal --component rustfmt,clippy \
 && rustup default "$RUST_VERSION" \
 && cargo --version && cargo fmt --version && cargo clippy --version

# The build user, with the cache directory sccache writes when a job keeps its compile cache.
RUN useradd --create-home --uid "$CI_UID" ci \
 && install -d -o ci /var/cache/sccache "$CARGO_HOME" \
 && chown -R ci "$RUSTUP_HOME" "$CARGO_HOME"

# makepkg checks its check dependencies in pacman's database, so the verified prebuilt test runner is a real package.
RUN <<'SHELL'
set -eu
install -d -o ci /tmp/nextest
cat > /tmp/nextest/PKGBUILD <<PKGBUILD
pkgname=cargo-nextest
pkgver=$NEXTEST_VERSION
pkgrel=1
pkgdesc='A next-generation Rust test runner'
arch=('x86_64')
url='https://nexte.st/'
license=('MIT' 'Apache-2.0')
depends=('cargo')
options=('!strip' '!debug')
source=("https://github.com/nextest-rs/nextest/releases/download/cargo-nextest-\$pkgver/cargo-nextest-\$pkgver-x86_64-unknown-linux-musl.tar.gz")
sha256sums=('$NEXTEST_SHA256')
package() {
  install -Dm755 cargo-nextest "\$pkgdir/usr/bin/cargo-nextest"
}
PKGBUILD
chown ci /tmp/nextest/PKGBUILD
runuser -u ci -- bash -c 'cd /tmp/nextest && makepkg --noconfirm'
pacman -U --noconfirm /tmp/nextest/cargo-nextest-*.pkg.tar.zst
rm -rf /tmp/nextest
cargo nextest --version
SHELL

# The runner of the instruction-count benches must be exactly the library's version.
RUN cargo install gungraun-runner --version "=$GUNGRAUN_RUNNER_VERSION" --locked --root /usr/local \
 && rm -rf "$CARGO_HOME/registry" "$CARGO_HOME/git" \
 && chown -R ci "$CARGO_HOME" \
 && gungraun-runner --version

RUN sccache --version && valgrind --version && command -v mkarchiso repo-add namcap gpg
