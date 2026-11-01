#!/usr/bin/env python3
"""cupertino-install: installs Cupertino from the live USB onto one whole disk, erasing it.

Usage:
  cupertino-install
  cupertino-install --answers FILE --disk DISK --yes-erase DISK

Asked: the disk (then its path typed again), encryption, the account and its password, the time zone. archinstall 4.5
does the install from a configuration written here (--config, --creds, --silent); the Cupertino steps follow on the
mounted system: the boot entries "Cupertino", "Cupertino (LTS kernel)" and "Cupertino (recovery: text login)", the
NVIDIA driver options, Cupertino as the account's session at GDM, and the live USB's package repository taken out of
pacman.conf.

--answers reads the answers from a JSON file instead (for the QEMU test): user, password, luks (false or a passphrase),
timezone, and optionally hostname, keyboard, extra_packages and kernel_args. The disk is never in the file: --disk and
--yes-erase must both name it.

The last line on stdout and /dev/console is "cupertino-install: done DISK", or "cupertino-install: failed: STEP: REASON"
with exit status 2 (arguments or answers), 3 (disk refused), 4 (archinstall) or 5 (the Cupertino steps).
"""
from __future__ import annotations

import argparse
import getpass
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from dataclasses import dataclass, field
from pathlib import Path
from uuid import uuid4

ARCHINSTALL_VERSION = "4.5"
MIB = 1024**2
GIB = 1024**3
SMALLEST_DISK = 32 * 1000**3
TARGET = Path("/mnt")
LIVE = Path("/run/archiso")
ARCHINSTALL_LOG = Path("/var/log/archinstall/install.log")
RECOVERY_ENTRY = "cupertino-recovery.conf"
SESSION = "cupertino"
BTRFS_SUBVOLUMES = (("@", "/"), ("@home", "/home"), ("@log", "/var/log"), ("@pkg", "/var/cache/pacman/pkg"))
# GNOME as the second session at GDM, for when the Cupertino session fails
GNOME_PACKAGES = ("gdm", "gnome-shell", "gnome-session", "gnome-control-center", "gnome-keyring", "gnome-terminal",
                  "nautilus", "xdg-desktop-portal-gnome")
REPOSITORY = "cupertino"
ZONEINFO = Path("/usr/share/zoneinfo")

BASE_PACKAGES = (
    "base-devel", "bluez-utils", "chromium", "foot", "git", "github-cli", "nano", "pipewire-pulse", "rustup", "sudo",
    "snap-pac", "wireplumber", "xorg-xwayland", "xwayland-satellite", "cupertino", "cupertino-data",
)
CHROMIUM_FLAGS = "--ozone-platform=wayland\n"
KERNELS = ("linux", "linux-lts")
NVIDIA_PACKAGES = ("nvidia-open", "nvidia-open-lts", "nvidia-utils", "libva-nvidia-driver")
# the working state of the RTX 3060 this installer is first used on: its modprobe.d, its driver parameters and its early KMS
NVIDIA_OPTIONS = (
    "options nvidia_drm modeset=1 fbdev=1\n"
    "options nvidia NVreg_UseKernelSuspendNotifiers=1 NVreg_PreserveVideoMemoryAllocations=1\n"
)
NVIDIA_EARLY_KMS = "MODULES+=(nvidia nvidia_modeset nvidia_uvm nvidia_drm)\n"
# with UseKernelSuspendNotifiers the driver suspends itself; these units would do it a second time
NVIDIA_SLEEP_UNITS = ("nvidia-suspend.service", "nvidia-resume.service", "nvidia-hibernate.service",
                      "nvidia-suspend-then-hibernate.service")
SECURE_BOOT = Path("/sys/firmware/efi/efivars/SecureBoot-8be4df61-93ca-11d2-aa0d-00e098032b8c")
NVIDIA = 0x10DE
AMD = 0x1002
INTEL = 0x8086
# the first Turing device; nvidia-open supports Turing and newer only
FIRST_TURING = 0x1E00
ACCOUNT = re.compile(r"[a-z_][a-z0-9_-]{0,31}")
HOSTNAME = re.compile(r"[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?")


class Failed(Exception):
    def __init__(self, status: int, step: str, reason: str) -> None:
        super().__init__(f"{step}: {reason}")
        self.status = status
        self.step = step
        self.reason = reason


@dataclass(frozen=True)
class Disk:
    path: str
    size: int
    model: str
    sector: int
    partitions: tuple[str, ...]
    refused: str | None


@dataclass(frozen=True)
class Answers:
    user: str
    password: str
    luks: str | None
    timezone: str
    hostname: str = "cupertino"
    keyboard: str = "us"
    extra_packages: tuple[str, ...] = ()
    kernel_args: tuple[str, ...] = ()


@dataclass(frozen=True)
class Graphics:
    packages: tuple[str, ...]
    nvidia: bool
    only_nvidia: bool
    summary: str


@dataclass
class Plan:
    disk: Disk
    answers: Answers
    graphics: Graphics
    ids: dict[str, str] = field(default_factory=lambda: {"esp": str(uuid4()), "root": str(uuid4())})


def disks(lsblk: dict[str, object]) -> list[Disk]:
    """The whole disks in `lsblk -J -b -o PATH,SIZE,MODEL,TYPE,RO,LOG-SEC,MOUNTPOINTS,FSTYPE,LABEL`, each with why it is
    refused (None when it may be erased)."""
    found = []
    for device in lsblk.get("blockdevices", []):  # type: ignore[union-attr]
        if device.get("type") != "disk" or device["path"].startswith("/dev/zram"):
            continue
        children = list(descendants(device))
        size = int(device["size"])
        refused = None
        if device.get("ro"):
            refused = "read-only"
        elif any(node.get("fstype") == "iso9660" for node in [device, *children]):
            # with copytoram the live USB is not mounted
            refused = "the live USB"
        elif any(m for node in [device, *children] for m in node.get("mountpoints") or [] if m):
            refused = "in use (mounted, swap, or the USB this system started from)"
        elif any(c.get("type") in ("crypt", "lvm", "raid0", "raid1", "raid5", "raid6", "raid10") for c in children):
            refused = "in use (an open encrypted, LVM or RAID volume)"
        elif size < SMALLEST_DISK:
            refused = f"smaller than {SMALLEST_DISK // 1000**3} GB"
        partitions = tuple(
            " ".join(filter(None, [c["path"], size_text(int(c["size"])), c.get("fstype"), c.get("label")]))
            for c in device.get("children") or []
        )
        found.append(Disk(device["path"], size, (device.get("model") or "").strip(), int(device.get("log-sec") or 512), partitions, refused))
    return found


def descendants(node: dict[str, object]):
    for child in node.get("children") or []:  # type: ignore[union-attr]
        yield child
        yield from descendants(child)


def size_text(size: int) -> str:
    return f"{size / 1000**3:.0f} GB" if size >= 1000**3 else f"{size / 1000**2:.0f} MB"


def graphics(devices: list[tuple[int, int, int]]) -> Graphics:
    """The driver packages for the PCI devices (class, vendor, device) of the machine."""
    display = [(vendor, device) for cls, vendor, device in devices if cls >> 16 == 0x03]
    packages = ["mesa"]
    summary = []
    nvidia = False
    for vendor, device in display:
        if vendor == NVIDIA and device >= FIRST_TURING:
            nvidia = True
            summary.append(f"NVIDIA {device:04x}: nvidia-open")
        elif vendor == NVIDIA:
            summary.append(f"NVIDIA {device:04x}: older than Turing, nouveau (mesa)")
        elif vendor == AMD:
            packages.append("vulkan-radeon")
            summary.append(f"AMD {device:04x}: mesa")
        elif vendor == INTEL:
            packages.append("vulkan-intel")
            summary.append(f"Intel {device:04x}: mesa")
    if nvidia:
        packages.extend(NVIDIA_PACKAGES)
    only_nvidia = nvidia and all(vendor == NVIDIA for vendor, _ in display)
    return Graphics(tuple(dict.fromkeys(packages)), nvidia, only_nvidia, "; ".join(summary) or "no graphics card found: mesa")


def pci_devices(root: Path = Path("/sys/bus/pci/devices")) -> list[tuple[int, int, int]]:
    return [
        tuple(int((d / name).read_text(), 16) for name in ("class", "vendor", "device"))  # type: ignore[misc]
        for d in sorted(root.iterdir())
    ]


def size(value: int, sector: int) -> dict[str, object]:
    return {"value": value, "unit": "MiB", "sector_size": {"value": sector, "unit": "B"}}


def configuration(plan: Plan) -> dict[str, object]:
    """archinstall 4.5's user_configuration.json: its default layout (a 1 GiB ESP at /boot, the rest btrfs with its
    default subvolumes and snapper, the root partition in LUKS when asked), linux and linux-lts, systemd-boot,
    NetworkManager, PipeWire, Bluetooth, zram and GDM."""
    disk, answers = plan.disk, plan.answers
    # archinstall's default layout: the last partition ends 1 MiB before the disk's end (the backup GPT), MiB-aligned
    root_start = 1 + 1024
    root_length = (disk.size - MIB) // MIB - root_start
    partition = {"status": "create", "type": "primary", "mount_options": [], "dev_path": None, "btrfs": []}
    disk_config: dict[str, object] = {
        "config_type": "default_layout",
        "device_modifications": [{
            "device": disk.path,
            "wipe": True,
            "partitions": [
                {**partition, "obj_id": plan.ids["esp"], "start": size(1, disk.sector), "size": size(1024, disk.sector),
                 "fs_type": "fat32", "mountpoint": "/boot", "flags": ["boot", "esp"]},
                {**partition, "obj_id": plan.ids["root"], "start": size(root_start, disk.sector),
                 "size": size(root_length, disk.sector), "fs_type": "btrfs", "mountpoint": None, "flags": [],
                 "mount_options": ["compress=zstd"],
                 "btrfs": [{"name": name, "mountpoint": mountpoint} for name, mountpoint in BTRFS_SUBVOLUMES]},
            ],
        }],
        "btrfs_options": {"snapshot_config": {"type": "Snapper"}},
    }
    if answers.luks is not None:
        disk_config["disk_encryption"] = {"encryption_type": "luks", "partitions": [plan.ids["root"]], "lvm_volumes": []}
    return {
        "app_config": {"audio_config": {"audio": "pipewire"}, "bluetooth_config": {"enabled": True}},
        "bootloader_config": {"bootloader": "Systemd-boot", "uki": False, "removable": True},
        "disk_config": disk_config,
        "hostname": answers.hostname,
        "kernels": list(KERNELS),
        "locale_config": {"kb_layout": answers.keyboard, "sys_enc": "UTF-8", "sys_lang": "en_US.UTF-8"},
        "network_config": {"type": "nm"},
        "ntp": True,
        "packages": list(dict.fromkeys([*BASE_PACKAGES, *GNOME_PACKAGES, *plan.graphics.packages, *answers.extra_packages])),
        "services": ["gdm"],
        "swap": {"enabled": True, "algorithm": "zstd"},
        "timezone": answers.timezone,
    }


def credentials(answers: Answers) -> dict[str, object]:
    creds: dict[str, object] = {"users": [{"username": answers.user, "!password": answers.password, "sudo": True, "groups": []}]}
    if answers.luks is not None:
        creds["encryption_password"] = answers.luks
    return creds


def entry(entries: Path, kernel: str) -> tuple[Path, list[str]]:
    found = sorted(entries.glob(f"*_{kernel}.conf"))
    if len(found) != 1:
        raise Failed(5, "boot entries", f"expected one archinstall entry for {kernel} in {entries}, found {len(found)}")
    lines = found[0].read_text().splitlines()
    if not any(line.startswith("options") for line in lines):
        raise Failed(5, "boot entries", f"{found[0].name} has no options line")
    return found[0], lines


def retitled(lines: list[str], title: str, extra: str) -> list[str]:
    return [f"title\t{title}" if line.startswith("title") else line + extra if line.startswith("options") else line for line in lines]


def boot_entries(entries: Path, kernel_args: tuple[str, ...]) -> None:
    """Retitles archinstall's entries "Cupertino" and "Cupertino (LTS kernel)", and writes the recovery entry beside
    them: the main system started to multi-user.target, which does not pull in display-manager.service, so tty1 shows a
    text login."""
    extra = "".join(f" {arg}" for arg in kernel_args)
    lts, lts_lines = entry(entries, "linux-lts")
    found, lines = entry(entries, "linux")
    main = retitled(lines, "Cupertino", extra)
    lts.write_text("\n".join(retitled(lts_lines, "Cupertino (LTS kernel)", extra)) + "\n")
    recovery = [
        "title\tCupertino (recovery: text login)" if line.startswith("title")
        else line + " systemd.unit=multi-user.target" if line.startswith("options") else line
        for line in main
    ]
    found.write_text("\n".join(main) + "\n")
    (entries / RECOVERY_ENTRY).write_text("\n".join(recovery) + "\n")


def without_repository(pacman_conf: str, name: str = REPOSITORY) -> str:
    """pacman.conf without the [name] section (the live USB's own repository)."""
    kept, inside = [], False
    for line in pacman_conf.splitlines(keepends=True):
        header = re.fullmatch(r"\s*\[([^]]+)\]\s*", line)
        if header:
            inside = header.group(1) == name
        if not inside:
            kept.append(line)
    return "".join(kept)


def parse_answers(data: object) -> Answers:
    if not isinstance(data, dict):
        raise Failed(2, "answers", "the file is not a JSON object")
    known = {"user", "password", "luks", "timezone", "hostname", "keyboard", "extra_packages", "kernel_args"}
    if unknown := sorted(set(data) - known):
        raise Failed(2, "answers", f"unknown keys {', '.join(unknown)}")
    for key in ("user", "password", "timezone"):
        if not isinstance(data.get(key), str) or not data[key]:
            raise Failed(2, "answers", f"{key} must be a non-empty string")
    luks = data.get("luks", False)
    if luks is not False and (not isinstance(luks, str) or not luks):
        raise Failed(2, "answers", "luks must be false or a non-empty passphrase")
    lists = {}
    for key in ("extra_packages", "kernel_args"):
        value = data.get(key, [])
        if not isinstance(value, list) or not all(isinstance(v, str) and v and not re.search(r"\s", v) for v in value):
            raise Failed(2, "answers", f"{key} must be a list of strings without spaces")
        lists[key] = tuple(value)
    answers = Answers(
        data["user"], data["password"], luks or None, data["timezone"], data.get("hostname", "cupertino"),
        data.get("keyboard", "us"), lists["extra_packages"], lists["kernel_args"],
    )
    check(answers)
    return answers


def check(answers: Answers) -> None:
    if not ACCOUNT.fullmatch(answers.user) or answers.user == "root":
        raise Failed(2, "answers", f"{answers.user!r} is not a valid account name (lower-case letters, digits, - and _)")
    if not HOSTNAME.fullmatch(answers.hostname):
        raise Failed(2, "answers", f"{answers.hostname!r} is not a valid computer name")
    zone = (ZONEINFO / answers.timezone).resolve()
    if ".." in Path(answers.timezone).parts or not zone.is_file() or not zone.is_relative_to(ZONEINFO.resolve()):
        raise Failed(2, "answers", f"{answers.timezone!r} is not a time zone")


def run(*command: str, step: str, status: int, capture: bool = False) -> str:
    result = subprocess.run(command, text=True, capture_output=capture)
    if result.returncode != 0:
        detail = (result.stderr or "").strip().splitlines()[-1:] if capture else []
        raise Failed(status, step, f"{' '.join(command)} exited with {result.returncode}" + "".join(f": {d}" for d in detail))
    return result.stdout if capture else ""


def preflight() -> None:
    if os.geteuid() != 0:
        raise Failed(2, "start", "run it as root: sudo cupertino-install")
    if not LIVE.is_dir():
        raise Failed(2, "start", "this is not the live USB; cupertino-install only runs there")
    if not Path("/sys/firmware/efi").is_dir():
        raise Failed(2, "start", "this computer started in legacy BIOS mode; start the USB in UEFI mode")
    if SECURE_BOOT.exists() and SECURE_BOOT.read_bytes()[-1:] == b"\x01":
        raise Failed(2, "start", "Secure Boot is on; turn it off in the firmware settings (the boot loader and drivers are not signed)")
    version = run("pacman", "-Q", "archinstall", step="start", status=2, capture=True).split()[1]
    if not re.fullmatch(re.escape(ARCHINSTALL_VERSION) + r"(-\d+)?", version):
        raise Failed(2, "start", f"archinstall {version} is not {ARCHINSTALL_VERSION}, the version this installer was written for")
    if subprocess.run(["findmnt", str(TARGET)], capture_output=True).returncode == 0:
        raise Failed(2, "start", f"{TARGET} is in use; unmount it first")


def listed_disks() -> list[Disk]:
    out = run("lsblk", "-J", "-b", "-o", "PATH,SIZE,MODEL,TYPE,RO,LOG-SEC,MOUNTPOINTS,FSTYPE,LABEL", step="disk", status=3, capture=True)
    return disks(json.loads(out))


def ask(prompt: str, default: str | None = None) -> str:
    answer = input(f"{prompt}{f' [{default}]' if default else ''}: ").strip()
    return answer or (default or "")


def ask_secret(prompt: str) -> str:
    while True:
        first = getpass.getpass(f"{prompt}: ")
        if not first:
            print("It cannot be empty.")
        elif getpass.getpass("Again: ") == first:
            return first
        else:
            print("They differ; try again.")


def choose_disk() -> Disk:
    while True:
        found = listed_disks()
        print("\nDisks:")
        for number, disk in enumerate(found, 1):
            print(f"  {number}. {disk.path}  {size_text(disk.size)}  {disk.model or 'unknown model'}"
                  + (f"  (cannot be used: {disk.refused})" if disk.refused else ""))
            for partition in disk.partitions:
                print(f"       {partition}")
        usable = [d for d in found if d.refused is None]
        if not usable:
            raise Failed(3, "disk", "no disk can be used (each one is listed with the reason)")
        choice = ask("Number of the disk to install on")
        if not choice.isdigit() or not 1 <= int(choice) <= len(found) or found[int(choice) - 1].refused:
            print("That is not a disk that can be used.")
            continue
        disk = found[int(choice) - 1]
        print(f"\nEverything on {disk.path} ({size_text(disk.size)}, {disk.model or 'unknown model'}) will be erased.")
        if ask(f"Type {disk.path} to erase it") == disk.path:
            return disk
        print("That does not match; nothing was erased.")


def interactive_answers(live_timezone: str) -> Answers:
    luks: str | None = None
    if ask("Encrypt the disk (a passphrase is asked at every start)? yes/no", "yes").lower() in ("y", "yes"):
        luks = ask_secret("Encryption passphrase")
    elif ask("Without encryption, anyone with the disk can read it. Type yes to continue without it").lower() != "yes":
        luks = ask_secret("Encryption passphrase")
    while True:
        user = ask("Account name (lower case, for example rafael)")
        if ACCOUNT.fullmatch(user) and user != "root":
            break
        print("Use lower-case letters, digits, - and _, starting with a letter.")
    password = ask_secret(f"Password for {user}")
    while True:
        answers = Answers(user, password, luks, ask("Time zone", live_timezone))
        try:
            check(answers)
            return answers
        except Failed as e:
            print(e.reason)


def install(plan: Plan, workdir: Path) -> None:
    config, creds = workdir / "user_configuration.json", workdir / "user_credentials.json"
    config.write_text(json.dumps(configuration(plan), indent=4))
    fd = os.open(creds, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "w") as out:
        json.dump(credentials(plan.answers), out)
    try:
        result = subprocess.run(["archinstall", "--config", str(config), "--creds", str(creds), "--silent"])
    finally:
        creds.unlink()
    # archinstall can return 0 without installing (a refused configuration): the mounted system is the proof
    missing = [p for p in ("usr/bin/cupertino-sessiond", "boot/loader/loader.conf") if not (TARGET / p).exists()]
    # an absolute symlink: read it, do not follow it into the live system
    display_manager = TARGET / "etc/systemd/system/display-manager.service"
    if not display_manager.is_symlink() or Path(os.readlink(display_manager)).name != "gdm.service":
        missing.append("display-manager.service -> gdm.service")
    if result.returncode != 0 or missing:
        if ARCHINSTALL_LOG.exists():
            shutil.copy(ARCHINSTALL_LOG, workdir / "archinstall.log")
        raise Failed(4, "archinstall", f"exit status {result.returncode}" + (f", missing {', '.join(missing)}" if missing else "")
                     + f"; its log: {workdir / 'archinstall.log'}")
    if plan.answers.luks is not None:
        root = run("findmnt", "-n", "--nofsroot", "-o", "SOURCE", str(TARGET), step="encryption", status=4, capture=True).strip()
        kind = run("lsblk", "-n", "-o", "TYPE", root, step="encryption", status=4, capture=True).split()
        if kind[:1] != ["crypt"]:
            raise Failed(4, "encryption", f"{root} is not an encrypted volume; archinstall did not encrypt the disk")


def without_kms_hook(mkinitcpio_conf: str) -> str:
    """mkinitcpio.conf without the kms hook: with nvidia_drm loaded early and no other GPU it only adds nouveau."""
    def strip(match: re.Match) -> str:
        return "HOOKS=(" + " ".join(h for h in match.group(1).split() if h != "kms") + ")"
    return re.sub(r"^HOOKS=\(([^)]*)\)", strip, mkinitcpio_conf, flags=re.MULTILINE)


def nvidia(only_nvidia: bool) -> None:
    # nvidia-open and nvidia-open-lts are built for one kernel release each: a module for each installed kernel proves the match
    for kernel in KERNELS:
        modules = [p.name for p in (TARGET / "usr/lib/modules").iterdir() if (p / "pkgbase").is_file()
                   and (p / "pkgbase").read_text().strip() == kernel]
        if len(modules) != 1:
            raise Failed(5, "nvidia", f"expected one module directory for {kernel}, found {modules}")
        run("arch-chroot", str(TARGET), "modinfo", "-k", modules[0], "-F", "version", "nvidia", step="nvidia", status=5, capture=True)
    (TARGET / "etc/modprobe.d").mkdir(parents=True, exist_ok=True)
    (TARGET / "etc/modprobe.d/nvidia.conf").write_text(NVIDIA_OPTIONS)
    (TARGET / "etc/mkinitcpio.conf.d").mkdir(parents=True, exist_ok=True)
    (TARGET / "etc/mkinitcpio.conf.d/nvidia.conf").write_text(NVIDIA_EARLY_KMS)
    if only_nvidia:
        conf = TARGET / "etc/mkinitcpio.conf"
        conf.write_text(without_kms_hook(conf.read_text()))
    units = [u for u in NVIDIA_SLEEP_UNITS if (TARGET / "usr/lib/systemd/system" / u).exists()]
    if units:
        run("arch-chroot", str(TARGET), "systemctl", "disable", "--quiet", *units, step="nvidia", status=5)
    run("arch-chroot", str(TARGET), "mkinitcpio", "-P", step="nvidia", status=5)


def finish(plan: Plan) -> None:
    boot_entries(TARGET / "boot/loader/entries", plan.answers.kernel_args)
    if plan.graphics.nvidia:
        nvidia(plan.graphics.only_nvidia)
    pacman_conf = TARGET / "etc/pacman.conf"
    pacman_conf.write_text(without_repository(pacman_conf.read_text()))
    config = TARGET / "home" / plan.answers.user / ".config"
    config.mkdir(exist_ok=True)
    (config / "chromium-flags.conf").write_text(CHROMIUM_FLAGS)
    run("arch-chroot", str(TARGET), "chown", "-R", f"{plan.answers.user}:{plan.answers.user}", f"/home/{plan.answers.user}/.config",
        step="chromium", status=5)
    if not (TARGET / f"usr/share/wayland-sessions/{SESSION}.desktop").is_file():
        raise Failed(5, "session", f"/usr/share/wayland-sessions/{SESSION}.desktop is missing")
    users = TARGET / "var/lib/AccountsService/users"
    users.mkdir(mode=0o700, parents=True, exist_ok=True)
    # GDM preselects the session AccountsService records for the account
    (users / plan.answers.user).write_text(f"[User]\nSession={SESSION}\nSystemAccount=false\n")
    (users / plan.answers.user).chmod(0o600)
    for unit, wanted in (("NetworkManager.service", "enabled"), ("bluetooth.service", "enabled"), ("gdm.service", "enabled"),
                         *((u, "not enabled") for u in ("cupertino-login.service", "cupertino-netd.service",
                                                       "cupertino-btd.service", "cupertino-powerd.service"))):
        state = subprocess.run(["arch-chroot", str(TARGET), "systemctl", "is-enabled", unit], text=True, capture_output=True).stdout.strip()
        if (state == "enabled") != (wanted == "enabled"):
            raise Failed(5, "services", f"{unit} is {state or 'missing'}, it should be {wanted}")
    for package in plan.answers.extra_packages:
        unit = TARGET / f"usr/lib/systemd/system/{package}.service"
        if unit.exists() and "[Install]" in unit.read_text():
            run("arch-chroot", str(TARGET), "systemctl", "enable", f"{package}.service", step="services", status=5)
    print("cupertino-server --check-hardware (in the new system, before its first start):")
    try:
        check_hardware = subprocess.run(["arch-chroot", str(TARGET), "cupertino-server", "--check-hardware"], text=True,
                                        capture_output=True, timeout=60)
        print((check_hardware.stdout + check_hardware.stderr).strip() or f"exit status {check_hardware.returncode}")
    except subprocess.TimeoutExpired:
        print("no answer within 60 s")
    root = run("findmnt", "-n", "--nofsroot", "-o", "SOURCE", str(TARGET), step="unmount", status=5, capture=True).strip()
    subprocess.run(["sync"])
    run("umount", "-R", str(TARGET), step="unmount", status=5)
    if plan.answers.luks is not None:
        run("cryptsetup", "close", Path(root).name, step="unmount", status=5)


def say(line: str) -> None:
    print(line, flush=True)
    try:
        with open("/dev/console", "w") as console:
            console.write(line + "\n")
    except OSError:
        pass


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(prog="cupertino-install", description="Install Cupertino on one whole disk, erasing it.")
    parser.add_argument("--answers", type=Path, help="JSON answers file (for the QEMU test)")
    parser.add_argument("--disk", help="with --answers: the disk to erase")
    parser.add_argument("--yes-erase", help="with --answers: the same disk again")
    args = parser.parse_args(argv)
    try:
        if args.answers is not None and (not args.disk or args.disk != args.yes_erase):
            raise Failed(2, "arguments", "--answers needs --disk DISK and --yes-erase DISK, naming the same disk")
        if args.answers is None and (args.disk or args.yes_erase):
            raise Failed(2, "arguments", "--disk and --yes-erase go with --answers")
        preflight()
        live_timezone = subprocess.run(["timedatectl", "show", "-p", "Timezone", "--value"], text=True, capture_output=True).stdout.strip()
        if args.answers is not None:
            try:
                answers = parse_answers(json.loads(args.answers.read_text()))
            except (OSError, ValueError) as e:
                raise Failed(2, "answers", str(e)) from e
            disk = next((d for d in listed_disks() if d.path == args.disk), None)
            if disk is None or disk.refused:
                raise Failed(3, "disk", f"{args.disk}: {'not a disk' if disk is None else disk.refused}")
            workdir = args.answers.resolve().parent
        else:
            print("Cupertino installer. It erases one whole disk and installs Cupertino on it.")
            disk = choose_disk()
            answers = interactive_answers(live_timezone or "UTC")
            workdir = Path(tempfile.mkdtemp(prefix="cupertino-install-", dir="/run"))
        plan = Plan(disk, answers, graphics(pci_devices()))
        print(f"\nDisk: {disk.path} ({size_text(disk.size)}), erased whole\nEncryption: {'yes' if answers.luks else 'no'}\n"
              f"Account: {answers.user} (administrator)\nGraphics: {plan.graphics.summary}\nTime zone: {answers.timezone}\n"
              f"Computer name: {answers.hostname}, keyboard: {answers.keyboard}")
        if args.answers is None and ask("Start? yes/no", "no").lower() not in ("y", "yes"):
            raise Failed(2, "summary", "not started; nothing was erased")
        install(plan, workdir)
        finish(plan)
    except Failed as e:
        say(f"cupertino-install: failed: {e.step}: {e.reason}")
        return e.status
    except KeyboardInterrupt:
        say("cupertino-install: failed: interrupted")
        return 2
    say(f"cupertino-install: done {plan.disk.path}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
