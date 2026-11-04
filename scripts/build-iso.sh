#!/usr/bin/env bash
# Build the Cupertino live and install ISO from the committed HEAD of this checkout.
#
#   scripts/build-iso.sh [OUT_DIR]        OUT_DIR defaults to ~/.cache/cupertino-iso
#
# Packages come from packaging/ci/build-in-container.sh arch (pinned Arch image), go into a local pacman repository, and
# mkarchiso (pinned Arch image, privileged) assembles packaging/iso around it. The Cupertino packages follow from the HEAD
# and the pinned image; the rest comes from the current Arch mirrors, so the manifest records every installed package
# version. SOURCE_DATE_EPOCH is the HEAD commit time. The output holds the ISO, its sha256 and the manifest. Needs docker and git; refuses to start below the build-root disk floor.
set -euo pipefail

root=$(git -C "$(dirname "$0")" rev-parse --show-toplevel)
out=${1:-$HOME/.cache/cupertino-iso}
image=docker.io/library/archlinux@sha256:3cbf69d2d5958d6a294f3722c5668e6187c4df8e00fa27f72352f0bf83e495e1
floor_gb=${CUPERTINO_ISO_FLOOR_GB:-30}
need_gb=$((floor_gb + 18))

mkdir -p "$out"
out=$(cd "$out" && pwd)
free_gb=$(df -BG --output=avail "$out" | tail -1 | tr -dc 0-9)
if [ "$free_gb" -lt "$need_gb" ]; then
    echo "only ${free_gb} GB free in $out; the build needs $need_gb GB (floor $floor_gb GB + 18 GB)" >&2
    exit 1
fi

if [ -n "$(git -C "$root" status --porcelain)" ]; then
    echo "the checkout has uncommitted changes; the ISO is built from a clean tree" >&2
    exit 1
fi
head=$(git -C "$root" rev-parse HEAD)
epoch=$(git -C "$root" log -1 --format=%ct HEAD)
work=$(mktemp -d "$out/work.XXXXXX")
trap 'docker run --rm -v "$work:/w" "$image" rm -rf /w/iso-work /w/iso-out >/dev/null 2>&1 || true; rm -rf "$work"' EXIT

# The installed system trusts packages signed by this key (cupertino-keyring); the public key comes from `local-release keygen`.
keyring_key=${CUPERTINO_KEYRING_KEY:-}
[ -f "$keyring_key" ] || { echo 'set CUPERTINO_KEYRING_KEY to the public key file (PUBLIC.asc from packaging/keyring/local-release keygen)' >&2; exit 1; }
# ...and only the release key whose fingerprint is in the tree (packaging/keyring/release-key.fpr), not whatever a variable holds.
"$root/packaging/keyring/check-key" "$keyring_key"

mkdir -p "$work/pkgs"
"$root/packaging/ci/build-in-container.sh" arch "$work/pkgs"
"$root/packaging/keyring/build-keyring" "$keyring_key" "$work/pkgs" >/dev/null
rm -rf "$work/pkgs/rev-"* "$work/pkgs"/*.log "$work/pkgs"/*.info "$work/pkgs"/*.contents "$work/pkgs/PKGBUILD"

cp -a "$root/packaging/iso" "$work/profile"
# Symlinks cannot live in the source tree (ci/gate.py); airootfs-links lists them as "path target" lines.
while read -r link target; do
    mkdir -p "$work/profile/airootfs/$(dirname "$link")"
    ln -sfn "$target" "$work/profile/airootfs/$link"
done <"$work/profile/airootfs-links"
rm "$work/profile/airootfs-links"
mkdir -p "$work/profile/airootfs/home/live" "$work/profile/airootfs/usr/local/bin"
install -m 0755 "$root/packaging/installer/cupertino_install.py" "$work/profile/airootfs/usr/local/bin/cupertino-install"
cp "$work/profile/pacman.conf" "$work/profile/airootfs/etc/pacman.conf"
mkdir -p "$work/profile/airootfs/opt/cupertino-repo"
cp "$work/pkgs"/*.pkg.tar.zst "$work/profile/airootfs/opt/cupertino-repo/"

docker run --rm --privileged -v "$work:/w" -e SOURCE_DATE_EPOCH="$epoch" -e OUT_UID="$(id -u)" -e OUT_GID="$(id -g)" "$image" bash -euc '
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
'

iso=$(ls "$work"/iso-out/*.iso)
name=cupertino-$(date -u --date="@$epoch" +%Y.%m.%d)-${head:0:8}-x86_64.iso
mv "$iso" "$out/$name"
(cd "$out" && sha256sum "$name" >"$name.sha256")
{
    echo "head $head"
    echo "source-date-epoch $epoch"
    cat "$work/archiso-version"
    ls "$work/pkgs"
    echo "installed packages:"
    cat "$work/iso-work/iso/arch/pkglist.x86_64.txt"
} >"$out/$name.manifest"
echo "$out/$name"
