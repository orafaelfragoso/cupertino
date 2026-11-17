# cupertino-keyring

The Cupertino package signing key. Arch only. Nothing here publishes a repository: the release workflow signs the packages
and writes the repository tree as an artifact of its run, and the upload waits for the host and key decisions. Risk for
Rafael and Orion: the release environments have no required reviewers, so the gate is who can push a `v*` tag; only a tag
ruleset restricting who can create `v*` tags closes the hole.

| File | Installed as | What it does |
|---|---|---|
| `PKGBUILD`, `cupertino-keyring.install` | package `cupertino-keyring` | ships the public key from `CUPERTINO_KEYRING_KEY` (an ASCII-armored export of the master key with its signing subkey; the key is not in this repository) as `/usr/share/pacman/keyrings/cupertino.gpg` and `cupertino-trusted`, then runs `pacman-key --populate cupertino` |
| `release-key.fpr` | not installed | the primary fingerprint of the release key |

Build it with `CUPERTINO_KEYRING_KEY=/path/to/PUBLIC.asc makepkg` in this directory.

## Bootstrap on a new machine

Trust the key before installing the keyring, because the keyring is signed by that key: `pacman-key --recv-keys FPR` (or
`--add KEYFILE`), `pacman-key --finger FPR` and compare all 40 hex digits with `release-key.fpr`,
`pacman-key --lsign-key FPR`, then add the `[cupertino]` section (`SigLevel = Required DatabaseRequired TrustedOnly`) and
`pacman -Sy cupertino-keyring`.

Updates are `pacman -Syu`; going back to an earlier release is `pacman -U` of the earlier `cupertino` and `cupertino-data`
with their `.sig` files, from the pacman cache or the repository's `archive/` (see the book's "Signed updates" page).
