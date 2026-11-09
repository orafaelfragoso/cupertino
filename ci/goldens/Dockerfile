# The renderer the golden baselines are recorded on: Arch is pinned by digest (archlinux:base-devel-20261101.0.620205),
# and every package comes from one Arch Linux Archive date, so Mesa and LLVM do not move until this file does. Change the
# digest, the date and tests/fixtures/goldens/renderer.txt together, and re-record with ci/goldens.sh --update (or the
# Record goldens workflow); see CONTRIBUTING.md.
FROM docker.io/library/archlinux:base-devel@sha256:2de80105abb49f813a85393c6758a4334deb6bcec4a2411bd2f059c0bee6ee4c

ARG ARCHIVE=2026/11/01
# sccache is used only when ci/goldens.sh mounts its per-user compile cache.
# The archive date is the only package source: the mirror list holds that one server, pacman.conf names no server of
# its own and every repository includes the mirror list. -Syyuu moves every installed package to that date, down too.
# The downloaded packages are removed by hand: `pacman -Scc --noconfirm` answers no to removing them.
RUN printf 'Server = https://archive.archlinux.org/repos/%s/$repo/os/$arch\n' "$ARCHIVE" > /etc/pacman.d/mirrorlist \
 && [ "$(grep -c '^[[:space:]]*Server' /etc/pacman.d/mirrorlist)" = 1 ] \
 && grep -qxF "Server = https://archive.archlinux.org/repos/$ARCHIVE/\$repo/os/\$arch" /etc/pacman.d/mirrorlist \
 && ! grep -q '^[[:space:]]*Server' /etc/pacman.conf \
 && [ -z "$(grep '^[[:space:]]*Include' /etc/pacman.conf | grep -v '= /etc/pacman.d/mirrorlist$')" ] \
 && pacman -Syyuu --noconfirm --needed \
      rustup python mesa libglvnd pam dbus xkeyboard-config clang libpipewire sccache \
 && rm -rf /var/cache/pacman/pkg/* \
 && test -f /usr/share/glvnd/egl_vendor.d/50_mesa.json

# The toolchain is read-only for the unprivileged user ci/goldens.sh runs the tests as.
ENV RUSTUP_HOME=/opt/rustup RUSTUP_TOOLCHAIN=1.99.1
RUN rustup toolchain install 1.99.1 --profile minimal \
 && chmod -R a+rX /opt/rustup \
 && cargo --version

# llvmpipe picks its vector width from the CPU; the baselines are rendered at one width on every host.
ENV LP_NATIVE_VECTOR_WIDTH=256
