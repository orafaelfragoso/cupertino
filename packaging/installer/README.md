# cupertino-install

The installer on the live USB. It erases one whole disk and installs Arch Linux with the Cupertino session started by
`cupertino-login.service`. The USB image ships `cupertino_install.py` as `/usr/local/bin/cupertino-install`; it is not
part of the `cupertino` package.

```sh
sudo cupertino-install
```

It asks for:

1. the disk, and then its path typed again, before anything is erased;
2. the account name and password (an administrator, in `wheel`; root stays locked);
3. the time zone.

archinstall 4.5 does the install from a configuration the script writes (`--config`, `--creds`, `--silent`). The
installer refuses any other archinstall version, because the configuration format is pinned to it.

| Part | What is installed |
|---|---|
| Disk | GPT: a 1 GiB FAT32 ESP at `/boot`, then ext4 at `/` filling the disk; no encryption; zram swap |
| Boot | systemd-boot with "Cupertino" (the default) and "Cupertino (recovery: text login)"; a 3 s menu |
| Graphics | `mesa`; on an NVIDIA card from Turing (PCI device `10de:1e00`) on: `nvidia-open`, `nvidia-utils`, `libva-nvidia-driver`; `nvidia_drm modeset=1 fbdev=1`, `NVreg_UseKernelSuspendNotifiers=1`, early KMS (`MODULES+=(nvidia nvidia_modeset nvidia_uvm nvidia_drm)`, no `kms` hook when NVIDIA is the only GPU) and the `nvidia-suspend`/`resume`/`hibernate` units disabled; older NVIDIA cards stay on nouveau |
| Login | `cupertino-login.service` is the display manager on tty1; tty2 to tty6 give text logins |
| Services | one owner per bus name: NetworkManager (network), `bluetooth.service` (BlueZ), `cupertino-powerd` (UPower; the `upower` package is not installed). `cupertino-netd` and `cupertino-btd` are installed but not enabled |
| System | PipeWire, `foot`, `xwayland-satellite` |
| Apps | Chromium (native Wayland: `~/.config/chromium-flags.conf` holds `--ozone-platform=wayland`), `git`, `base-devel`, `rustup`, `github-cli` |
| Cupertino | `cupertino` and `cupertino-data` from the USB's `[cupertino]` repository, which is then taken out of the new `pacman.conf` |

## When the Cupertino session does not start

- After 5 greeter failures in 30 s, `cupertino-sessiond` stops restarting it; Ctrl+Alt+F2 to F6 give a text login.
- The boot menu's "Cupertino (recovery: text login)" starts the system without the login manager
  (`systemd.unit=multi-user.target`): tty1 shows a text login.
- `nmtui` sets up the network from a text login.
- A bad package update: `sudo pacman -U /var/cache/pacman/pkg/NAME-OLDVERSION-*.pkg.tar.zst`.

## Testing

`python3 -I packaging/installer/test_cupertino_install.py` covers the disk filter, the driver choice, the archinstall
configuration, the boot entries and the answers file. In QEMU, `cupertino-install --answers FILE --disk DISK --yes-erase
DISK` runs without questions: FILE is a JSON object with `user`, `password`, `timezone`
and optionally `hostname`, `keyboard`, `extra_packages` and `kernel_args`. The disk is never in the file. The last line is
`cupertino-install: done DISK` or `cupertino-install: failed: STEP: REASON`, with exit status 2 (arguments or answers),
3 (disk refused), 4 (archinstall) or 5 (the Cupertino steps).

## Known limitations

- UEFI only, with Secure Boot off. One disk, erased whole: no dual boot.
- Installing needs a network for the Arch packages.
- NVIDIA cards older than Turing get nouveau; Arch has no proprietary driver for them.
- No disk encryption.
- One kernel (linux): a kernel or driver update that breaks booting is undone from the USB (`arch-chroot`, `pacman -U`).
- No polkit agent in the package yet, so administrator actions go through `sudo` in a terminal.
