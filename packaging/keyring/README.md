# cupertino-keyring

The Cupertino package signing key and the tools for updating and rolling back from a signed repository. Arch only. Nothing here publishes a repository:
the release job waits for the host and key decisions.

| File | Installed as | What it does |
|---|---|---|
| `PKGBUILD`, `cupertino-keyring.install` | package `cupertino-keyring` | ships the public key from `CUPERTINO_KEYRING_KEY` (an ASCII-armored export of the master key with its signing subkey; the key is not in this repository) as `/usr/share/pacman/keyrings/cupertino.gpg` and `cupertino-trusted`, then runs `pacman-key --populate cupertino` |
| `cupertino-rollback` | `/usr/bin/cupertino-rollback` | `sudo cupertino-rollback VERSION` installs `cupertino` and `cupertino-data` of that version |
| `cupertino-save-previous`, `cupertino-previous.hook` | `/usr/share/libalpm/scripts/`, `/usr/share/libalpm/hooks/` | before an upgrade, copies the installed packages and their `.sig` into `/var/lib/cupertino/previous/` (`root:root 0700`, the last release only) |
| `archive-prune` | not installed | `archive-prune DIR [N]` keeps the newest N (default 5) versions of each package in the archive directory, with their signatures |

## Rollback rules

`cupertino-rollback` copies both packages and their signatures into a private directory (from `previous/`, the pacman
cache, or the `archive=URL` of `/etc/cupertino-rollback.conf`), and installs them only when, for each:

- the version argument matches `[0-9A-Za-z.+_:~-]+`;
- the signature verifies against the pacman keyring with exactly one `VALIDSIG`, none of `BADSIG`, `ERRSIG`, `EXPSIG`,
  `EXPKEYSIG` or `REVKEYSIG`, and the **primary** key fingerprint is one in `cupertino-trusted`;
- `.PKGINFO` has exactly one `pkgname`, equal to the package asked for, and a `pkgver` equal to the version asked for.

A package without a `.sig` is refused: the stock `pacman.conf` has `LocalFileSigLevel = Optional`, so `pacman -U` alone would
accept it.

## Bootstrap on a new machine

Trust the key before installing the keyring, because the keyring is signed by that key: `pacman-key --recv-keys FPR` (or
`--add KEYFILE`), `pacman-key --finger FPR` and compare all 40 hex digits with the install page, `pacman-key --lsign-key FPR`,
then add the `[cupertino]` section (`SigLevel = Required DatabaseRequired TrustedOnly`) and `pacman -Sy cupertino-keyring`.

## Test

`packaging/ci/test-keyring.sh [OUT_DIR]` (needs docker; the pinned Arch image; throwaway keys; about two minutes) builds the
keyring and stand-in packages, serves a signed repository on the loopback and checks 16 things: the repository is refused
before its key is trusted, install and `pacman -Syu`, `previous/` mode and contents, the refusal of an unsigned package, of one
signed by another key that pacman trusts, of a Cupertino-signed package of another name renamed to `cupertino`, of one with
another version, of bad version arguments, a good rollback from `previous/` and from the archive, and `archive-prune`.
