"""Tests of the application files and the release tags of the runtime package: `python3 -I packaging/test_release.py`."""
import re
import shutil
import subprocess
import sys
import tempfile
import unittest
from argparse import Namespace
from pathlib import Path
from unittest import mock

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


class TagTest(unittest.TestCase):
    def test_a_release_and_a_release_candidate_give_the_arch_pkgver(self):
        self.assertEqual(release.release_version('v0.1.0'), '0.1.0')
        self.assertEqual(release.release_version('v0.1.0-rc1'), '0.1.0rc1')

    def test_a_tag_for_another_version_or_kind_is_rejected(self):
        with mock.patch.object(release, 'version', return_value='0.1.0'):
            for tag in ['v0.1.1-rc1', 'v0.1.0-beta1']:
                with self.subTest(tag), self.assertRaisesRegex(SystemExit, f'tag {tag} does not match'):
                    release.cmd_verify_tag(Namespace(tag=tag))


    def test_a_missing_tag_is_reported(self):
        with mock.patch.object(release, 'version', return_value='0.1.0'), \
                self.assertRaisesRegex(SystemExit, 'tag v0.1.0-rc99 does not exist'):
            release.cmd_verify_tag(Namespace(tag='v0.1.0-rc99'))

    def test_the_pkgbuild_needs_exactly_one_version_line(self):
        with tempfile.TemporaryDirectory() as tmp, mock.patch.object(release, 'ROOT', Path(tmp)), \
                mock.patch.object(release, 'version', return_value='0.1.0'):
            recipe = Path(tmp, 'packaging/arch/cupertino/PKGBUILD')
            recipe.parent.mkdir(parents=True)
            recipe.write_text("pkgver=0.1.0\nsha256sums=('UNRELEASED')\n")
            archive = Path(tmp, 'a.tar.xz')
            archive.write_bytes(b'x')
            with self.assertRaisesRegex(SystemExit, r'exactly one line'):
                release.cmd_pkgbuild(Namespace(tag='v0.1.0-rc1', archive=archive, output=Path(tmp, 'out')))


GIT_RECIPE = Path(__file__).resolve().parent / 'arch/cupertino-git/PKGBUILD'


class LicenceTest(unittest.TestCase):
    def test_a_list_missing_a_shipped_licence_is_reported(self):
        shipped = {'GPL-3.0-or-later', 'MPL-2.0'}
        declared = {'PKGBUILD': {'GPL-3.0-or-later'}, 'rpm': {'GPL-3.0-or-later', 'MPL-2.0', 'OFL-1.1'}}
        self.assertEqual(release.missing_licenses(shipped, declared), ['PKGBUILD lacks MPL-2.0'])

    def test_a_crate_the_copyright_file_gives_another_licence_is_reported(self):
        text = ('Files: *\nCopyright: x\nLicense: GPL-3+\n\n'
                'Files: crates/a/*\n crates/b/*\nCopyright: x\nLicense: MPL-2.0\n\n'
                'Files: crates/*/protocols/*.xml\nCopyright: x\nLicense: MIT or Apache-2.0\n\n'
                'Files: crates/b/src/*\nCopyright: x\nLicense: GPL-3+\n')
        crates = {'a': 'MPL-2.0', 'b': 'MPL-2.0', 'c': 'MPL-2.0', 'd': 'GPL-3.0-or-later', 'e': 'MPL-2.0 AND OFL-1.1'}
        paths = ['crates/a/Cargo.toml', 'crates/a/protocols/x.xml', 'crates/b/Cargo.toml', 'crates/b/src/lib.rs',
                 'crates/c/Cargo.toml', 'crates/d/src/main.rs', 'crates/e/Cargo.toml', 'README.md']
        self.assertEqual(release.crate_license_problems(text, crates, paths),
                         ['crates/b/src/lib.rs is MPL-2.0 as its crate, the Debian copyright file gives it GPL-3.0-or-later',
                          'crates/c/Cargo.toml is MPL-2.0 as its crate, the Debian copyright file gives it GPL-3.0-or-later'])

    def test_every_workspace_crate_file_has_its_licence_in_the_copyright_file(self):
        text = (release.ROOT / 'packaging/debian/cupertino.copyright').read_text()
        problems = release.crate_license_problems(text, release.workspace_crate_licenses(), release.tracked_crate_files())
        self.assertEqual(problems, [])

    def test_the_runtime_ships_the_licences_of_its_crates_and_embedded_notices(self):
        self.assertEqual(release.shipped_licenses('runtime'), {'GPL-3.0-or-later', 'MPL-2.0', 'MIT', 'OFL-1.1', 'LGPL-2.1-or-later', 'MIT-CMU', 'BSD-3-Clause'})

    def test_notice_files_are_found_by_name_only(self):
        with tempfile.TemporaryDirectory() as tmp:
            for name in ('NOTICE', 'notice.md', 'NOTICES.txt', 'LICENSE', 'NOTICE-extra'):
                (Path(tmp) / name).write_text('x')
            found = release.notice_files({'manifest_path': f'{tmp}/Cargo.toml'})
        self.assertEqual([f.name for f in found], ['NOTICE', 'NOTICES.txt', 'notice.md'])

    def test_the_khronos_licence_comments_are_extracted(self):
        crate = next(p for p in release.linked_packages() if p['name'] == 'khronos_api')
        found = dict(release.embedded_notices(crate))
        self.assertIn('Copyright (c) 2013-2017 The Khronos Group Inc.', found['api_egl/api/egl.xml'])
        self.assertIn('Licensed under the Apache License, Version 2.0', found['api/xml/gl.xml'])
        text = (release.ROOT / release.THIRD_PARTY).read_text()
        self.assertIn('The Khronos Group Inc.', text)

    def test_the_protocol_files_the_scanner_compiles_carry_their_notices(self):
        text = (release.ROOT / release.THIRD_PARTY).read_text()
        linked = {p['name']: p for p in release.linked_packages()}
        self.assertEqual(release.embedded_notice_files(linked['wayland-server']), ['wayland.xml'])
        self.assertIn('Kristian', dict(release.embedded_notices(linked['wayland-server']))['wayland.xml'])
        for crate in release.SCANNER_CRATES:
            notices = release.embedded_notices(linked[crate])
            self.assertTrue(notices, crate)
            self.assertTrue(all(body in text for _name, body in notices), crate)

    def test_every_licence_an_embedded_notice_names_has_its_text_and_a_licence_field(self):
        named = release.notice_licenses()
        self.assertEqual(set(named), {'LGPL-2.1-or-later', 'MIT-CMU', 'BSD-3-Clause', 'MIT'})
        text = (release.ROOT / release.THIRD_PARTY).read_text()
        for ident in named:
            self.assertRegex(text, rf'(?m)^--- .*\({re.escape(ident)}\)$')
        self.assertLessEqual(set(named), release.shipped_licenses('runtime'))

    def test_a_licence_named_by_a_notice_without_its_text_is_reported(self):
        with tempfile.TemporaryDirectory() as tmp:
            stale = Path(tmp) / 'THIRD-PARTY-LICENSES'
            text = (release.ROOT / release.THIRD_PARTY).read_text()
            stale.write_text(re.sub(r'(?m)^(--- .*)\(MIT-CMU\)$', r'\1(removed)', text))
            with mock.patch.object(release, 'THIRD_PARTY', str(stale)):
                problems = release.third_party_problems()
        self.assertEqual(problems, [f'{stale} has no text of MIT-CMU, which an embedded notice names'])

    def test_an_embedded_notice_file_that_is_missing_is_an_error(self):
        with tempfile.TemporaryDirectory() as tmp:
            with self.assertRaisesRegex(ValueError, 'api_egl/api/egl.xml is missing'):
                release.embedded_notices({'name': 'khronos_api', 'version': '0', 'manifest_path': f'{tmp}/Cargo.toml'})

    def test_a_missing_or_stale_third_party_file_is_reported(self):
        self.assertEqual(release.third_party_problems(), [])
        with tempfile.TemporaryDirectory() as tmp:
            stale = Path(tmp) / 'THIRD-PARTY-LICENSES'
            with mock.patch.object(release, 'THIRD_PARTY', str(stale)):
                self.assertRegex(release.third_party_problems()[0], 'is missing')
                text = (release.ROOT / 'packaging/licenses/THIRD-PARTY-LICENSES').read_text()
                first = re.search(r'^CRATES \(\d+\)\n=+\n(\S+) (\S+)\n', text, re.M)
                name, version = first[1], first[2]
                stale.write_text(text.replace(f'\n{name} {version}\n', f'\n{name} 0.0.0-stale\n', 1))
                problems = release.third_party_problems()
        self.assertEqual(len(problems), 2)
        self.assertRegex(problems[0], f'lacks {name} {version}')
        self.assertRegex(problems[1], f'lists {name} 0.0.0-stale, which is not linked')

    def test_every_package_lists_what_it_ships(self):
        for package in release.MANIFESTS:
            self.assertEqual(
                release.missing_licenses(release.shipped_licenses(package), release.declared_licenses(package)), [])


def git_pkgver(describe):
    """What pkgver() of the -git recipe makes of a `git describe --long` line."""
    script = re.search(r"git describe .*\| (sed .+)$", GIT_RECIPE.read_text(), re.M)[1]
    return subprocess.run(script, shell=True, input=describe, capture_output=True, text=True, check=True).stdout.strip()


class GitVersionTest(unittest.TestCase):
    def test_a_release_candidate_build_is_named_below_the_release(self):
        self.assertEqual(git_pkgver('v0.1.0-rc1-5-gabc1234'), '0.1.0rc1.r5.gabc1234')
        self.assertEqual(git_pkgver('v0.1.0-3-gabc1234'), '0.1.0.r3.gabc1234')

    @unittest.skipUnless(shutil.which('vercmp'), 'vercmp (pacman) is not installed')
    def test_pacman_orders_candidate_before_release_before_later_commits(self):
        order = ['0.1.0rc1', '0.1.0rc1.r3.gabc1234', '0.1.0', '0.1.0.r3.gabc1234']
        for older, newer in zip(order, order[1:]):
            self.assertEqual(subprocess.run(['vercmp', older, newer], capture_output=True, text=True).stdout.strip(), '-1')


if __name__ == '__main__':
    unittest.main()
