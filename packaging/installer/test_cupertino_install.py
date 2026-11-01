#!/usr/bin/env python3
import io
import sys
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))  # isolated mode (-I) does not add the script's directory
import cupertino_install as ci  # noqa: E402

TB = 1000204886016


def part(path, size, fstype=None, label=None, mountpoints=None, children=None, kind="part"):
    return {"path": path, "size": size, "type": kind, "fstype": fstype, "label": label,
            "mountpoints": mountpoints or [None], "children": children or []}


def disk(path, size, model="", ro=False, children=None, sector=512):
    return {"path": path, "size": size, "model": model, "type": "disk", "ro": ro, "log-sec": sector,
            "mountpoints": [None], "children": children or []}


ANSWERS = ci.Answers("rafael", "secret", "phrase", "Europe/Lisbon")


class DisksTest(unittest.TestCase):
    def test_only_whole_unused_disks_of_32_gb_or_more_may_be_erased(self):
        found = ci.disks({"blockdevices": [
            disk("/dev/nvme0n1", TB, "Samsung SSD", children=[
                part("/dev/nvme0n1p1", 2 * 1024**3, "vfat", "EFI"), part("/dev/nvme0n1p2", TB - 3 * 1024**3, "crypto_LUKS")]),
            disk("/dev/sda", 32 * 1000**3, "USB stick", children=[
                part("/dev/sda1", 1400 * 1000**2, "iso9660", "ARCH_202611", ["/run/archiso/bootmnt"])]),
            disk("/dev/sdb", 16 * 1000**3, "small"),
            disk("/dev/sde", 64 * 1000**3, "swap", children=[part("/dev/sde1", 64 * 1000**3, "swap", mountpoints=["[SWAP]"])]),
            disk("/dev/sdc", 500 * 1000**3, "open", children=[
                part("/dev/sdc1", 500 * 1000**3, "crypto_LUKS", children=[part("/dev/mapper/data", 499 * 1000**3, "ext4", kind="crypt")])]),
            disk("/dev/sdd", 500 * 1000**3, "write-protected", ro=True),
            disk("/dev/zram0", 8 * 1000**3),
            {"path": "/dev/sr0", "size": 1400 * 1000**2, "type": "rom", "mountpoints": [None]},
        ]})
        self.assertEqual([(d.path, d.refused) for d in found], [
            ("/dev/nvme0n1", None),
            ("/dev/sda", "the live USB"),
            ("/dev/sdb", "smaller than 32 GB"),
            ("/dev/sde", "in use (mounted, swap, or the USB this system started from)"),
            ("/dev/sdc", "in use (an open encrypted, LVM or RAID volume)"),
            ("/dev/sdd", "read-only"),
        ])
        self.assertEqual(found[0].partitions, ("/dev/nvme0n1p1 2 GB vfat EFI", "/dev/nvme0n1p2 997 GB crypto_LUKS"))


class GraphicsTest(unittest.TestCase):
    def test_turing_and_newer_nvidia_cards_get_the_open_module(self):
        # Rafael's RTX 3060 (GA106)
        found = ci.graphics([(0x030000, 0x10DE, 0x2504), (0x040300, 0x10DE, 0x228E)])
        self.assertEqual(found.packages, ("mesa", "nvidia-open", "nvidia-utils", "libva-nvidia-driver"))
        self.assertEqual((found.nvidia, found.only_nvidia), (True, True))

    def test_cards_older_than_turing_stay_on_nouveau(self):
        found = ci.graphics([(0x030000, 0x10DE, 0x1C03)])
        self.assertEqual((found.packages, found.nvidia), (("mesa",), False))
        self.assertEqual(found.summary, "NVIDIA 1c03: older than Turing, nouveau (mesa)")

    def test_a_hybrid_laptop_gets_both_drivers(self):
        found = ci.graphics([(0x030000, 0x1002, 0x1638), (0x030200, 0x10DE, 0x25A2)])
        self.assertEqual(found.packages, ("mesa", "vulkan-radeon", "nvidia-open", "nvidia-utils", "libva-nvidia-driver"))
        self.assertEqual((found.nvidia, found.only_nvidia), (True, False))


class InitramfsTest(unittest.TestCase):
    def test_only_the_kms_hook_is_removed(self):
        conf = "# HOOKS=(base kms)\nMODULES=()\nHOOKS=(base systemd autodetect microcode modconf kms keyboard sd-vconsole block filesystems fsck)\n"
        self.assertEqual(ci.without_kms_hook(conf),
                         "# HOOKS=(base kms)\nMODULES=()\nHOOKS=(base systemd autodetect microcode modconf keyboard sd-vconsole block filesystems fsck)\n")


class ConfigurationTest(unittest.TestCase):
    def plan(self, answers=ANSWERS, sector=512):
        return ci.Plan(ci.Disk("/dev/nvme0n1", TB, "", sector, (), None), answers,
                       ci.graphics([(0x030000, 0x10DE, 0x2504)]), {"esp": "esp-id", "root": "root-id"})

    def test_the_root_partition_fills_the_disk_up_to_the_backup_gpt(self):
        config = ci.configuration(self.plan(sector=4096))
        esp, root = config["disk_config"]["device_modifications"][0]["partitions"]
        self.assertEqual((esp["start"]["value"], esp["size"]["value"], esp["mountpoint"], esp["flags"]), (1, 1024, "/boot", ["boot", "esp"]))
        self.assertEqual((root["start"]["value"], root["size"]["value"], root["fs_type"]), (1025, 952843, "ext4"))
        self.assertLessEqual((root["start"]["value"] + root["size"]["value"]) * ci.MIB, TB - ci.MIB)
        self.assertEqual(root["size"]["sector_size"], {"value": 4096, "unit": "B"})

    def test_encryption_covers_the_root_partition_only_when_asked(self):
        self.assertEqual(ci.configuration(self.plan())["disk_config"]["disk_encryption"],
                         {"encryption_type": "luks", "partitions": ["root-id"], "lvm_volumes": []})
        self.assertEqual(ci.credentials(ANSWERS)["encryption_password"], "phrase")
        plain = ci.Answers("rafael", "secret", None, "UTC")
        self.assertNotIn("disk_encryption", ci.configuration(self.plan(plain))["disk_config"])
        self.assertNotIn("encryption_password", ci.credentials(plain))

    def test_cupertino_is_the_display_manager_and_the_driver_is_installed(self):
        config = ci.configuration(self.plan(ci.Answers("rafael", "secret", None, "UTC", extra_packages=("qemu-guest-agent",))))
        self.assertEqual(config["services"], ["cupertino-login"])
        self.assertEqual(config["bootloader_config"], {"bootloader": "Systemd-boot", "uki": False, "removable": True})
        for package in ("cupertino", "cupertino-data", "nvidia-open", "chromium", "rustup", "github-cli", "qemu-guest-agent"):
            self.assertIn(package, config["packages"])


class BootEntriesTest(unittest.TestCase):
    def test_the_recovery_entry_starts_the_same_system_without_the_login_manager(self):
        with tempfile.TemporaryDirectory() as tmp:
            entries = Path(tmp)
            (entries / "2026-11-01_20-00-00_linux.conf").write_text(
                "# Created by: archinstall\ntitle\tArch Linux (linux)\nlinux\t/vmlinuz-linux\ninitrd\t/initramfs-linux.img\n"
                "options cryptdevice=PARTUUID=abc:root root=/dev/mapper/root rw rootfstype=ext4\n")
            ci.boot_entries(entries, ("console=ttyS0,115200",))
            self.assertEqual((entries / "2026-11-01_20-00-00_linux.conf").read_text().splitlines()[1:], [
                "title\tCupertino", "linux\t/vmlinuz-linux", "initrd\t/initramfs-linux.img",
                "options cryptdevice=PARTUUID=abc:root root=/dev/mapper/root rw rootfstype=ext4 console=ttyS0,115200"])
            self.assertEqual((entries / ci.RECOVERY_ENTRY).read_text().splitlines()[1:], [
                "title\tCupertino (recovery: text login)", "linux\t/vmlinuz-linux", "initrd\t/initramfs-linux.img",
                "options cryptdevice=PARTUUID=abc:root root=/dev/mapper/root rw rootfstype=ext4 console=ttyS0,115200"
                " systemd.unit=multi-user.target"])

    def test_no_archinstall_entry_is_a_failure(self):
        with tempfile.TemporaryDirectory() as tmp, self.assertRaises(ci.Failed) as failed:
            ci.boot_entries(Path(tmp), ())
        self.assertEqual(failed.exception.status, 5)


class PacmanConfTest(unittest.TestCase):
    def test_the_live_repository_is_removed(self):
        conf = "[options]\nHoldPkg = pacman\n\n[cupertino]\nSigLevel = Optional TrustAll\nServer = file:///opt/cupertino-repo\n\n[core]\nInclude = /etc/pacman.d/mirrorlist\n"
        self.assertEqual(ci.without_repository(conf), "[options]\nHoldPkg = pacman\n\n[core]\nInclude = /etc/pacman.d/mirrorlist\n")


class AnswersTest(unittest.TestCase):
    def setUp(self):
        self.zoneinfo = tempfile.TemporaryDirectory()
        (Path(self.zoneinfo.name) / "Europe").mkdir()
        (Path(self.zoneinfo.name) / "Europe/Lisbon").write_text("TZif")
        patcher = mock.patch.object(ci, "ZONEINFO", Path(self.zoneinfo.name))
        patcher.start()
        self.addCleanup(patcher.stop)
        self.addCleanup(self.zoneinfo.cleanup)

    def failure(self, data):
        with self.assertRaises(ci.Failed) as failed:
            ci.parse_answers(data)
        return failed.exception.status, failed.exception.reason

    def test_a_complete_file_is_read(self):
        answers = ci.parse_answers({"user": "rafael", "password": "pw", "luks": False, "timezone": "Europe/Lisbon",
                                    "extra_packages": ["qemu-guest-agent"], "kernel_args": ["console=ttyS0,115200"]})
        self.assertEqual(answers, ci.Answers("rafael", "pw", None, "Europe/Lisbon", "cupertino", "us", ("qemu-guest-agent",), ("console=ttyS0,115200",)))

    def test_the_disk_cannot_come_from_the_file(self):
        self.assertEqual(self.failure({"user": "rafael", "password": "pw", "timezone": "UTC", "disk": "/dev/vda"}),
                         (2, "unknown keys disk"))

    def test_bad_values_are_refused(self):
        base = {"user": "rafael", "password": "pw", "timezone": "Europe/Lisbon"}
        self.assertEqual(self.failure({**base, "user": "Root"})[0], 2)
        self.assertEqual(self.failure({**base, "timezone": "/etc/passwd"}), (2, "'/etc/passwd' is not a time zone"))
        self.assertEqual(self.failure({**base, "luks": ""}), (2, "luks must be false or a non-empty passphrase"))
        self.assertEqual(self.failure({**base, "kernel_args": ["a b"]}), (2, "kernel_args must be a list of strings without spaces"))


class MainTest(unittest.TestCase):
    def test_the_disk_must_be_named_twice_before_anything_runs(self):
        with mock.patch.object(ci, "preflight") as preflight, redirect_stdout(io.StringIO()) as out, \
                mock.patch.object(ci, "say", print):
            status = ci.main(["--answers", "a.json", "--disk", "/dev/vda", "--yes-erase", "/dev/vdb"])
        self.assertEqual(status, 2)
        preflight.assert_not_called()
        self.assertEqual(out.getvalue().strip(),
                         "cupertino-install: failed: arguments: --answers needs --disk DISK and --yes-erase DISK, naming the same disk")


if __name__ == "__main__":
    unittest.main()
