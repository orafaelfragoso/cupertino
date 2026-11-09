# The Arch package builder. Package dependencies and release tools are image layers; source and Cargo outputs are
# mounted only at run time.
FROM docker.io/library/archlinux@sha256:3cbf69d2d5958d6a294f3722c5668e6187c4df8e00fa27f72352f0bf83e495e1 AS arch
ARG CARGO_BUILD_JOBS=4
ENV RUSTUP_HOME=/opt/rustup CARGO_HOME=/opt/cargo CARGO_INCREMENTAL=0 RUSTUP_TOOLCHAIN=1.99.1
ENV CARGO_BUILD_JOBS=$CARGO_BUILD_JOBS
ENV PATH=/opt/cargo/bin:$PATH
RUN pacman -Syu --noconfirm --needed -q rustup python mesa libglvnd pam dbus xkeyboard-config \
      shared-mime-info gcc-libs systemd adwaita-icon-theme clang libpipewire pipewire namcap git \
 && rustup toolchain install 1.99.1 --profile minimal \
 && rustup default 1.99.1 \
 && useradd -m builder \
 && install -d -o builder /home/builder/nextest /opt/cargo \
 && pacman -Scc --noconfirm

# makepkg checks its dependencies in pacman's database, so the verified prebuilt runner is a real package.
RUN <<'SHELL'
set -eu
cat > /home/builder/nextest/PKGBUILD <<'PKGBUILD'
pkgname=cargo-nextest
pkgver=0.9.151
pkgrel=1
pkgdesc='A next-generation Rust test runner'
arch=('x86_64')
url='https://nexte.st/'
license=('MIT' 'Apache-2.0')
depends=('cargo')
options=('!strip' '!debug')
source=("https://github.com/nextest-rs/nextest/releases/download/cargo-nextest-$pkgver/cargo-nextest-$pkgver-x86_64-unknown-linux-musl.tar.gz")
sha256sums=('6b284d6fde9670dfb6ee10ea9d9abbc85f95643d3f592472d420d82e257b3a6c')
package() {
  install -Dm755 cargo-nextest "$pkgdir/usr/bin/cargo-nextest"
}
PKGBUILD
chown builder /home/builder/nextest/PKGBUILD
su builder -c 'cd /home/builder/nextest && makepkg --noconfirm'
pacman -U --noconfirm /home/builder/nextest/cargo-nextest-*.pkg.tar.zst
cargo nextest --version
rm -rf /home/builder/nextest
SHELL
