# Install Cupertino from a USB stick

This page takes you from the ISO file to your first login. The installer erases one whole disk and installs Arch Linux with Cupertino as the only desktop. There is no dual boot. Back up your data before you start; this page has no commands for that.

The ISO and the installer have not been booted on real hardware yet. The commands here match `scripts/build-iso.sh` and `packaging/installer/cupertino_install.py`; where a result is what the code does and not something observed, the page says so.

You need:

- a UEFI machine (the image has no BIOS boot mode),
- a USB stick larger than the ISO (about 2.1 GiB; `ls -l` on the file gives its exact size, and the `.sha256` file next to it checks it),
- a network connection during the install, because the installer downloads the Arch packages.

## Build or download the image

The ISO is built from a clean checkout of `main` with Docker and `git` installed:

```sh
scripts/build-iso.sh [OUT_DIR]
```

`OUT_DIR` defaults to `~/.cache/cupertino-iso`, and the build refuses to start with less than 48 GB free there. It writes three files:

| File | Content |
|---|---|
| `cupertino-YYYY.MM.DD-COMMIT-x86_64.iso` | The live and install image. The date is the commit date in UTC. |
| `cupertino-YYYY.MM.DD-COMMIT-x86_64.iso.sha256` | The output of `sha256sum` for the ISO. |
| `cupertino-YYYY.MM.DD-COMMIT-x86_64.iso.manifest` | The commit, the archiso version, the Cupertino packages and every installed package with its version. |

The image is not signed. The checksum is the only integrity check. In the folder that holds the files:

```sh
sha256sum -c cupertino-*.iso.sha256
```

It prints `OK` for the ISO.

## Write the ISO to the stick

Find the stick. You need the whole disk (`sdX`), not a partition (`sdX1`). Check the model and size:

```sh
lsblk -o NAME,MODEL,SIZE,TRAN
```

Write the image. This erases the stick:

```sh
sudo dd if=cupertino-*.iso of=/dev/sdX bs=4M status=progress conv=fsync
```

Check the stick against the image. No output means they match:

```sh
sudo cmp -n "$(stat -c %s cupertino-*.iso)" cupertino-*.iso /dev/sdX
```

## Boot the stick

1. In the firmware settings, set UEFI mode and turn Secure Boot off. The boot loader and the drivers are not signed, and the installer stops when Secure Boot is on.
2. Open the firmware's boot menu (the key depends on the maker) and pick the stick.
3. The boot menu has one entry, "Cupertino live (x86_64, UEFI)", and starts it after 3 seconds. The kernel command line sets `nvidia-drm.modeset=1` and `nvidia-drm.fbdev=1`, and the image ships `nvidia-open`.

The live session starts straight into Cupertino as the user `live`, with no password and no login screen. `Ctrl+Alt+F2` gives a text login on tty2: log in as `root` with no password (the `live` account is locked). The network comes from `systemd-networkd`; for Wi-Fi from a text login, use `iwctl`.

## Install

In the live session, run this in a terminal or in a text login (the image also ships an **Install Cupertino** entry that opens `foot` and runs the same command):

```sh
sudo cupertino-install
```

The installer checks first that it runs as root on the live USB, in UEFI mode, with Secure Boot off and with archinstall 4.5. It then asks:

1. **The disk.** It lists every disk with its path, its size and its model. The sizes are in binary units (GiB, TiB) and read the same as `lsblk -o NAME,MODEL,SIZE`, so a "1 TB" drive shows as about 931.5 GiB. The installer refuses disks under 32 GB (29.8 GiB), and marks the ones it cannot use, such as the stick, with the reason. Type the number of the disk. It then says what will be erased and asks you to type the disk's path, for example `/dev/nvme0n1`. A path that does not match erases nothing.
2. **The account name,** in lower case. Then the password, twice. The account is an administrator through `sudo` with its password; root stays locked.
3. **The time zone.** The default is the live session's.

It then prints a summary (disk, account, graphics, time zone, computer name and keyboard) and asks `Start? yes/no`. The default is `no`.

The last line of the output is `cupertino-install: done DISK` on success, or `cupertino-install: failed: STEP: REASON` on failure. When it says done, remove the stick and reboot.

What it installs:

| Part | Content |
|---|---|
| Disk | A 1 GiB FAT32 boot partition and ext4 for the rest of the disk. No encryption. zram swap. |
| Boot | systemd-boot with a 3 second menu: "Cupertino" (the default) and "Cupertino (recovery: text login)". |
| Graphics | `mesa`. On an NVIDIA card from the Turing generation on: `nvidia-open`, `nvidia-utils` and `libva-nvidia-driver`, with `nvidia_drm modeset=1 fbdev=1` and early loading of the driver. Older NVIDIA cards use nouveau. |
| Login | `cupertino-login.service` on tty1. Text logins on tty2 to tty6. |
| Services | NetworkManager, BlueZ and `cupertino-powerd`. |
| System | PipeWire, `foot` and `xwayland-satellite`. |
| Apps | Chromium (on Wayland), `git`, `base-devel`, `rustup` and `gh`. |

The installer fails before it unmounts the disk if the new system keeps anything of the live USB: the `[cupertino]` repository, `NOPASSWD` sudo, a root password or another account.

## First login

Boot from the disk, not the stick. The default entry, "Cupertino", starts `cupertino-login` on tty1. Log in with the account you created. For shortcuts, logs and the known limits, see [Using Cupertino every day](daily-use.md). The installed system starts the session from `cupertino-login`; the session-chooser steps there are for machines that use GDM.

## When the session does not start

| Problem | What to do |
|---|---|
| The login screen fails repeatedly | After 5 failures in 30 seconds, `cupertino-sessiond` stops restarting the login screen. `Ctrl+Alt+F2` to `Ctrl+Alt+F6` give a text login. |
| The login screen is broken | At the boot menu, pick "Cupertino (recovery: text login)". tty1 shows a text login. |
| No network in a text login | Run `nmtui`. |
| A bad package update | `sudo pacman -U /var/cache/pacman/pkg/NAME-OLDVERSION-*.pkg.tar.zst` |

## Known limits

- UEFI only, with Secure Boot off. One disk, erased whole. No dual boot.
- No disk encryption.
- One kernel (`linux`). A kernel or driver update that breaks booting has no documented repair here.
- Not yet observed on real hardware: the live session, the installer run, the first login, the recovery entry and the NVIDIA setup.
