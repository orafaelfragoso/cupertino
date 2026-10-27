# The renderer the golden baselines are recorded on: Rust and Debian are pinned by digest, and every package comes from one
# Debian snapshot, so Mesa and LLVM do not move until this file does. Change the digest, the snapshot and
# tests/fixtures/goldens/renderer.txt together, and re-record with ci/goldens.sh --update.
FROM rust:1.99.1-trixie@sha256:931c2c0531a8b143e044f0f3914dfe802f15bb3d1955f0b657e9359327ac2ffc

ARG SNAPSHOT=20261020T213255Z
RUN sed -i \
      -e "s|http://deb.debian.org/debian-security|http://snapshot.debian.org/archive/debian-security/${SNAPSHOT}|" \
      -e "s|http://deb.debian.org/debian\$|http://snapshot.debian.org/archive/debian/${SNAPSHOT}|" \
      /etc/apt/sources.list.d/debian.sources \
 && grep -q snapshot.debian.org /etc/apt/sources.list.d/debian.sources \
 && apt-get -o Acquire::Check-Valid-Until=false update \
 && apt-get install -y --no-install-recommends \
      build-essential pkg-config libgbm-dev libpam0g-dev dbus libgl1-mesa-dri libegl1 libegl-mesa0 xkb-data \
 && rm -rf /var/lib/apt/lists/* \
 && test -f /usr/share/glvnd/egl_vendor.d/50_mesa.json

# llvmpipe picks its vector width from the CPU; the baselines are rendered at one width on every host.
ENV LP_NATIVE_VECTOR_WIDTH=256
