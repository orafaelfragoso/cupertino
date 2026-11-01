# cupertino-install

The installer on the live USB. It erases one whole disk and installs Arch Linux with Cupertino as the only desktop:
Cupertino's login screen starts at boot (`cupertino-login.service` is the display manager). The USB image ships
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
| Disk | GPT: a 1 GiB FAT32 ESP at `/boot`, then ext4 at `/` filling the disk (in LUKS2 when encrypted); zram swap |
| Boot | systemd-boot with the entries "Cupertino" (the default) and "Cupertino (recovery: text login)"; a 3 s menu |
| Graphics | `mesa`; on an NVIDIA card from Turing (PCI device `10de:1e00`) on: `nvidia-open`, `nvidia-utils`, `libva-nvidia-driver` and `nvidia_drm modeset=1`; older NVIDIA cards stay on nouveau |
| System | NetworkManager (enabled, so a text login has a network: `nmtui`), PipeWire, Bluetooth, `foot`, `xwayland-satellite` |
| Apps | Chromium (native Wayland: `~/.config/chromium-flags.conf` holds `--ozone-platform=wayland`), `git`, `base-devel`, `rustup`, `github-cli` |
| Cupertino | `cupertino` and `cupertino-data` from the USB's `[cupertino]` repository, which is then taken out of the new `pacman.conf` |

## When the Cupertino session does not start

- Ctrl+Alt+F2 to F6 give a text login; the login manager only takes tty1.
- The boot menu's "Cupertino (recovery: text login)" starts the system without the login manager
  (`systemd.unit=multi-user.target`): tty1 shows a text login.
- From a text login, `sudo systemctl disable cupertino-login` makes every boot a text login until it is enabled again,
  and `nmtui` sets up the network.

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
- No polkit agent in the package yet, so administrator actions go through `sudo` in a terminal.
