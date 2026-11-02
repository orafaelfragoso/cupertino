"""The live image starts the services the desktop and the installed system use: NetworkManager, BlueZ, cupertino-powerd,
resolved and timesyncd, and not systemd-networkd or iwd (they would fight NetworkManager for the interfaces)."""
import re
import unittest
from pathlib import Path

PROFILE = Path(__file__).resolve().parent
LINKS = dict(line.split(" ", 1) for line in (PROFILE / "airootfs-links").read_text().splitlines() if line)
PACKAGES = set((PROFILE / "packages.x86_64").read_text().split())
UNIT_DIR = "/usr/lib/systemd/system/"


def enabled(unit: str) -> bool:
    return any(link.rsplit("/", 2)[-2].endswith(".wants") and link.endswith("/" + unit) and target == UNIT_DIR + unit
               for link, target in LINKS.items())


class LiveServicesTest(unittest.TestCase):
    def test_the_services_of_the_installed_system_are_enabled(self):
        for unit in ("NetworkManager.service", "bluetooth.service", "cupertino-powerd.service", "systemd-resolved.service",
                     "systemd-timesyncd.service"):
            self.assertTrue(enabled(unit), unit)

    def test_networkmanager_and_bluez_are_installed_and_aliased_for_dbus(self):
        self.assertLessEqual({"networkmanager", "bluez", "bluez-utils"}, PACKAGES)
        self.assertEqual(LINKS["etc/systemd/system/dbus-org.freedesktop.NetworkManager.service"], UNIT_DIR + "NetworkManager.service")
        self.assertEqual(LINKS["etc/systemd/system/dbus-org.bluez.service"], UNIT_DIR + "bluetooth.service")
        self.assertEqual(LINKS["etc/systemd/system/bluetooth.target.wants/bluetooth.service"], UNIT_DIR + "bluetooth.service")

    def test_nothing_else_manages_the_interfaces(self):
        for unit in ("systemd-networkd.service", "systemd-networkd.socket", "iwd.service"):
            self.assertFalse(any(link.endswith("/" + unit) for link in LINKS), unit)
        self.assertFalse((PROFILE / "airootfs/etc/systemd/network").exists())
        self.assertFalse((PROFILE / "airootfs/etc/systemd/networkd.conf.d").exists())

    def test_every_link_names_a_unit_or_path_in_the_image(self):
        for link, target in LINKS.items():
            self.assertTrue(re.fullmatch(r"(/usr/lib/systemd/system/[\w@.-]+|\.\./[\w.-]+|/usr/share/zoneinfo/UTC|/run/systemd/resolve/stub-resolv\.conf|/dev/null)", target), link)


if __name__ == "__main__":
    unittest.main()
