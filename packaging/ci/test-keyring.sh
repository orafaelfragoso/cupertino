#!/usr/bin/env bash
# Signing, update and rollback of the Cupertino package repository, with throwaway keys, in the pinned Arch image.
#
#   packaging/ci/test-keyring.sh [OUT_DIR]      the log goes to OUT_DIR/keyring-test.log
#
# Builds cupertino-keyring from packaging/keyring (committed HEAD) and small stand-in packages `cupertino` and
# `cupertino-data` in two versions, signs them with a throwaway Cupertino key (a master and a signing subkey) and a second
# throwaway key that pacman trusts like an Arch packager, serves the repository over HTTP on the loopback, and checks:
# the bootstrap order, an update, the rollback hook, and that cupertino-rollback refuses an unsigned package, a package
# signed with the other trusted key, a renamed package and a package of another version, accepts a good one from
# previous/ and from the archive, and that archive-prune keeps five releases.
set -euo pipefail

root=$(git -C "$(dirname "$0")" rev-parse --show-toplevel)
out=${1:-$HOME/.cache/cupertino-keyring-test}
image=docker.io/library/archlinux@sha256:3cbf69d2d5958d6a294f3722c5668e6187c4df8e00fa27f72352f0bf83e495e1
mkdir -p "$out"
out=$(cd "$out" && pwd)
work=$(mktemp -d "$out/work.XXXXXX")
trap 'docker run --rm -v "$work:/w" "$image" rm -rf /w/build /w/repo /w/archive /w/prune /w/previous-good /w/v2 /w/extra /w/prune-rx >/dev/null 2>&1 || true; rm -rf "$work"' EXIT
git -C "$root" archive --format=tar --prefix=src/ HEAD packaging/keyring packaging/ci/keyring-test-lib.sh | tar -x -C "$work"

test_script=$(
    cat <<'INNER'
set -euo pipefail
. /work/src/packaging/ci/keyring-test-lib.sh
prepare
install -d -o builder /work/repo /work/archive /work/v2 /work/extra

publish() {
    rm -f /work/repo/cupertino.db* /work/repo/cupertino.files*
    as_builder "cd /work/repo && repo-add -q --sign --key $SUB cupertino.db.tar.zst *.pkg.tar.zst"
}

mk_keyring /work/repo
for p in cupertino cupertino-data; do mk "$p" 1.0-1 /work/repo; mk "$p" 2.0-1 /work/v2; done
mk cupertino-extra 1.0-1 /work/extra
for f in /work/repo/*.pkg.tar.zst /work/v2/*.pkg.tar.zst /work/extra/*.pkg.tar.zst; do sign "$f" "$SUB"; done
publish

cat >>/etc/pacman.conf <<CONF
[cupertino]
SigLevel = Required DatabaseRequired TrustedOnly
Server = http://127.0.0.1:8000/repo
CONF
mkdir -p /srv
ln -s /work/repo /srv/repo
ln -s /work/archive /srv/archive
(cd /srv && python3 -m http.server 8000 --bind 127.0.0.1 >/dev/null 2>&1 &)
for _ in $(seq 20); do curl -fsS http://127.0.0.1:8000/repo/cupertino.db >/dev/null 2>&1 && break; sleep 0.5; done

# 1. bootstrap order: nothing from the repository is accepted before the key is trusted
pacman-key --init >/dev/null 2>&1
no_keyserver
pacman-key --populate archlinux >/dev/null 2>&1
refused 'the repository before its key is trusted' pacman -Sy --noconfirm cupertino-keyring
pacman-key --add /work/build/cupertino-public.asc >/dev/null 2>&1
pacman-key --finger "$MASTER" | tr -d ' ' | grep -qi "$MASTER" || fail 'pacman-key --finger does not show the master fingerprint'
pacman-key --lsign-key "$MASTER" >/dev/null 2>&1
pacman -Sy --noconfirm cupertino-keyring >/dev/null 2>&1 || fail 'cupertino-keyring does not install after the key is trusted'
ok 'cupertino-keyring installs once the key is trusted'
grep -q "^$MASTER:4:" /usr/share/pacman/keyrings/cupertino-trusted || fail 'the trusted file lacks the master fingerprint'
pacman -S --noconfirm cupertino cupertino-data >/dev/null 2>&1 || fail 'the first install'
[ "$(installed cupertino)" = 1.0-1 ] || fail 'cupertino 1.0-1 expected'
ok 'cupertino 1.0-1 and cupertino-data installed from the signed repository'

# 2. update: 2.0-1 arrives through pacman -Syu and the hook keeps 1.0-1
mv /work/v2/* /work/repo/
publish
pacman -Syu --noconfirm >/dev/null 2>&1 || fail 'the update'
[ "$(installed cupertino)" = 2.0-1 ] && [ "$(installed cupertino-data)" = 2.0-1 ] || fail '2.0-1 expected after the update'
ok 'pacman -Syu updated both packages to 2.0-1'
P=/var/lib/cupertino/previous
[ "$(stat -c '%a %U:%G' $P)" = '700 root:root' ] || fail 'previous/ must be 0700 root:root'
for n in cupertino cupertino-data; do
    [ -f "$P/$n-1.0-1-x86_64.pkg.tar.zst" ] && [ -f "$P/$n-1.0-1-x86_64.pkg.tar.zst.sig" ] || fail "previous/ lacks $n 1.0-1 or its .sig"
done
ok 'previous/ is 0700 root:root and holds 1.0-1 of both packages with their .sig'

# 3. refusals, each from a fresh copy of previous/ and with 2.0-1 still installed
good=/work/previous-good
cp -a $P "$good"
reset() {
    rm -rf $P
    cp -a "$good" $P
    rm -f /var/cache/pacman/pkg/*-1.0-1-*
}
C1=cupertino-1.0-1-x86_64.pkg.tar.zst
reset
rm "$P/$C1.sig"
refused 'an unsigned package' cupertino-rollback 1.0-1

reset
pacman-key --add /work/build/other-public.asc >/dev/null 2>&1
pacman-key --lsign-key "$OTHER" >/dev/null 2>&1
cp "$P/$C1" /work/build/c1.pkg
chown builder /work/build/c1.pkg
as_builder "$gen --detach-sign -u '$OTHER!' -o /work/build/c1.pkg.sig /work/build/c1.pkg"
cp /work/build/c1.pkg.sig "$P/$C1.sig"
gpg --homedir /etc/pacman.d/gnupg --verify "$P/$C1.sig" "$P/$C1" >/dev/null 2>&1 || fail 'the other key must be one pacman trusts'
refused 'a package signed with another key that pacman trusts' cupertino-rollback 1.0-1

reset
cp /work/extra/cupertino-extra-1.0-1-x86_64.pkg.tar.zst "$P/$C1"
cp /work/extra/cupertino-extra-1.0-1-x86_64.pkg.tar.zst.sig "$P/$C1.sig"
refused 'a Cupertino-signed package of another name renamed to cupertino' cupertino-rollback 1.0-1

reset
cp /srv/repo/cupertino-2.0-1-x86_64.pkg.tar.zst "$P/$C1"
cp /srv/repo/cupertino-2.0-1-x86_64.pkg.tar.zst.sig "$P/$C1.sig"
refused 'a 2.0-1 package under the name of 1.0-1' cupertino-rollback 1.0-1

reset
refused 'a version with a shell metacharacter' cupertino-rollback '1.0-1;id'
refused 'a version with a path' cupertino-rollback '../x'
refused 'a version that is nowhere' cupertino-rollback 9.9-9
[ "$(installed cupertino)" = 2.0-1 ] || fail 'a refusal changed the installed version'
ok 'every refusal left 2.0-1 installed'

# 4. a good rollback from previous/
reset
printf 'y\n' | cupertino-rollback 1.0-1 >/tmp/rollback.out 2>&1 || { cat /tmp/rollback.out; fail 'the rollback from previous/'; }
[ "$(installed cupertino)" = 1.0-1 ] && [ "$(installed cupertino-data)" = 1.0-1 ] || fail '1.0-1 expected after the rollback'
ok 'cupertino-rollback 1.0-1 from previous/ installed 1.0-1 of both packages'

# 5. a good rollback from the archive, with previous/ and the cache emptied
cp "$good"/* /work/archive/
pacman -Syu --noconfirm >/dev/null 2>&1 || fail 'the second update'
[ "$(installed cupertino)" = 2.0-1 ] || fail '2.0-1 expected'
rm -rf $P/* /var/cache/pacman/pkg/*-1.0-1-*
echo 'archive=http://127.0.0.1:8000/archive' >/etc/cupertino-rollback.conf
printf 'y\n' | cupertino-rollback 1.0-1 >/tmp/rollback.out 2>&1 || { cat /tmp/rollback.out; fail 'the rollback from the archive'; }
[ "$(installed cupertino)" = 1.0-1 ] || fail '1.0-1 expected after the archive rollback'
ok 'cupertino-rollback 1.0-1 from the archive installed 1.0-1'

# 6. the archive keeps five releases
prune=/work/prune
mkdir -p $prune
for v in 0.1.0-1 0.1.0-2 0.2.0-1 0.10.0-1 0.9.0-1 1:0.0.1-1 0.3.0-1; do
    for n in cupertino cupertino-data; do
        : >"$prune/$n-$v-x86_64.pkg.tar.zst"
        : >"$prune/$n-$v-x86_64.pkg.tar.zst.sig"
    done
done
sh /work/src/packaging/keyring/archive-prune "$prune"
left=$(find $prune -name 'cupertino-[0-9]*.pkg.tar.zst' | wc -l)
[ "$left" -eq 5 ] || fail "5 releases of cupertino expected, found $left"
[ -f "$prune/cupertino-1:0.0.1-1-x86_64.pkg.tar.zst" ] || fail 'the epoch version is the newest and stays'
[ -f "$prune/cupertino-0.10.0-1-x86_64.pkg.tar.zst" ] || fail '0.10.0 is newer than 0.9.0 and stays'
[ ! -f "$prune/cupertino-0.1.0-1-x86_64.pkg.tar.zst.sig" ] && [ ! -f "$prune/cupertino-data-0.1.0-2-x86_64.pkg.tar.zst" ] || fail 'the oldest releases and their signatures should be gone'
ok 'archive-prune keeps the newest five releases of each package, epoch and 0.10 versus 0.9 included'
# a name with a character that is special in a regular expression is not another package's prefix
rx=/work/prune-rx
mkdir -p $rx
for f in a.b-1-1 a.b-0-1 aXb-5-1; do : >"$rx/$f-x86_64.pkg.tar.zst"; : >"$rx/$f-x86_64.pkg.tar.zst.sig"; done
sh /work/src/packaging/keyring/archive-prune "$rx" 1
[ -f "$rx/a.b-1-1-x86_64.pkg.tar.zst" ] && [ ! -f "$rx/a.b-0-1-x86_64.pkg.tar.zst" ] && [ -f "$rx/aXb-5-1-x86_64.pkg.tar.zst" ] || fail 'a.b and aXb must be pruned apart'
ok 'archive-prune treats a package name literally, not as a regular expression'
echo "keyring test: $pass checks passed"
INNER
)

timeout 900 docker run --rm -v "$work:/work" "$image" bash -c "$test_script" 2>&1 | tee "$out/keyring-test.log"
test "${PIPESTATUS[0]}" -eq 0
