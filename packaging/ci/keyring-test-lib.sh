# Shared by packaging/ci/test-keyring.sh and test-repo.sh; sourced inside the pinned Arch container, where the host's
# scratch directory is /work and the committed packaging files are /work/src/packaging.
pass=0
ok() { pass=$((pass + 1)); echo "ok   $*"; }
fail() { echo "FAIL $*"; exit 1; }
# refused NAME CMD...: the command must fail even when its prompt is answered yes
refused() {
    local name=$1
    shift
    if printf 'y\n' | "$@" >/tmp/refused.out 2>&1; then
        cat /tmp/refused.out
        fail "$name was accepted"
    fi
    ok "$name is refused: $(tail -n 1 /tmp/refused.out)"
}
installed() { pacman -Q "$1" | cut -d ' ' -f 2; }
as_builder() { su builder -c "export GNUPGHOME=/home/builder/gnupg; $*"; }
gen='gpg --batch --pinentry-mode loopback --passphrase ""'
# detach-sign FILE with the key $2 (a fingerprint)
sign() { as_builder "$gen --detach-sign -u '$2!' -o '$1.sig' '$1'"; }

# the build user, the work directories, and the keys: a throwaway Cupertino master with a signing subkey (MASTER, SUB)
# and a second key that pacman is made to trust like an Arch packager (OTHER)
prepare() {
    pacman -Syu --noconfirm --needed -q gnupg python libarchive curl >/dev/null
    useradd -m builder
    install -d -o builder /work/build /work/pkgs
    cp -r /work/src/packaging/keyring /work/build/keyring
    chown -R builder /work/build
    as_builder 'mkdir -p $GNUPGHOME && chmod 700 $GNUPGHOME'
    as_builder "$gen --quick-gen-key 'Cupertino Test Master <t@example.invalid>' ed25519 cert 1y"
    MASTER=$(as_builder "gpg --batch --with-colons --list-keys 'Cupertino Test Master' | awk -F: '\$1==\"fpr\"{print \$10; exit}'")
    as_builder "$gen --quick-add-key $MASTER ed25519 sign 1y"
    SUB=$(as_builder "gpg --batch --with-colons --list-keys $MASTER | awk -F: '\$1==\"sub\"{s=1} s&&\$1==\"fpr\"{print \$10; exit}'")
    as_builder "$gen --quick-gen-key 'Other Packager <o@example.invalid>' ed25519 cert,sign 1y"
    OTHER=$(as_builder "gpg --batch --with-colons --list-keys 'Other Packager' | awk -F: '\$1==\"fpr\"{print \$10; exit}'")
    as_builder "gpg --batch --armor --export $MASTER > /work/build/cupertino-public.asc"
    as_builder "gpg --batch --armor --export $OTHER > /work/build/other-public.asc"
}

# mk NAME VERSION DESTDIR: an unsigned stand-in package, one empty file named after the package and version
mk() {
    local dir=/work/build/pkg-$1-$2
    install -d -o builder "$dir"
    printf '%s\n' "pkgname=$1" "pkgver=${2%-*}" "pkgrel=${2#*-}" "pkgdesc='stand-in'" "arch=('x86_64')" \
        "license=('GPL-3.0-or-later')" \
        "package() { install -Dm644 /dev/null \"\$pkgdir/usr/share/cupertino-test/$1-\$pkgver\"; }" >"$dir/PKGBUILD"
    chown builder "$dir/PKGBUILD"
    su builder -c "cd $dir && makepkg -f --nodeps >/dev/null 2>&1"
    mkdir -p "$3"
    mv "$dir/$1-$2-x86_64.pkg.tar.zst" "$3/"
    chown builder "$3"
}

# the real cupertino-keyring package, built with the throwaway public key, into DESTDIR
mk_keyring() {
    su builder -c "cd /work/build/keyring && CUPERTINO_KEYRING_KEY=/work/build/cupertino-public.asc makepkg -f --nodeps 2>&1 | tail -n 3" || fail 'cupertino-keyring did not build (are the sha256sums current?)'
    mkdir -p "$1"
    mv /work/build/keyring/cupertino-keyring-1-1-any.pkg.tar.zst "$1/"
    chown builder "$1"
}

# no key server: an unknown key must be refused, not looked up
no_keyserver() { echo 'keyserver hkps://127.0.0.1:9' >>/etc/pacman.d/gnupg/gpg.conf; }
# flip one bit in the middle of a file
tamper() { python3 -c 'import sys; p = sys.argv[1]; b = bytearray(open(p, "rb").read()); b[len(b) // 2] ^= 1; open(p, "wb").write(b)' "$1"; }
