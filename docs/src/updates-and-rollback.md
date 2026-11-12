# Signed updates and rollback

This page describes the pieces on `main` that sign Cupertino's Arch packages, update a machine from a signed repository, and go back to an earlier release. All of it lives in `packaging/keyring/`. Arch only.

Read this first:

- **No repository is published yet.** `build-repo` writes the tree of a repository; nothing uploads it. The current distribution uses a local repository; public hosting remains deferred.
- **The public key is supplied separately.** The expected primary fingerprint is in `packaging/keyring/release-key.fpr`; `check-key` rejects a different key. The `cupertino-keyring` package is built with that public key. Secret keys never belong in the source tree.
- **A machine installed from the [USB image](install-usb.md) updates from its own repository.** The installer installs `cupertino-keyring` and replaces the live image's `[cupertino]` repository with a local one. The next section describes it; the sections after it describe the published-repository tools.

## Local updates on an installed machine

Nothing is published. The installed `pacman.conf` has `[cupertino]` with `Server = file:///var/lib/cupertino/repo` and `SigLevel = Required DatabaseOptional`: every package must carry a valid signature, and the database may be unsigned only because it is empty until the first release is built. `cupertino-keyring` makes pacman trust the Cupertino key. `/var/lib/cupertino/repo` and `/var/lib/cupertino/archive` belong to the user the installer created.

### Make the signing key (once)

Do this on the machine that builds the releases, with a USB drive mounted for the master key:

```sh
packaging/keyring/local-release keygen --master-home /path/on/usb/cupertino-master-gnupg
```

`gpg` asks for a passphrase for the master key. The master key stays in that directory on the drive. Back it up and unplug it; keep it offline. The command puts the signing subkey (valid one year) and `PUBLIC.asc` in `~/.config/cupertino/signing`. Copy that directory to the key USB as `cupertino-signing`, next to the master home, so a new install can get it back. It holds the secret signing subkey: treat the copy as a secret. To build an ISO that trusts the key, set it when you build:

```sh
CUPERTINO_KEYRING_KEY=~/.config/cupertino/signing/PUBLIC.asc scripts/build-iso.sh
```

### After a new install: restore the signing key

A fresh install has the public key (through `cupertino-keyring`) but not the subkey that signs, and `local-release` stops with "no signing key" until `~/.config/cupertino/signing` exists. Copy the `cupertino-signing` directory that you saved from the key USB (see above) to `~/.config/cupertino/signing`. A FAT drive does not keep file modes, so afterwards run `chmod 700 ~/.config/cupertino/signing`; otherwise `gpg` may warn about unsafe permissions. Only the signing subkey and `PUBLIC.asc` go there; the master key stays on the drive. `local-release` refuses a signing home that holds the master secret key.

The subkey can sign for any repository that pacman trusts the master key for, so keep `~/.config/cupertino/signing` private. When the subkey expires, add a new one on the drive; `local-release` does not do this.

### Update

From a checkout with the change committed:

```sh
packaging/keyring/local-release
sudo pacman -Syu
```

`local-release` builds the committed `HEAD` in the pinned Arch image. The `pkgrel` is the number of commits, so every commit is an update. It signs the packages and the database with the subkey and writes `repo/` and `archive/` under `/var/lib/cupertino`. Uncommitted changes are not built.

### What pacman refuses

pacman installs only packages signed by the Cupertino key. It refuses a package that is unsigned, one that was changed after it was signed (tampered), and one signed by another key. `packaging/ci/test-local-repo.sh` checks these three refusals against a throwaway key, along with keygen, an empty first-boot database, and install and update from a `file://` repository. It was not run for this page.

To go back, use `sudo cupertino-rollback VERSION` (see [Roll back](#roll-back)).

If `pacman-key --populate cupertino` failed during the install, the installer printed a warning; run it once after the first boot.

## What is there

| File | Installed as | What it does |
|---|---|---|
| `cupertino-keyring` (`PKGBUILD`, `cupertino-keyring.install`) | the package | Ships the public key as `/usr/share/pacman/keyrings/cupertino.gpg`, the primary key's fingerprint as `cupertino-trusted` and an empty `cupertino-revoked` in the same directory, then runs `pacman-key --populate cupertino` on install and on upgrade. |
| `cupertino-rollback` | `/usr/bin/cupertino-rollback` | Installs an earlier release of `cupertino` and `cupertino-data` (`cupertino-kit`, the C SDK, is not rolled back). |
| `cupertino-save-previous` | `/usr/share/libalpm/scripts/cupertino-save-previous` | Keeps the installed release before an upgrade. |
| `cupertino-previous.hook` | `/usr/share/libalpm/hooks/cupertino-previous.hook` | The pacman hook that runs the script above. |
| `build-repo` | not installed | Writes a signed repository tree. |
| `archive-prune` | not installed | Prunes the archive of old releases. |

The package depends on `pacman`, `gnupg`, `libarchive` and `curl`.

## Trust the key on a new machine

The keyring package is signed by the key it carries, so trust the key before you install the package. With `FPR` the key's fingerprint:

```sh
sudo pacman-key --recv-keys FPR        # or: sudo pacman-key --add KEYFILE
sudo pacman-key --finger FPR           # compare all 40 hex digits with the fingerprint you were given
sudo pacman-key --lsign-key FPR
```

Then add the `[cupertino]` section to `/etc/pacman.conf` and install the keyring:

```ini
[cupertino]
SigLevel = Required DatabaseRequired TrustedOnly
Server = BASE_URL/repo
```

```sh
sudo pacman -Sy cupertino-keyring
```

`build-repo` writes this section for you as `cupertino.conf`, with `BASE_URL` filled in.

## Update

Once the repository is in `pacman.conf` and the key is trusted, an update is the usual pacman command:

```sh
sudo pacman -Syu
```

Before a transaction that upgrades `cupertino` or `cupertino-data`, the hook runs `cupertino-save-previous`. For each of the two packages it copies the installed release from `/var/cache/pacman/pkg/`, with its `.sig`, into `/var/lib/cupertino/previous/` (owned by `root:root`, mode `0700`). It keeps only the last release: it deletes the older copy of that package first. If the cache holds no signed file for the installed version (for example after a local install), it prints a message to standard error and keeps no copy for that package.

## Roll back

```sh
sudo cupertino-rollback VERSION
```

`VERSION` is the full package version, for example `0.1.0-1`. The command installs `cupertino` and `cupertino-data` of that version and takes both from the first place that has them:

1. `/var/lib/cupertino/previous/`,
2. `/var/cache/pacman/pkg/`,
3. the archive named in `/etc/cupertino-rollback.conf`, a line `archive=URL` where `URL` is `https://...` (or `http://127.0.0.1...` for tests). `build-repo` writes this line as `cupertino-rollback.conf`.

It copies the packages and their signatures into a private directory under `/var/tmp` and installs them with `pacman -U` only when, for each package:

- the version matches `[0-9A-Za-z.+_:~-]+`;
- the file has a `.sig` (a file without one is refused, because the stock `pacman.conf` accepts unsigned local files);
- the signature verifies against the pacman keyring with exactly one valid signature and none that is bad, expired or from a revoked key, and the primary key's fingerprint is listed in `/usr/share/pacman/keyrings/cupertino-trusted`;
- `.PKGINFO` names exactly one package, the one asked for, with the version asked for.

It must run as root, and it needs `cupertino-trusted`, so it needs `cupertino-keyring`.

Exit status: 1 for every refusal, with a message that starts `cupertino-rollback:`. This covers a wrong number of arguments, a bad version, running as non-root, a missing `cupertino-trusted`, a package that is in none of the three places, and each check above. After the checks, the exit status is the one `pacman -U` returns.

The hook also runs for that `pacman -U`, since it upgrades an installed package. By reading the hook, `previous/` then holds the release you rolled back from. This was not run.

## Build a repository tree

For the maintainers. `build-repo` needs `gpg`, `repo-add` and the secret signing key in `GNUPGHOME`:

```sh
packaging/keyring/build-repo --packages DIR --key FPR --url BASE_URL --out DIR [--archive-from DIR] [--keep N] [--gnupghome DIR]
```

| Option | Meaning |
|---|---|
| `--packages DIR` | The built, unsigned `*.pkg.tar.zst` files of one release. |
| `--key FPR` | The signing key, as hexadecimal digits, at least 16 of them. |
| `--url BASE_URL` | `https://...`, or `http://127.0.0.1` for tests. |
| `--out DIR` | A new or empty directory. |
| `--archive-from DIR` | The earlier releases to carry into the archive. |
| `--keep N` | How many versions of each package the archive keeps. A positive whole number, 5 by default. |
| `--gnupghome DIR` | Use this GnuPG home instead of `GNUPGHOME`. |

It signs each package, runs `repo-add --sign`, copies the release into the archive, prunes the archive and writes:

| Path in `--out` | Content |
|---|---|
| `repo/` | The packages with their `.sig`, and `cupertino.db` and `cupertino.files` with their `.sig`, as plain files. |
| `archive/` | The releases `cupertino-rollback` can fetch. |
| `cupertino.conf` | The `[cupertino]` section for `pacman.conf`, with `SigLevel = Required DatabaseRequired TrustedOnly` and `Server = BASE_URL/repo`. |
| `cupertino-rollback.conf` | The line `archive=BASE_URL/archive`. |
| `SHA256SUMS` | The checksum of every other file in the tree. |

It uploads nothing. When you upload, put the packages and their `.sig` first and the database files last, so a client never sees a database that names a missing file.

It exits 1, with a message that starts `build-repo:`, for a usage error, a key that is not hexadecimal or is shorter than 16 digits, a bad `--keep` or URL, no `*.pkg.tar.zst` in `--packages`, a secret key missing from the keyring, a non-empty `--out`, and a signature that does not verify. A failure of `gpg` or `repo-add` itself stops the script with that command's status.

`archive-prune DIR [N]` keeps the newest `N` (5 by default) versions of each package in `DIR`, ordered with `vercmp`, and deletes the older ones with their signatures. A `N` that is not a positive whole number exits 2.

## Tests

Both need Docker, use a pinned Arch image and throwaway keys:

```sh
packaging/ci/test-keyring.sh [OUT_DIR]
packaging/ci/test-repo.sh [OUT_DIR]
```

The first builds the keyring and stand-in packages, serves a signed repository on the loopback and exercises the bootstrap order, an update, the hook, the rollback and its refusals, and `archive-prune`. The second runs `build-repo` for two releases and checks the tree, and that pacman refuses a tampered database or package. They were not run for this page.
