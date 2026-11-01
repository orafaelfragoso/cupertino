# cupertino-keyring

The Cupertino package signing key and the tools for updating and rolling back from a signed repository. Arch only. Nothing here publishes a repository:
`build-repo` writes the tree and the upload waits for the host and key decisions. The release workflow's `sign` job runs
`packaging/ci/sign-release.sh` (build-repo in the pinned Arch image, no network, the key on stdin) with the secret
`CUPERTINO_SIGNING_KEY` (an armored export of the signing subkey only; a key that holds the master is refused) and the
variable `CUPERTINO_REPO_URL`, and attaches the packages, their `.sig`, the database files and `SHA256SUMS.repo` (with `.sig`) to
the draft release. Without the secret the job fails and no draft is made. The archive of earlier releases and `cupertino-keyring`
are not part of that job yet.

| File | Installed as | What it does |
|---|---|---|
| `PKGBUILD`, `cupertino-keyring.install` | package `cupertino-keyring` | ships the public key from `CUPERTINO_KEYRING_KEY` (an ASCII-armored export of the master key with its signing subkey; the key is not in this repository) as `/usr/share/pacman/keyrings/cupertino.gpg` and `cupertino-trusted`, then runs `pacman-key --populate cupertino` |
| `cupertino-rollback` | `/usr/bin/cupertino-rollback` | `sudo cupertino-rollback VERSION` installs `cupertino` and `cupertino-data` of that version |
| `cupertino-save-previous`, `cupertino-previous.hook` | `/usr/share/libalpm/scripts/`, `/usr/share/libalpm/hooks/` | before an upgrade, copies the installed packages and their `.sig` into `/var/lib/cupertino/previous/` (`root:root 0700`, the last release only) |
| `build-repo` | not installed | `build-repo --packages DIR --key FPR --url BASE_URL --out DIR [--archive-from DIR] [--keep N]` signs the built packages, runs `repo-add --sign`, adds the release to the archive (pruned to N, default 5) and writes the whole tree to a new directory: `repo/`, `archive/`, `cupertino.conf` (the `[cupertino]` section), `cupertino-rollback.conf`, `SHA256SUMS` and its detached signature `SHA256SUMS.sig`. Every package taken from `--archive-from` must verify with gpg and be signed under the primary key of `--key` (one valid signature, none bad, expired or revoked); otherwise it stops before writing anything. It uploads nothing: upload the packages and their `.sig` first and the database files last. The key is read from `GNUPGHOME`; the URL is `https://` (`http://127.0.0.1` for tests) |
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

## Tests

`packaging/ci/test-sign-release.sh` (docker, a throwaway key, 5 checks) runs the release signing step without GitHub: no key,
a key with the master secret, an empty URL and a non-key each sign nothing; a signing subkey alone signs the packages, the
database and `SHA256SUMS` under the master.

Both need docker, use the pinned Arch image and throwaway keys, and take about two minutes.

- `packaging/ci/test-keyring.sh [OUT_DIR]` builds the keyring and stand-in packages, serves a signed repository on the loopback
  and checks 17 things: the repository is refused before its key is trusted, install and `pacman -Syu`, `previous/` mode and
  contents, the refusal of an unsigned package, of one signed by another key that pacman trusts, of a Cupertino-signed
  package of another name renamed to `cupertino`, of one with another version, of bad version arguments, a good rollback
  from `previous/` and from the archive, and `archive-prune` (five releases kept, names matched literally).
- `packaging/ci/test-repo.sh [OUT_DIR]` runs `build-repo` for two releases and checks 15 things: the tree (plain files,
  signatures, `SHA256SUMS` and `SHA256SUMS.sig` under the master key, pruning to N), the refusal of a tampered, unsigned or foreign-signed `--archive-from` package before anything is written, the refusal of a bad URL, a missing key, a non-empty output directory and no packages,
  then serves the tree over `http://127.0.0.1` and checks that pacman refuses it before the key is trusted and refuses a
  tampered database signature, a tampered database, a tampered package and a missing package signature; that the untampered
  repository installs; and that `cupertino-rollback` installs the earlier release from the archive and refuses a tampered
  archive package.
