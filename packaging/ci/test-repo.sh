#!/usr/bin/env bash
# packaging/keyring/build-repo with a throwaway key, in the pinned Arch image: the staging tree it writes is served over
# http://127.0.0.1 and used by pacman with signature checking on.
#
#   packaging/ci/test-repo.sh [OUT_DIR]      the log goes to OUT_DIR/repo-test.log
#
# Checks: the tree (signed packages and database, no symlinks, SHA256SUMS, archive pruned to N); a pacman that does not trust
# the key refuses the repository; with the key trusted, a tampered database signature, a tampered database and a tampered
# package each fail, and an untampered repository installs; cupertino-rollback installs the earlier release from the archive
# and refuses a tampered archive package; SHA256SUMS.sig verifies under the master key; build-repo refuses a tampered,
# unsigned or foreign-signed --archive-from package before writing anything, and refuses a bad URL, a missing key and a non-empty output directory.
set -euo pipefail

root=$(git -C "$(dirname "$0")" rev-parse --show-toplevel)
out=${1:-$HOME/.cache/cupertino-keyring-test}
image=docker.io/library/archlinux@sha256:3cbf69d2d5958d6a294f3722c5668e6187c4df8e00fa27f72352f0bf83e495e1
mkdir -p "$out"
out=$(cd "$out" && pwd)
work=$(mktemp -d "$out/work.XXXXXX")
trap 'docker run --rm -v "$work:/w" "$image" rm -rf /w/build /w/pkgs1 /w/pkgs2 /w/stage1 /w/stage2 /w/stage3 /w/tamper /w/bad >/dev/null 2>&1 || true; rm -rf "$work"' EXIT
git -C "$root" archive --format=tar --prefix=src/ HEAD packaging/keyring packaging/ci/keyring-test-lib.sh | tar -x -C "$work"

test_script=$(
    cat <<'INNER'
set -euo pipefail
. /work/src/packaging/ci/keyring-test-lib.sh
prepare
BUILD="sh /work/src/packaging/keyring/build-repo"
URL=http://127.0.0.1:8000

# release 1 and release 2 (with the keyring package), unsigned, as the build leaves them
for p in cupertino cupertino-data; do mk "$p" 1.0-1 /work/pkgs1; mk "$p" 2.0-1 /work/pkgs2; done
mk_keyring /work/pkgs2

as_builder "$BUILD --packages /work/pkgs1 --key $SUB --url $URL --out /work/stage1" >/dev/null
as_builder "$BUILD --packages /work/pkgs2 --key $SUB --url $URL --out /work/stage2 --archive-from /work/stage1/archive" >/dev/null
S=/work/stage2

# the tree
for f in cupertino.db cupertino.db.sig cupertino.files cupertino.files.sig cupertino.db.tar.zst.sig; do
    [ -f "$S/repo/$f" ] && [ ! -L "$S/repo/$f" ] || fail "repo/$f is not a regular file"
done
[ -z "$(find "$S" -type l)" ] || fail 'the tree has symlinks'
for f in "$S"/repo/*.pkg.tar.zst "$S"/archive/*.pkg.tar.zst; do [ -f "$f.sig" ] || fail "$f has no .sig"; done
[ "$(ls "$S/repo"/*.pkg.tar.zst | wc -l)" -eq 3 ] || fail 'repo/ holds the three packages of release 2'
for v in 1.0-1 2.0-1; do for n in cupertino cupertino-data; do [ -f "$S/archive/$n-$v-x86_64.pkg.tar.zst" ] || fail "archive/ lacks $n $v"; done; done
(cd $S && sha256sum -c --quiet SHA256SUMS) || fail 'SHA256SUMS does not match the tree'
grep -qx "Server = $URL/repo" "$S/cupertino.conf" && grep -qx 'SigLevel = Required DatabaseRequired TrustedOnly' "$S/cupertino.conf" || fail 'cupertino.conf'
[ "$(cat "$S/cupertino-rollback.conf")" = "archive=$URL/archive" ] || fail 'cupertino-rollback.conf'
ok 'the staging tree: signed packages and database, plain files, SHA256SUMS, cupertino.conf, rollback conf'

# the archive is pruned to N
as_builder "$BUILD --packages /work/pkgs2 --key $SUB --url $URL --out /work/stage3 --archive-from $S/archive --keep 1" >/dev/null
[ "$(ls /work/stage3/archive/*.pkg.tar.zst | wc -l)" -eq 3 ] || fail '--keep 1 leaves one release of each package (3 files with the keyring)'
[ ! -e /work/stage3/archive/cupertino-1.0-1-x86_64.pkg.tar.zst ] && [ ! -e /work/stage3/archive/cupertino-1.0-1-x86_64.pkg.tar.zst.sig ] || fail '--keep 1 keeps only the newest'
ok '--keep 1 prunes the archive to the newest release, signatures included'

# SHA256SUMS.sig verifies with the repo key, signed under the Cupertino primary
[ -f "$S/SHA256SUMS.sig" ] || fail 'no SHA256SUMS.sig'
as_builder "gpg --batch --status-fd 1 --verify $S/SHA256SUMS.sig $S/SHA256SUMS 2>/dev/null" | awk '$2 == "VALIDSIG" { print toupper($NF) }' | grep -qx "$MASTER" || fail 'SHA256SUMS.sig is not a valid signature under the master key'
ok 'SHA256SUMS.sig verifies, primary key = master'

# an archive package that is tampered, unsigned or signed by another key stops build-repo before it writes anything
for n in tampered unsigned foreign; do
    rm -rf /work/arch-$n /work/stage-$n
    cp -r /work/stage1/archive /work/arch-$n
    chown -R builder /work/arch-$n
    f=/work/arch-$n/cupertino-1.0-1-x86_64.pkg.tar.zst
    case $n in
        tampered) tamper $f ;;
        unsigned) rm -f $f.sig ;;
        foreign) sign $f "$OTHER" ;;
    esac
    refused "an archive package that is $n" as_builder "$BUILD --packages /work/pkgs2 --key $SUB --url $URL --out /work/stage-$n --archive-from /work/arch-$n"
    [ ! -e /work/stage-$n ] || fail "$n archive package: build-repo wrote the output directory"
done

# build-repo refuses what it should
refused 'an http URL that is not loopback' as_builder "$BUILD --packages /work/pkgs2 --key $SUB --url http://example.invalid --out /work/bad1"
refused 'a key that is not in the keyring' as_builder "$BUILD --packages /work/pkgs2 --key 0123456789ABCDEF0123456789ABCDEF01234567 --url $URL --out /work/bad2"
refused 'a non-empty output directory' as_builder "$BUILD --packages /work/pkgs2 --key $SUB --url $URL --out /work/stage2"
refused 'a directory without packages' as_builder "$BUILD --packages /work/build --key $SUB --url $URL --out /work/bad3"

# serve the tree and make pacman use it
cat "$S/cupertino.conf" >>/etc/pacman.conf
(cd /work && python3 -m http.server 8000 --bind 127.0.0.1 --directory $S >/dev/null 2>&1 &)
for _ in $(seq 20); do curl -fsS $URL/repo/cupertino.db >/dev/null 2>&1 && break; sleep 0.5; done
pacman-key --init >/dev/null 2>&1
pacman-key --populate archlinux >/dev/null 2>&1
no_keyserver
pull() { rm -f /var/lib/pacman/sync/cupertino.db*; pacman -Syy --noconfirm; }

refused 'the repository before its key is trusted' pull
pacman-key --add /work/build/cupertino-public.asc >/dev/null 2>&1
pacman-key --finger "$MASTER" | tr -d ' ' | grep -qi "$MASTER" || fail 'pacman-key --finger does not show the master fingerprint'
pacman-key --lsign-key "$MASTER" >/dev/null 2>&1

# tampering: each change is made, must fail, and is undone
swap() { cp "$1" /work/tamper.orig; tamper "$1"; }
unswap() { cp /work/tamper.orig "$1"; }
f=$S/repo/cupertino.db.sig
swap $f; refused 'a tampered database signature' pull; unswap $f
f=$S/repo/cupertino.db
swap $f; refused 'a tampered database' pull; unswap $f
pull >/dev/null 2>&1 || fail 'the untampered repository does not synchronise'
ok 'the untampered repository synchronises with signature checking on'
f=$S/repo/cupertino-2.0-1-x86_64.pkg.tar.zst
swap $f; refused 'a tampered package' pacman -S --noconfirm cupertino; unswap $f
[ ! -e /usr/share/cupertino-test ] || fail 'a tampered package left files behind'
rm -f $S/repo/cupertino-2.0-1-x86_64.pkg.tar.zst.sig
refused 'a package without its signature file' pacman -S --noconfirm cupertino
cp $S/archive/cupertino-2.0-1-x86_64.pkg.tar.zst.sig $S/repo/
pacman -S --noconfirm cupertino-keyring cupertino cupertino-data >/dev/null 2>&1 || fail 'the untampered install'
[ "$(installed cupertino)" = 2.0-1 ] || fail '2.0-1 expected'
ok 'cupertino-keyring, cupertino and cupertino-data 2.0-1 install from the staging repository'

# rollback from the archive, with previous/ and the cache empty
rm -rf /var/lib/cupertino/previous/* /var/cache/pacman/pkg/*-1.0-1-*
cp "$S/cupertino-rollback.conf" /etc/cupertino-rollback.conf
f=$S/archive/cupertino-1.0-1-x86_64.pkg.tar.zst
swap $f; refused 'a tampered archive package' cupertino-rollback 1.0-1; unswap $f
[ "$(installed cupertino)" = 2.0-1 ] || fail 'a refusal changed the installed version'
printf 'y\n' | cupertino-rollback 1.0-1 >/tmp/rollback.out 2>&1 || { cat /tmp/rollback.out; fail 'the rollback from the archive'; }
[ "$(installed cupertino)" = 1.0-1 ] && [ "$(installed cupertino-data)" = 1.0-1 ] || fail '1.0-1 expected after the rollback'
ok 'cupertino-rollback 1.0-1 from the archive of the staging tree installed 1.0-1'
echo "repo test: $pass checks passed"
INNER
)

timeout 900 docker run --rm -v "$work:/work" "$image" bash -c "$test_script" 2>&1 | tee "$out/repo-test.log"
test "${PIPESTATUS[0]}" -eq 0
