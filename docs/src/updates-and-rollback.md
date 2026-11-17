# Signed updates

This page describes how Cupertino's Arch packages are signed, how a machine updates from a signed repository, how it goes back to an earlier release, and how it installs a build of its own. Arch only.

Read this first:

- **No repository is published yet.** The release workflow signs the packages and writes the tree of a repository as an artifact of its run; nothing uploads it. Public hosting remains deferred.
- **The public key is supplied separately.** The expected primary fingerprint is in `packaging/keyring/release-key.fpr`. The `cupertino-keyring` package is built with that public key. Secret keys never belong in the source tree.

## The keyring package

`packaging/keyring/` holds the recipe of `cupertino-keyring` (`PKGBUILD`, `cupertino-keyring.install`). Built with `CUPERTINO_KEYRING_KEY` naming an ASCII-armored export of the public key, the package ships the key as `/usr/share/pacman/keyrings/cupertino.gpg`, the primary key's fingerprint as `cupertino-trusted` and an empty `cupertino-revoked` in the same directory, and runs `pacman-key --populate cupertino` on install and on upgrade. It depends on `pacman`.

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

The repository tree carries this section as `cupertino.conf`, with `BASE_URL` filled in.

## Update

Once the repository is in `pacman.conf` and the key is trusted, an update is the usual pacman command:

```sh
sudo pacman -Syu
```

pacman installs only packages signed by the Cupertino key. It refuses a package that is unsigned, one that was changed after it was signed, and one signed by another key.

## Go back to an earlier release

pacman keeps the packages it installed, with their signatures, in `/var/cache/pacman/pkg/`. Install `cupertino` and `cupertino-data` of the same earlier version together, with `VERSION` the full package version, for example `0.1.0-1`:

```sh
sudo pacman -U /var/cache/pacman/pkg/cupertino-VERSION-x86_64.pkg.tar.zst \
               /var/cache/pacman/pkg/cupertino-data-VERSION-any.pkg.tar.zst
```

A release the cache no longer holds is in the `archive/` directory of the repository tree; download both packages with their `.sig` files into one directory and install them the same way. pacman checks a signature that lies beside a package file, so keep the `.sig` files next to the packages. `cupertino-kit`, the C SDK, is versioned with `cupertino`; go back with it too when it is installed.

## Install a build of your own

On an Arch machine with a checkout, the stable recipe builds the checkout's committed `HEAD`:

```sh
cd packaging/arch/cupertino
makepkg -si
```

It builds `cupertino`, `cupertino-data` and `cupertino-kit` and installs them. The version is the release version, the number of commits and the commit, for example `0.1.0.4048.gd80dfeb0d`, so every commit is newer than the one before it and than the release it follows. Uncommitted changes are not built. `CARGO_TARGET_DIR` keeps the Cargo target directory between builds. These packages are not signed; pacman installs them because they are local files.

## The repository tree

For the maintainers. The release workflow signs each package (a detached `.sig`, checked), makes the signed database with `repo-add --sign` and writes:

| Path | Content |
|---|---|
| `repo/` | The packages with their `.sig`, and `cupertino.db` and `cupertino.files` with their `.sig`, as plain files. |
| `archive/` | The packages of this release with their `.sig`. |
| `cupertino.conf` | The `[cupertino]` section for `pacman.conf`, with `SigLevel = Required DatabaseRequired TrustedOnly` and `Server = BASE_URL/repo`. |
| `SHA256SUMS` | The checksum of every other file in the tree, with its detached signature `SHA256SUMS.sig`. |

When you upload it, put the packages and their `.sig` first and the database files last, so a client never sees a database that names a missing file.
