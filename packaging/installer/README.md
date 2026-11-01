# cupertino-install

The installer on the live USB. It erases one whole disk and installs Arch Linux with GDM as the login screen, the
Cupertino session as the account's default and a GNOME session beside it. `cupertino-login.service` is not enabled: GDM
does not depend on the Cupertino compositor working. The USB image ships
`cupertino_install.py` as `/usr/local/bin/cupertino-install`; it is not part of the `cupertino` package.

```sh
sudo cupertino-install
```

It asks for:

1. the disk, and then its path typed again, before anything is erased;
2. encryption (LUKS2 on the root partition, default yes) and its passphrase;
3. the account name and password (an administrator, in `wheel`; root stays locked);
4. the time zone.

archinstall 4.5 does the install from a configuration the script writes (`--config`, `--creds`, `--silent`). The
installer refuses any other archinstall version, because the configuration format is pinned to it.

| Part | What is installed |
|---|---|
| Disk | GPT: a 1 GiB FAT32 ESP at `/boot`, then btrfs filling the disk (in LUKS2 when encrypted) with the subvolumes `@` (`/`), `@home`, `@log` and `@pkg` (the package cache), `compress=zstd`; snapper configs `root` and `home` with snap-pac; zram swap |
| Boot | systemd-boot with "Cupertino" (linux, the default), "Cupertino (LTS kernel)" (linux-lts) and "Cupertino (recovery: text login)"; a 3 s menu |
| Graphics | `mesa`; on an NVIDIA card from Turing (PCI device `10de:1e00`) on: `nvidia-open`, `nvidia-open-lts`, `nvidia-utils`, `libva-nvidia-driver`; `nvidia_drm modeset=1 fbdev=1`, `NVreg_UseKernelSuspendNotifiers=1`, early KMS (`MODULES+=(nvidia nvidia_modeset nvidia_uvm nvidia_drm)`, no `kms` hook when NVIDIA is the only GPU) and the `nvidia-suspend`/`resume`/`hibernate` units disabled; older NVIDIA cards stay on nouveau |
| Login | GDM; the account's session is `cupertino` (`/var/lib/AccountsService/users/ACCOUNT`); GNOME (`gnome-shell`, `gnome-session`, `gnome-control-center`, `gnome-keyring`, `gnome-terminal`, `nautilus`, `xdg-desktop-portal-gnome`) is the second session |
| System | NetworkManager (enabled, so a text login has a network: `nmtui`), PipeWire, Bluetooth, `foot`, `xwayland-satellite` |
| Apps | Chromium (native Wayland: `~/.config/chromium-flags.conf` holds `--ozone-platform=wayland`), `git`, `base-devel`, `rustup`, `github-cli` |
| Cupertino | `cupertino` and `cupertino-data` from the USB's `[cupertino]` repository, which is then taken out of the new `pacman.conf` |

## When the Cupertino session does not start

- At GDM, pick the account, then the GNOME session in the gear menu before the password.
- Ctrl+Alt+F3 to F6 give a text login.
- The boot menu's "Cupertino (recovery: text login)" starts the system without the login manager
  (`systemd.unit=multi-user.target`): tty1 shows a text login.
- "Cupertino (LTS kernel)" starts linux-lts with nvidia-open-lts, for when a kernel or driver update breaks the main one.
- `nmtui` sets up the network from a text login.
- A bad package update: `sudo pacman -U /var/cache/pacman/pkg/NAME-OLDVERSION-*.pkg.tar.zst`, or undo a whole pacman
  transaction with snapper (`snapper -c root list` shows snap-pac's pre and post pairs; `sudo snapper -c root undochange
  PRE..POST`). `/boot` is not in the snapshots, so a kernel is rolled back with the LTS entry and `pacman -U`.

## Testing

`python3 -I packaging/installer/test_cupertino_install.py` covers the disk filter, the driver choice, the archinstall
configuration, the boot entries and the answers file. In QEMU, `cupertino-install --answers FILE --disk DISK --yes-erase
DISK` runs without questions: FILE is a JSON object with `user`, `password`, `luks` (false or a passphrase), `timezone`
and optionally `hostname`, `keyboard`, `extra_packages` and `kernel_args`. The disk is never in the file. The last line is
`cupertino-install: done DISK` or `cupertino-install: failed: STEP: REASON`, with exit status 2 (arguments or answers),
3 (disk refused), 4 (archinstall) or 5 (the Cupertino steps).

## Known limitations

- UEFI only, with Secure Boot off. One disk, erased whole: no dual boot.
- Installing needs a network for the Arch packages.
- NVIDIA cards older than Turing get nouveau; Arch has no proprietary driver for them.
- systemd-boot cannot start a snapshot: snapper rollback works from a running system (undochange), not from the boot menu.
- No polkit agent in the package yet, so administrator actions go through `sudo` in a terminal.
