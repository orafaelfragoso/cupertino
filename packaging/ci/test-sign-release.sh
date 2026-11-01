#!/usr/bin/env bash
# packaging/ci/sign-release.sh with a throwaway key, the way the release workflow calls it, without GitHub:
#
#   packaging/ci/test-sign-release.sh
#
# Checks: no key, a key with the master secret, a second full key next to the subkey, and an empty URL each sign nothing and fail; a signing subkey alone
# produces a tree whose database, package and SHA256SUMS signatures verify under the throwaway master.
set -euo pipefail

root=$(git -C "$(dirname "$0")" rev-parse --show-toplevel)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
export GNUPGHOME=$work/gnupg
mkdir -m 700 "$GNUPGHOME"
gen() { gpg --batch --quiet --pinentry-mode loopback --passphrase '' "$@"; }
gen --quick-gen-key 'Throwaway Master <t@example.invalid>' ed25519 cert 1d
master=$(gpg --batch --with-colons --list-keys | awk -F: '$1 == "fpr" { print $10; exit }')
gen --quick-add-key "$master" ed25519 sign 1d
gen --export-secret-subkeys --armor "$master" >"$work/subkey.asc"
gen --export-secret-keys --armor "$master" >"$work/master.asc"
gen --quick-gen-key 'Other <o@example.invalid>' ed25519 cert 1d
other=$(gpg --batch --with-colons --list-keys Other | awk -F: '$1 == "fpr" { print $10; exit }')
gen --export-secret-keys --armor "$other" >"$work/other.asc"

pass=0
ok() { pass=$((pass + 1)); echo "ok   $*"; }
fail() { echo "FAIL $*"; exit 1; }

mkdir "$work/pkgs" "$work/pkg"
printf '%s\n' 'pkgname = cupertino' 'pkgver = 1.0-1' 'pkgdesc = stand-in' 'size = 0' 'arch = x86_64' 'builddate = 1' >"$work/pkg/.PKGINFO"
(cd "$work/pkg" && tar --zstd -cf "$work/pkgs/cupertino-1.0-1-x86_64.pkg.tar.zst" .PKGINFO)
url=https://example.invalid/arch/cupertino/x86_64
S=$root/packaging/ci/sign-release.sh

refused() {
    local name=$1 out=$2
    shift 2
    if "$@" >"$work/refused.out" 2>&1; then cat "$work/refused.out"; fail "$name was accepted"; fi
    [ -z "$(ls -A "$out")" ] || fail "$name left files in the output directory"
    ok "$name is refused: $(tail -n 1 "$work/refused.out")"
}

refused 'no signing key' "$work/o1" env CUPERTINO_SIGNING_KEY= "$S" "$work/pkgs" "$url" "$work/o1"
refused 'a key that holds the master secret' "$work/o2" env CUPERTINO_SIGNING_KEY="$(cat "$work/master.asc")" "$S" "$work/pkgs" "$url" "$work/o2"
refused 'a second full key next to the subkey' "$work/o6" env CUPERTINO_SIGNING_KEY="$(cat "$work/subkey.asc" "$work/other.asc")" "$S" "$work/pkgs" "$url" "$work/o6"
refused 'an empty URL' "$work/o3" env CUPERTINO_SIGNING_KEY="$(cat "$work/subkey.asc")" "$S" "$work/pkgs" '' "$work/o3"
refused 'a key that is not a key' "$work/o4" env CUPERTINO_SIGNING_KEY=garbage "$S" "$work/pkgs" "$url" "$work/o4"

CUPERTINO_SIGNING_KEY="$(cat "$work/subkey.asc")" "$S" "$work/pkgs" "$url" "$work/o5" >/dev/null
t=$work/o5/tree
for f in repo/cupertino-1.0-1-x86_64.pkg.tar.zst repo/cupertino.db.tar.zst repo/cupertino.files.tar.zst SHA256SUMS; do
    gpg --batch --status-fd 1 --verify "$t/$f.sig" "$t/$f" 2>/dev/null | awk '$2 == "VALIDSIG" { print toupper($NF) }' | grep -qx "$master" || fail "$f: no valid signature under the master"
done
(cd "$t" && sha256sum -c --quiet SHA256SUMS) || fail 'SHA256SUMS does not match the tree'
ok 'a signing subkey alone signs the packages, the database and SHA256SUMS under the master'
echo "sign-release test: $pass checks passed"
