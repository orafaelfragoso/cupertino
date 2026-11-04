# cupertino-keyring

The Cupertino package signing key and the tools for updating and rolling back from a signed repository. Arch only. Nothing here publishes a repository:
`build-repo` writes the tree and the upload waits for the host and key decisions. The release workflow's `sign` job runs
`packaging/ci/sign-release.sh` (build-repo in the pinned Arch image, no network, the key on stdin) with the secret
`CUPERTINO_SIGNING_KEY` (stored in the `signing` environment, which admits v* tags only and is used by no other job) (an armored export of the signing subkey only; a key that holds the master is refused) and the
variable `CUPERTINO_REPO_URL`, and attaches the packages, their `.sig`, the database files and `SHA256SUMS.repo` (with `.sig`) to
the draft release. For a final release it also signs the ISO's `.sha256` (`NAME.iso.sha256.sig`, same subkey), because every install
trusts the key the ISO carries; `build-iso.sh` runs `check-key`, which refuses a key file whose primary fingerprint is not the one
in `release-key.fpr` (and any file with a second key). Without the secret the job fails and no draft is made. The archive of earlier releases and `cupertino-keyring`
are not part of that job yet. Risk for Rafael and Orion: the environments have no required reviewers, so the gate is who can push a
`v*` tag (a tagged commit runs its own `release.yml`); `source` makes the job chain fail for a tag that is not on main, but
only a tag ruleset restricting who can create `v*` tags closes the hole.

| File | Installed as | What it does |
|---|---|---|
| `PKGBUILD`, `cupertino-keyring.install` | package `cupertino-keyring` | ships the public key from `CUPERTINO_KEYRING_KEY` (an ASCII-armored export of the master key with its signing subkey; the key is not in this repository) as `/usr/share/pacman/keyrings/cupertino.gpg` and `cupertino-trusted`, then runs `pacman-key --populate cupertino` |
| `cupertino-rollback` | `/usr/bin/cupertino-rollback` | `sudo cupertino-rollback VERSION` installs `cupertino` and `cupertino-data` of that version |
| `cupertino-save-previous`, `cupertino-previous.hook` | `/usr/share/libalpm/scripts/`, `/usr/share/libalpm/hooks/` | before an upgrade, copies the installed packages and their `.sig` into `/var/lib/cupertino/previous/` (`root:root 0700`, the last release only) |
| `build-repo` | not installed | `build-repo --packages DIR --key FPR --url BASE_URL --out DIR [--archive-from DIR] [--keep N]` signs the built packages, runs `repo-add --sign`, adds the release to the archive (pruned to N, default 5) and writes the whole tree to a new directory: `repo/`, `archive/`, `cupertino.conf` (the `[cupertino]` section), `cupertino-rollback.conf`, `SHA256SUMS` and its detached signature `SHA256SUMS.sig`. Every package taken from `--archive-from` is copied into `archive/` first and verified there, so the source is read once and the bytes checked are the bytes published; it must verify with gpg and be signed under the primary key of `--key` (one valid signature, none bad, expired or revoked); otherwise `repo/` and `archive/` are removed and nothing is published. It uploads nothing: upload the packages and their `.sig` first and the database files last. The key is read from `GNUPGHOME`; the URL is `https://` (`http://127.0.0.1` for tests) |
| `build-keyring` | not installed | `build-keyring PUBLIC_KEY_FILE OUT_DIR` builds `cupertino-keyring` in the pinned Arch image |
| `local-release` | not installed | `local-release keygen` makes the key; `local-release` builds the checkout, signs it and installs it into `/var/lib/cupertino` (see "Local updates") |
| `archive-prune` | not installed | `archive-prune DIR [N]` keeps the newest N (default 5) versions of each package in the archive directory, with their signatures |

## Local updates (the installed machine hosts its own signed repository)

Nothing is published. The installed system has `[cupertino]` with `Server = file:///var/lib/cupertino/repo` and
`SigLevel = Required DatabaseOptional` (package signatures are required; the database may be unsigned only because the first
boot starts with an empty one), and `cupertino-keyring` trusts the Cupertino key.

1. **Once, with the USB drive mounted:** `packaging/keyring/local-release keygen --master-home /path/on/usb/cupertino-master-gnupg`.
   gpg asks for a passphrase for the master key. The master key stays in that directory on the drive: back it up, and unplug
   it. The signing subkey (valid one year) and `PUBLIC.asc` land in `~/.config/cupertino/signing`. Build the ISO with
   `CUPERTINO_KEYRING_KEY=~/.config/cupertino/signing/PUBLIC.asc scripts/build-iso.sh`, so that the installed system trusts it.
2. **Each update:** from a checkout with the change committed, run `packaging/keyring/local-release`. It builds the committed
   HEAD in the pinned Arch image (`pkgrel` = number of commits, so every commit is an update), signs the packages and the
   database with the subkey and writes `repo/` and `archive/` under `/var/lib/cupertino`. Then `sudo pacman -Syu`.
   `local-release` refuses a signing home that holds the master secret key.
3. **Roll back:** `sudo cupertino-rollback VERSION` (cache, `previous/`, or the archive).

The subkey can sign for any repository pacman trusts the master for; keep `~/.config/cupertino/signing` private. When the
subkey expires, rerun the key steps by adding a new subkey on the drive (not covered by `local-release`).

`packaging/ci/test-local-repo.sh` (docker, throwaway key, 10 checks) runs this flow: keygen, an empty first-boot database,
install and update from a `file://` repository, and the refusal of a tampered, an unsigned and a foreign-signed package.

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
  signatures, `SHA256SUMS` and `SHA256SUMS.sig` under the master key, pruning to N), the refusal of a tampered, unsigned or foreign-signed `--archive-from` package with no tree left behind, a source that changes after its copy (the verified bytes are published), the refusal of a bad URL, a missing key, a non-empty output directory and no packages,
  then serves the tree over `http://127.0.0.1` and checks that pacman refuses it before the key is trusted and refuses a
  tampered database signature, a tampered database, a tampered package and a missing package signature; that the untampered
  repository installs; and that `cupertino-rollback` installs the earlier release from the archive and refuses a tampered
  archive package.
