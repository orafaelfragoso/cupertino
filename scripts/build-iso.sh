#!/usr/bin/env bash
# Build the Cupertino live and install ISO from the committed HEAD of this checkout.
#
#   scripts/build-iso.sh [OUT_DIR] [--packages DIR]   OUT_DIR defaults to ~/.cache/cupertino-iso
#
# Packages come from packaging/ci/build-in-container.sh arch (pinned Arch image), go into a local pacman repository, and
# mkarchiso (pinned Arch image, privileged) assembles packaging/iso around it. The Cupertino packages follow from the HEAD
# and the pinned image; the rest comes from the current Arch mirrors, so the manifest records every installed package
# version. SOURCE_DATE_EPOCH is the HEAD commit time. The output holds the ISO, its sha256 and the manifest. Needs docker and git; refuses to start below the build-root disk floor.
set -euo pipefail

root=$(git -C "$(dirname "$0")" rev-parse --show-toplevel)
usage() { echo 'usage: build-iso.sh [OUT_DIR] [--packages DIR]' >&2; exit 2; }
out=
packages=
while [ "$#" -gt 0 ]; do
    case $1 in
    --packages)
        [ "$#" -ge 2 ] && [ -z "$packages" ] && [ -n "$2" ] && [[ "$2" != --* ]] || usage
        packages=$2
        shift 2
        ;;
    --*) usage ;;
    *)
        [ -z "$out" ] && [ -n "$1" ] || usage
        out=$1
        shift
        ;;
    esac
done
if [ -z "$out" ]; then out=$HOME/.cache/cupertino-iso; fi
# Refuse missing key input before creating a build tree or invoking Docker cleanup.
keyring_key=${CUPERTINO_KEYRING_KEY:-}
[ -f "$keyring_key" ] || { echo 'set CUPERTINO_KEYRING_KEY to the public key file (PUBLIC.asc from packaging/keyring/local-release keygen)' >&2; exit 1; }
keyring_key=$(realpath "$keyring_key")
export CUPERTINO_KEYRING_KEY=$keyring_key
image=docker.io/library/archlinux@sha256:3cbf69d2d5958d6a294f3722c5668e6187c4df8e00fa27f72352f0bf83e495e1
floor_gb=${CUPERTINO_ISO_FLOOR_GB:-30}
[[ "$floor_gb" =~ ^[0-9]+$ ]] || { echo 'CUPERTINO_ISO_FLOOR_GB must be a non-negative whole number' >&2; exit 2; }
need_gb=$((10#$floor_gb + 18))
need_bytes=$((need_gb * 1024 * 1024 * 1024))

mkdir -p "$out"
out=$(cd "$out" && pwd)
if [ -n "$packages" ]; then packages=$(cd "$packages" && pwd); fi
cd "$root"
free_bytes=$(df -B1 --output=avail "$out" | tail -1 | tr -dc 0-9)
if [ "$free_bytes" -lt "$need_bytes" ]; then
    echo "only $free_bytes bytes free in $out; the build needs $need_gb GiB (floor $floor_gb GiB + 18 GiB)" >&2
    exit 1
fi

if [ -n "$(git -C "$root" status --porcelain)" ]; then
    echo "the checkout has uncommitted changes; the ISO is built from a clean tree" >&2
    exit 1
fi
head=$(git -C "$root" rev-parse HEAD)
epoch=$(git -C "$root" log -1 --format=%ct "$head")
work=$(mktemp -d "$out/work.XXXXXX")
active_job=
cleanup() {
    local status=$? cleanup_status=0
    trap - EXIT
    trap "" INT TERM
    if [ -s "$work/iso.cid" ]; then
        if ! docker rm --force "$(cat "$work/iso.cid")" >/dev/null; then cleanup_status=1; fi
    fi
    if [ -n "$active_job" ]; then
        if kill -0 "$active_job" 2>/dev/null; then
            kill -TERM "$active_job" || cleanup_status=1
        fi
        if wait "$active_job"; then :; fi
    fi
    if [ -e "$work/iso-work" ] || [ -e "$work/iso-out" ]; then
        if ! docker run --rm -v "$work:/w" "$image" rm -rf /w/iso-work /w/iso-out >/dev/null; then cleanup_status=1; fi
    fi
    if ! rm -rf "$work"; then cleanup_status=1; fi
    if [ "$status" = 0 ]; then status=$cleanup_status; fi
    exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

run_job() {
    local status
    "$@" &
    active_job=$!
    if wait "$active_job"; then status=0; else status=$?; fi
    active_job=
    return "$status"
}

# Profile, installer and trust policy must all come from the captured commit, even if HEAD changes during packaging.
source_root=$work/source
mkdir "$source_root"
git -C "$root" archive "$head" packaging/iso packaging/installer packaging/keyring | tar -x -C "$source_root"
cp "$keyring_key" "$work/public.asc"
keyring_key=$work/public.asc
"$source_root/packaging/keyring/check-key" "$keyring_key" "$source_root/packaging/keyring/release-key.fpr"

mkdir -p "$work/pkgs"
tag=()
if [ -n "${RELEASE_TAG:-}" ]; then tag=(--tag "$RELEASE_TAG"); fi
if [ -n "$packages" ]; then
    python3 "$root/packaging/release.py" verify-packages "$packages" "${tag[@]}" --commit "$head" --copy-to "$work/pkgs"
else
    run_job "$root/packaging/ci/build-in-container.sh" arch "$work/built-pkgs"
    python3 "$root/packaging/release.py" verify-packages "$work/built-pkgs" "${tag[@]}" --commit "$head" --copy-to "$work/pkgs"
fi
run_job "$source_root/packaging/keyring/build-keyring" "$keyring_key" "$work/pkgs" >/dev/null
rm -rf "$work/pkgs/rev-"* "$work/pkgs"/*.log "$work/pkgs"/*.info "$work/pkgs"/*.contents "$work/pkgs/PKGBUILD"

cp -a "$source_root/packaging/iso" "$work/profile"
# Symlinks cannot live in the source tree (ci/gate.py); airootfs-links lists them as "path target" lines.
while read -r link target; do
    mkdir -p "$work/profile/airootfs/$(dirname "$link")"
    ln -sfn "$target" "$work/profile/airootfs/$link"
done <"$work/profile/airootfs-links"
rm "$work/profile/airootfs-links"
mkdir -p "$work/profile/airootfs/home/live" "$work/profile/airootfs/usr/local/bin"
install -m 0755 "$source_root/packaging/installer/cupertino_install.py" "$work/profile/airootfs/usr/local/bin/cupertino-install"
cp "$work/profile/pacman.conf" "$work/profile/airootfs/etc/pacman.conf"
mkdir -p "$work/profile/airootfs/opt/cupertino-repo"
cp "$work/pkgs"/*.pkg.tar.zst "$work/profile/airootfs/opt/cupertino-repo/"

container=$(docker create --cidfile "$work/iso.cid" --privileged -v "$work:/w" -e SOURCE_DATE_EPOCH="$epoch" -e OUT_UID="$(id -u)" -e OUT_GID="$(id -g)" "$image" bash -euc '
    pacman -Syu --noconfirm --needed -q archiso >/dev/null
    mkdir -p /opt/cupertino-repo
    cp /w/profile/airootfs/opt/cupertino-repo/*.pkg.tar.zst /opt/cupertino-repo/
    repo-add -q /opt/cupertino-repo/cupertino.db.tar.zst /opt/cupertino-repo/*.pkg.tar.zst
    cp /opt/cupertino-repo/cupertino.db.tar.zst /opt/cupertino-repo/cupertino.files.tar.zst /w/profile/airootfs/opt/cupertino-repo/
    ln -sf cupertino.db.tar.zst /w/profile/airootfs/opt/cupertino-repo/cupertino.db
    ln -sf cupertino.files.tar.zst /w/profile/airootfs/opt/cupertino-repo/cupertino.files
    pacman -Q archiso | tee /w/archiso-version
    mkarchiso -v -w /w/iso-work -o /w/iso-out /w/profile
    chown -R "$OUT_UID:$OUT_GID" /w
')
run_job docker start --attach "$container"

iso=$(ls "$work"/iso-out/*.iso)
name=cupertino-$(date -u --date="@$epoch" +%Y.%m.%d)-${head:0:8}-x86_64.iso
mv "$iso" "$out/$name"
(cd "$out" && sha256sum "$name" >"$name.sha256")
{
    echo "head $head"
    echo "source-date-epoch $epoch"
    cat "$work/pkgs/packages.json"
    cat "$work/archiso-version"
    ls "$work/pkgs"
    echo "installed packages:"
    cat "$work/iso-work/iso/arch/pkglist.x86_64.txt"
} >"$out/$name.manifest"
echo "$out/$name"
