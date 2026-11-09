#!/usr/bin/env bash
# The Arch packages that CI builds and tests the workspace with, in one place for the workflows and for contributors.
#
#   ci/arch-ci.sh list GROUP...      print the packages of the groups, one per line
#   ci/arch-ci.sh install GROUP...   install them with pacman (as root: the CI container, or `sudo` on your machine)
#   ci/arch-ci.sh user               CI container only: create the unprivileged user `ci` that builds and runs the tests,
#                                    or give it back what root steps (checkout, cache restore) wrote; safe to repeat
#
# Groups:
#   build   the PKGBUILD's makedepends (cargo comes from rustup, which installs the pinned toolchain) and git and jq
#   test    build, plus what the tests start: dbus-daemon, the PipeWire daemon, gnome-keyring and secret-tool
#   weston  test, plus Weston (the second compositor of the transport's simple-window test)
#   gate    the policy and packaging script tests: Python, git, gpg, the session bus and the keyring daemon
#
# On your machine: `sudo pacman -S --needed $(ci/arch-ci.sh list test)`.
#
# `user` makes `ci` with the uid that owns $RUNNER_TEMP (the runner's own user, so the step scripts and the files the
# runner passes to steps stay writable), gives it the checkout, CARGO_HOME and RUSTUP_HOME (default ~ci/.cargo and
# ~ci/.rustup) and a runtime directory, and exports those three to the later steps. Actions run as root in a container job, so run it again after a step that writes
# there as root. The tests must not run as root: several check that a file is owned by root or unreadable to the caller.
set -euo pipefail

usage="usage: ci/arch-ci.sh list|install GROUP... | ci/arch-ci.sh user"

packages() {
    local group
    for group in "$@"; do
        case $group in
        build)
            # The PKGBUILD's makedepends, in its order, then the CI tools.
            printf '%s\n' base-devel python mesa libglvnd pam gcc-libs systemd dbus xkeyboard-config shared-mime-info \
                adwaita-icon-theme clang libpipewire rustup git jq zstd
            ;;
        test)
            packages build
            # The shells and mandoc check the completions and manual pages of `cupertino`.
            printf '%s\n' pipewire gnome-keyring libsecret bash-completion zsh fish mandoc
            ;;
        weston)
            packages test
            printf '%s\n' weston
            ;;
        gate)
            printf '%s\n' python git gnupg dbus systemd gnome-keyring libsecret rustup
            ;;
        *)
            echo "ci/arch-ci.sh: unknown group: $group" >&2
            echo "$usage" >&2
            exit 2
            ;;
        esac
    done
}

command=${1:-}
[ "$#" -gt 0 ] && shift
case $command in
list)
    [ "$#" -gt 0 ] || { echo "$usage" >&2; exit 2; }
    packages "$@" | awk '!seen[$0]++'
    ;;
install)
    [ "$#" -gt 0 ] || { echo "$usage" >&2; exit 2; }
    # A command substitution, not a process substitution, so that an unknown group stops the script before pacman.
    list=$(packages "$@" | awk '!seen[$0]++')
    mapfile -t wanted <<<"$list"
    pacman -Syu --noconfirm --needed "${wanted[@]}"
    ;;
user)
    [ "$#" -eq 0 ] || { echo "$usage" >&2; exit 2; }
    : "${RUNNER_TEMP:?ci/arch-ci.sh user runs in a CI container job only}"
    : "${GITHUB_WORKSPACE:?ci/arch-ci.sh user runs in a CI container job only}"
    # Cargo and rustup live in the user's home; the job's later steps (the cache action too) read them from GITHUB_ENV.
    CARGO_HOME=${CARGO_HOME:-/home/ci/.cargo}
    RUSTUP_HOME=${RUSTUP_HOME:-/home/ci/.rustup}
    uid=$(stat -c %u "$RUNNER_TEMP")
    gid=$(stat -c %g "$RUNNER_TEMP")
    if [ "$uid" = 0 ]; then
        echo "ci/arch-ci.sh: $RUNNER_TEMP is owned by root; the tests cannot run as root" >&2
        exit 1
    fi
    if ! getent group "$gid" >/dev/null; then groupadd --gid "$gid" ci; fi
    if ! id ci >/dev/null 2>&1; then useradd --create-home --uid "$uid" --gid "$gid" ci; fi
    mkdir -p "$CARGO_HOME" "$RUSTUP_HOME"
    # actions/checkout and the cache restore run as root in the container; the build writes in all three.
    chown -R ci:"$gid" "$GITHUB_WORKSPACE" "$CARGO_HOME" "$RUSTUP_HOME"
    safe=$(runuser -u ci -- git config --global --get-all safe.directory || true)
    if ! printf '%s\n' "$safe" | grep -qxF "$GITHUB_WORKSPACE"; then
        runuser -u ci -- git config --global --add safe.directory "$GITHUB_WORKSPACE"
    fi
    # The compositor tests open a Wayland socket in the runtime directory; a container has none of its own.
    install -d -m 700 -o ci -g "$gid" "/run/user/$uid"
    if [ -n "${GITHUB_ENV:-}" ]; then
        printf '%s\n' "CARGO_HOME=$CARGO_HOME" "RUSTUP_HOME=$RUSTUP_HOME" "XDG_RUNTIME_DIR=/run/user/$uid" >> "$GITHUB_ENV"
    fi
    id ci
    ;;
*)
    echo "$usage" >&2
    exit 2
    ;;
esac
