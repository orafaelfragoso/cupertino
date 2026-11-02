#!/usr/bin/env bash
# The local signed repository of an installed machine, in the pinned Arch image with a throwaway key: a [cupertino] section
# with Server = file://..., SigLevel = Required DatabaseOptional, as cupertino-install writes it.
#
#   packaging/ci/test-local-repo.sh [OUT_DIR]      the log goes to OUT_DIR/local-repo-test.log
#
# Checks: local-release keygen keeps the master secret out of the signing home and never overwrites; local-release refuses a
# home that holds the master; pacman -Sy works against the empty database of a first boot; cupertino-keyring from the first
# release trusts the key; an update from the file:// repository installs; a tampered package, a package without its
# signature and a package signed by another key are each refused and leave the installed version alone.
set -euo pipefail

root=$(git -C "$(dirname "$0")" rev-parse --show-toplevel)
out=${1:-$HOME/.cache/cupertino-local-repo-test}
image=docker.io/library/archlinux@sha256:3cbf69d2d5958d6a294f3722c5668e6187c4df8e00fa27f72352f0bf83e495e1
mkdir -p "$out"
out=$(cd "$out" && pwd)
work=$(mktemp -d "$out/work.XXXXXX")
trap 'docker run --rm -v "$work:/w" "$image" rm -rf /w/build /w/pkgs1 /w/pkgs2 /w/pkgs3 /w/local >/dev/null 2>&1 || true; rm -rf "$work"' EXIT
git -C "$root" archive --format=tar --prefix=src/ HEAD packaging/keyring packaging/ci/keyring-test-lib.sh | tar -x -C "$work"

test_script=$(
    cat <<'INNER'
set -euo pipefail
. /work/src/packaging/ci/keyring-test-lib.sh
prepare
LR="bash /work/src/packaging/keyring/local-release"
H=/home/builder
L=/work/local

as_builder "$LR keygen --master-home $H/master --signing-home $H/signing --no-passphrase" >/dev/null 2>&1 || fail 'local-release keygen'
as_builder "gpg --homedir $H/signing --list-secret-keys" 2>/dev/null | grep -q '^sec#' || fail 'the signing home has no stub of the master'
! as_builder "gpg --homedir $H/signing --with-colons --list-secret-keys" 2>/dev/null | awk -F: '$1 == "sec" && $15 != "#" { f = 1 } END { exit !f }' || fail 'the signing home holds the master secret'
[ -f $H/signing/PUBLIC.asc ] || fail 'no PUBLIC.asc'
refused 'keygen over an existing key' as_builder "$LR keygen --master-home $H/master --signing-home $H/signing2 --no-passphrase"
cp $H/signing/PUBLIC.asc $H/master/; refused 'a release signed from the home that holds the master' as_builder "$LR --signing-home $H/master --repo-root $L --packages /work/pkgs1"
ok 'keygen: the signing home has the subkey and the public key, not the master; it never overwrites'

# the keyring package carries the new public key; the three releases are three version pairs
cp $H/signing/PUBLIC.asc /work/build/cupertino-public.asc
mk_keyring /work/pkgs1
for p in cupertino cupertino-data; do mk "$p" 1.0-1 /work/pkgs1; mk "$p" 2.0-1 /work/pkgs2; mk "$p" 3.0-1 /work/pkgs3; done
for d in pkgs2 pkgs3; do mkdir -p /work/$d; cp /work/pkgs1/cupertino-keyring-1-1-any.pkg.tar.zst /work/$d/; done
install -d -o builder $L $L/repo $L/archive
chmod 755 /work $L $L/repo $L/archive

# the first boot: the installer's empty database and its section
python3 -c 'import tarfile; tarfile.open("'$L'/repo/cupertino.db", "w:gz").close()'
chown builder $L/repo/cupertino.db
printf '\n[cupertino]\nSigLevel = Required DatabaseOptional\nServer = file://%s/repo\n' $L >>/etc/pacman.conf
pacman-key --init >/dev/null 2>&1
pacman-key --populate archlinux >/dev/null 2>&1
no_keyserver
pacman -Sy --noconfirm >/dev/null 2>&1 || fail 'pacman -Sy fails against the empty database of a first boot'
ok 'pacman -Sy works against the empty database of a first boot'

# the installer installs the keyring from the live USB (unsigned, local file); here the first release has it
pacman -U --noconfirm /work/pkgs1/cupertino-keyring-1-1-any.pkg.tar.zst >/dev/null 2>&1 || fail 'the keyring does not install'
pacman-key --list-keys 2>/dev/null | grep -qi "$(gpg --homedir $H/signing --with-colons --list-keys 2>/dev/null | awk -F: '$1 == "pub" { p = 1 } p && $1 == "fpr" { print $10; exit }')" || fail 'the Cupertino key is not in the pacman keyring'

as_builder "$LR --signing-home $H/signing --repo-root $L --packages /work/pkgs1" >/tmp/lr.out 2>&1 || { cat /tmp/lr.out; fail 'release 1'; }
pacman -Sy --noconfirm >/dev/null 2>&1 || fail 'pacman -Sy after release 1'
pacman -S --noconfirm cupertino cupertino-data >/dev/null 2>&1 || fail 'release 1 does not install from the local repository'
[ "$(installed cupertino)" = 1.0-1 ] || fail '1.0-1 expected'
ok 'release 1 installs from the file:// repository with SigLevel Required'

as_builder "$LR --signing-home $H/signing --repo-root $L --packages /work/pkgs2" >/tmp/lr.out 2>&1 || { cat /tmp/lr.out; fail 'release 2'; }
pacman -Sy --noconfirm >/dev/null 2>&1 || fail 'pacman -Sy after release 2'
pacman -S --noconfirm cupertino cupertino-data >/dev/null 2>&1 || fail 'the update does not install'
[ "$(installed cupertino)" = 2.0-1 ] && [ "$(installed cupertino-data)" = 2.0-1 ] || fail '2.0-1 expected after the update'
[ -f $L/archive/cupertino-1.0-1-x86_64.pkg.tar.zst.sig ] || fail 'release 1 is not in the archive'
ok 'release 2 updates cupertino and cupertino-data from the local repository; release 1 is in the archive'

fresh() { rm -f /var/cache/pacman/pkg/cupertino-3.0-1*; }
# a third release, then each way of breaking it
as_builder "$LR --signing-home $H/signing --repo-root $L --packages /work/pkgs3" >/tmp/lr.out 2>&1 || { cat /tmp/lr.out; fail 'release 3'; }
pkg=$L/repo/cupertino-3.0-1-x86_64.pkg.tar.zst
cp $pkg /work/pkg.orig; cp $pkg.sig /work/sig.orig
tamper $pkg
fresh
refused 'a tampered package from the local repository' bash -c 'pacman -Sy --noconfirm && pacman -S --noconfirm cupertino'
[ "$(installed cupertino)" = 2.0-1 ] || fail 'a refusal changed the installed version'
cp /work/pkg.orig $pkg; rm -f $pkg.sig
fresh
refused 'a package without its signature' bash -c 'pacman -Sy --noconfirm && pacman -S --noconfirm cupertino'
cp /work/sig.orig $pkg.sig
rm -f $pkg.sig; sign $pkg "$OTHER"
fresh
refused 'a package signed by another key' bash -c 'pacman -Sy --noconfirm && pacman -S --noconfirm cupertino'
cp /work/pkg.orig $pkg; cp /work/sig.orig $pkg.sig
fresh
pacman -Sy --noconfirm >/dev/null 2>&1 && pacman -S --noconfirm cupertino cupertino-data >/dev/null 2>&1 || fail 'the untampered release 3 does not install'
[ "$(installed cupertino)" = 3.0-1 ] || fail '3.0-1 expected'
ok 'the untampered release 3 installs after the three refusals'
echo "local repo test: $pass checks passed"
INNER
)

timeout 1500 docker run --rm -v "$work:/work" "$image" bash -c "$test_script" 2>&1 | tee "$out/local-repo-test.log"
test "${PIPESTATUS[0]}" -eq 0
