# Install Cupertino from a USB stick

This page takes you from the ISO file to your first login. The install erases one whole disk and installs Arch Linux with Cupertino as the only desktop. There is no dual boot. Back up your data before you start; this page has no commands for that.

The live ISO has VM boot evidence. The full real-hardware install, login and recovery acceptance remains open. The commands here match the ISO workflow (`.github/workflows/iso.yml`) and the profile in `packaging/iso`; where a result is what the code does and not something observed, the page says so.

You need:

- a UEFI machine (the image has no BIOS boot mode),
- a USB stick larger than the ISO (about 2.1 GiB; `ls -l` on the file gives its exact size, and the `.sha256` file next to it checks it),
- a network connection during the install, because archinstall downloads the Arch packages.

## Build or download the image

The ISO workflow (`.github/workflows/iso.yml`) builds the image from a commit: it builds the Arch packages of that commit and `cupertino-keyring` with the public release key, puts them in a `[cupertino]` repository inside the image (`/opt/cupertino-repo`) and runs `mkarchiso` on `packaging/iso`. A final release attaches the image to its draft release; the workflow can also be run by hand on any commit. It writes three files:

| File | Content |
|---|---|
| `cupertino-YYYY.MM.DD-COMMIT-x86_64.iso` | The live and install image. The date is the commit date in UTC. |
| `cupertino-YYYY.MM.DD-COMMIT-x86_64.iso.sha256` | The output of `sha256sum` for the ISO. |
| `cupertino-YYYY.MM.DD-COMMIT-x86_64.iso.manifest` | The commit, the archiso version, the Cupertino packages and every installed package with its version. |

The release workflow signs the ISO's `.sha256` file and attaches `.sha256.sig` to the draft release. Release-candidate tags skip the ISO job. The ISO itself does not carry a detached signature.

The expected primary release-key fingerprint is `25ACA804115FFB01E53B35272FC4E6B0B511CA1A`, also stored in `packaging/keyring/release-key.fpr`. The public key is not yet committed or attached by the release workflow. Obtain the public key from the maintainer and compare its complete primary fingerprint before trusting it. A checksum without a verified signature does not authenticate a download.

With that public key imported into a separate GnuPG keyring, verify the signature on the exact checksum file:

```sh
gpg --status-fd 1 --verify cupertino-YYYY.MM.DD-COMMIT-x86_64.iso.sha256.sig cupertino-YYYY.MM.DD-COMMIT-x86_64.iso.sha256
```

Require exit status 0 and a `VALIDSIG` status line whose last field equals the primary fingerprint above. A trust warning after a fresh import is expected; a missing `VALIDSIG`, a different fingerprint or a signature error means stop. Then check the ISO bytes in the folder that holds the files:

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

1. In the firmware settings, set UEFI mode and turn Secure Boot off. The boot loader and the drivers are not signed, so the image does not boot with Secure Boot on.
2. Open the firmware's boot menu (the key depends on the maker) and pick the stick.
3. The boot menu has one entry, "Cupertino live (x86_64, UEFI)", and starts it after 3 seconds. The kernel command line sets `nvidia-drm.modeset=1` and `nvidia-drm.fbdev=1`, and the image ships `nvidia-open`.

The live session starts straight into Cupertino as the user `live`, with no password and no login screen. `Ctrl+Alt+F2` gives a text login on tty2: log in as `root` with no password (the `live` account is locked). The live system runs the same services as the installed one: NetworkManager (wired networks connect on their own; Wi-Fi from Settings or `nmtui`), BlueZ for Bluetooth, `cupertino-powerd`, `systemd-resolved` and `systemd-timesyncd`.

## Install

The image ships `archinstall` for the Arch part and the Cupertino packages in its `[cupertino]` repository. Not yet observed on real hardware: the steps below are what the image and the packages provide.

1. In a text login on tty2 (`Ctrl+Alt+F2`, as `root`), run `archinstall`. Choose the disk, systemd-boot as the boot loader, an account with administrator rights (`sudo`), the **Minimal** profile with no desktop and no display manager, PipeWire for audio, NetworkManager for the network, and on an NVIDIA card from the Turing generation on the open NVIDIA driver. Add `mesa foot xwayland-satellite bluez` as additional packages. Leave archinstall without rebooting.
2. Mount the new system at `/mnt` if archinstall left it unmounted (the root partition first, then the boot partition at `/mnt/boot`), and install Cupertino from the image's repository:

   ```sh
   pacstrap -C /etc/pacman.conf /mnt cupertino cupertino-data cupertino-keyring
   arch-chroot /mnt pacman-key --populate cupertino
   arch-chroot /mnt systemctl enable cupertino-login.service bluetooth.service
   arch-chroot /mnt systemctl add-wants multi-user.target cupertino-powerd.service
   ```

   `cupertino-login.service` is the display manager on tty1; tty2 to tty6 keep their text logins. `cupertino-powerd` answers for the power service, so do not install `upower`. `cupertino-netd` and `cupertino-btd` are installed but stay disabled: NetworkManager and BlueZ own the network and Bluetooth.
3. Remove the stick and reboot.

The new system's `pacman.conf` has no `[cupertino]` section: the image's repository stays on the stick. Updates come from a signed repository or a build of your own ([Signed updates](updates-and-rollback.md)).

## First login

Boot from the disk, not the stick. The boot loader starts the system, and `cupertino-login` starts on tty1. Log in with the account you created. For shortcuts, logs and the known limits, see [Using Cupertino every day](daily-use.md). The installed system starts the session from `cupertino-login`; the session-chooser steps there are for machines that use GDM.

## When the session does not start

| Problem | What to do |
|---|---|
| The login screen fails repeatedly | After 5 failures in 30 seconds, `cupertino-sessiond` stops restarting the login screen. `Ctrl+Alt+F2` to `Ctrl+Alt+F6` give a text login. |
| The login screen is broken | At the boot menu, press `e` on the entry and add `systemd.unit=multi-user.target` to the kernel command line. tty1 shows a text login. |
| No network in a text login | Run `nmtui`. |
| A bad package update | `sudo pacman -U /var/cache/pacman/pkg/NAME-OLDVERSION-*.pkg.tar.zst` |

## Known limits

- UEFI only, with Secure Boot off. One disk, erased whole. No dual boot.
- No disk encryption.
- One kernel (`linux`). A kernel or driver update that breaks booting has no documented repair here.
- Not yet observed on real hardware: the live session, the install, the first login and the NVIDIA setup.
