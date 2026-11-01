#!/usr/bin/env bash
# Sign a release's Arch packages and write the signed repository tree with packaging/keyring/build-repo, in the pinned Arch
# image without a network:
#
#   CUPERTINO_SIGNING_KEY=<armored secret subkey export> packaging/ci/sign-release.sh PACKAGES_DIR URL OUT_DIR
#
# PACKAGES_DIR holds the built *.pkg.tar.zst, URL is the https base of the repository, OUT_DIR is a new directory that
# receives tree/ (see build-repo). The key reaches the container on stdin only. With no key, a key that does not import, a key
# without a signing subkey or one that holds any primary secret key (the master never goes to CI), nothing is signed and
# the exit status is not zero. Nothing is uploaded.
set -euo pipefail

[ "$#" -eq 3 ] || { echo 'usage: sign-release.sh PACKAGES_DIR URL OUT_DIR' >&2; exit 2; }
packages=$(cd "$1" && pwd)
url=$2
mkdir -p "$3"
out=$(cd "$3" && pwd)
[ -n "${CUPERTINO_SIGNING_KEY:-}" ] || { echo 'sign-release: CUPERTINO_SIGNING_KEY is empty; nothing is signed' >&2; exit 1; }
[ -n "$url" ] || { echo 'sign-release: the repository URL is empty; nothing is signed' >&2; exit 1; }
[ -z "$(ls -A "$out")" ] || { echo "sign-release: $out is not empty" >&2; exit 1; }

here=$(cd "$(dirname "$0")/../keyring" && pwd)
image=docker.io/library/archlinux@sha256:3cbf69d2d5958d6a294f3722c5668e6187c4df8e00fa27f72352f0bf83e495e1

inner=$(
    cat <<'INNER'
set -eu
export HOME=/tmp GNUPGHOME=/tmp/gnupg LC_ALL=C
mkdir -m 700 "$GNUPGHOME"
gpg --batch --quiet --import 2>/dev/null || { echo 'sign-release: the signing key does not import' >&2; exit 1; }
gpg --batch --with-colons --list-secret-keys | awk -F: '$1 == "sec" { n++; if ($15 != "#") bad = 1 } END { exit !(n > 0 && !bad) }' || { echo 'sign-release: the key holds a primary secret key; export only the signing subkey' >&2; exit 1; }
sub=$(gpg --batch --with-colons --list-secret-keys | awk -F: '$1 == "ssb" { want = ($12 ~ /s/ && $15 != "#"); next } want && $1 == "fpr" { print $10; exit }')
[ -n "$sub" ] || { echo 'sign-release: the key has no signing subkey' >&2; exit 1; }
sh /keyring/build-repo --packages /packages --key "$sub" --url "$URL" --out /out/tree
INNER
)

printf '%s\n' "$CUPERTINO_SIGNING_KEY" | docker run --rm -i --network none --user "$(id -u):$(id -g)" \
    -e URL="$url" -v "$packages:/packages:ro" -v "$here:/keyring:ro" -v "$out:/out" "$image" bash -c "$inner"
