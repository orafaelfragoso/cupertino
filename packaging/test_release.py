"""Tests of the application files the runtime package installs: `python3 -I packaging/test_release.py`."""
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))  # isolated mode (-I) does not add the script's directory
import release  # noqa: E402

APPS = ('cupertino-files', 'cupertino-settings')


def check(files):
    """release.applications() over a tree holding the given crates/*/data/applications files."""
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        for rel in files:
            (root / rel).parent.mkdir(parents=True, exist_ok=True)
            (root / rel).write_text('[Desktop Entry]\n')
        return [f.relative_to(root).as_posix() for f in release.applications(root, APPS)]


SHIPPED = ['crates/cupertino-files/data/applications/cupertino-files.desktop',
           'crates/cupertino-files/data/applications/cupertino-mimeapps.list',
           'crates/cupertino-settings/data/applications/cupertino-settings.desktop']


class ApplicationsTest(unittest.TestCase):
    def test_the_apps_entries_and_the_mimeapps_list_are_installed(self):
        self.assertEqual(check(SHIPPED), SHIPPED)

    def test_a_shipped_app_without_its_entry_fails(self):
        with self.assertRaisesRegex(ValueError, 'cupertino-settings.desktop: missing for the shipped app'):
            check(SHIPPED[:2])

    def test_a_stray_file_fails(self):
        for stray in ['crates/cupertino-files/data/applications/cupertino-files.desktop~',
                      'crates/cupertino-files/data/applications/README.md',
                      'crates/cupertino-dock/data/applications/cupertino-dock.desktop']:
            with self.subTest(stray), self.assertRaisesRegex(ValueError, 'not a desktop entry of a shipped app'):
                check(SHIPPED + [stray])

    def test_one_file_name_from_two_crates_fails(self):
        with self.assertRaisesRegex(ValueError, 'cupertino-mimeapps.list also comes from crates/cupertino-files'):
            check(SHIPPED + ['crates/cupertino-settings/data/applications/cupertino-mimeapps.list'])


if __name__ == '__main__':
    unittest.main()
