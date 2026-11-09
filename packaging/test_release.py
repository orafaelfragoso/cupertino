"""Tests of the application files and the release tags of the runtime package: `python3 -I packaging/test_release.py`."""
import configparser
import json
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
    def test_original_pack_and_every_manifest_payload_are_installed(self):
        root = release.ROOT / 'data/assets/cupertino'
        manifest = json.loads((root / 'manifest.json').read_text())
        payloads = {'manifest.json', 'CREDITS.md', 'LICENSE'}
        for entry in manifest['icons'].values():
            payloads.update(entry.get('raster', {}).values())
            payloads.update(entry[k] for k in ('svg', 'symbol') if k in entry)
        installed = {p for p, _src, _mode, _conf in release.data()}
        self.assertEqual({f'usr/share/cupertino/assets/cupertino/{p}' for p in payloads} - installed, set())

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


class PortalsConfTest(unittest.TestCase):
    CONF = 'usr/share/xdg-desktop-portal/cupertino-portals.conf'

    def backends(self):
        conf = configparser.ConfigParser()
        conf.read(release.ROOT / 'packaging/files' / self.CONF)
        return [b for b in conf['preferred']['default'].split(';') if b]

    def test_the_conf_is_in_the_runtime_package(self):
        self.assertIn(self.CONF, [rel for rel, _src, _mode, _conf in release.runtime()])

    def test_every_backend_it_prefers_is_installed_with_the_package(self):
        depends = re.findall(r"'([\w-]+)'", re.search(r'depends=\((.*?)\)', (
            release.ROOT / 'packaging/arch/cupertino/PKGBUILD').read_text().split('package_cupertino()')[1], re.S)[1])
        backends = self.backends()
        self.assertEqual(backends, ['gtk'])
        for backend in backends:
            with self.subTest(backend=backend):
                self.assertIn(f'xdg-desktop-portal-{backend}', depends)


class SecretServiceTest(unittest.TestCase):
    """gnome-keyring is the session's Secret Service, unlocked through PAM (cupertino-sessiond's README, "Secret Service")."""
    PAM = release.ROOT / 'packaging/pam'
    AUTH = '-auth optional pam_gnome_keyring.so'
    SESSION = '-session optional pam_gnome_keyring.so auto_start'

    def lines(self, service):
        return [line for line in (self.PAM / 'arch' / service).read_text().splitlines() if line and not line.startswith('#')]

    # The stack that checks the password: the login's and the lock screen's.
    STACK = 'auth include system-local-login'
    LOCK_STACK = 'auth include system-auth'

    def test_a_login_with_a_password_unlocks_the_keyring_and_its_session_starts_the_daemon(self):
        lines = self.lines('cupertino-session')
        # right after the stack that checks the password: the module only keeps the password it was given
        self.assertEqual(lines[lines.index(self.AUTH) - 1], self.STACK)
        # last: pam_systemd has made the runtime directory, where the daemon's control socket is
        self.assertEqual(lines[-1], self.SESSION)
        self.assertEqual(lines.count(self.AUTH) + lines.count(self.SESSION), 2)

    def test_auto_login_starts_the_daemon_locked_and_the_lock_screen_unlocks_it(self):
        autologin = self.lines('cupertino-autologin')
        self.assertEqual(autologin[-1], self.SESSION)
        self.assertNotIn(self.AUTH, autologin)
        lock = self.lines('cupertino-login')
        self.assertEqual(lock[:2], [self.LOCK_STACK, self.AUTH], 'right after the password stack')
        self.assertFalse([line for line in lock if line.startswith(('session', '-session'))])
        self.assertFalse([line for line in self.lines('cupertino-greeter') if 'keyring' in line])

    # pam_faillock(8) resets the count of failures in the account phase, so the lock screen's `pam_acct_mgmt` must reach it.
    LOCK_ACCOUNT = 'account include system-auth'

    def test_the_lock_screen_counts_towards_the_account_lockout(self):
        """The lock screen authenticates through the shared password stack and account stack, system-auth, which holds
        pam_faillock (preauth, authfail, authsucc and the account reset) by default. Nothing before the shared stack
        decides, and nothing after it can let a password through on its own, so every failed attempt at the lock screen
        goes through the same count as a failed login."""
        service = (release.ROOT / 'crates/cupertino-login/src/auth.rs').read_text()
        self.assertIn('pub const SERVICE: &str = "cupertino-login";', service)
        self.assertIn('cupertino-login', release.PAM_SERVICES)
        lock = self.lines('cupertino-login')
        auth = [line for line in lock if line.split()[0] in ('auth', '-auth') or line == self.LOCK_STACK]
        self.assertEqual(auth[0], self.LOCK_STACK, 'the shared stack comes first')
        for line in auth[1:]:
            self.assertEqual(line.split()[1], 'optional', f'{line!r} cannot decide')
        self.assertFalse([line for line in lock if re.search(r'pam_(permit|succeed_if|faillock)\.so', line)],
                         'the shared stack alone passes or counts')
        self.assertIn(self.LOCK_ACCOUNT, lock)
        # the lock screen runs as the user and pam_faillock records a failure only in a tally file that exists:
        # a session that started without a password gets one from a module run as root, at no cost to its login
        autologin = self.lines('cupertino-autologin')
        self.assertEqual([line for line in autologin if line.split()[0] in ('auth', '-auth')],
                         ['auth required pam_permit.so', '-auth optional pam_faillock.so authsucc'])

    def test_the_package_installs_the_provider_its_pam_module_and_a_client(self):
        for recipe in ('packaging/arch/cupertino/PKGBUILD', 'packaging/arch/cupertino-git/PKGBUILD'):
            with self.subTest(recipe):
                depends = re.search(r'^\s+depends=\((.*?)\)', (release.ROOT / recipe).read_text(), re.M | re.S).group(1)
                self.assertIn("'gnome-keyring'", depends)
                self.assertIn("'libsecret'", depends)
        live = (release.ROOT / 'packaging/iso/packages.x86_64').read_text().split()
        self.assertIn('gnome-keyring', live)
        self.assertIn('libsecret', live)

    def test_sandboxed_apps_reach_the_keyring_through_the_secret_portal(self):
        conf = configparser.ConfigParser()
        conf.read(release.ROOT / 'packaging/files' / PortalsConfTest.CONF)
        self.assertEqual(conf['preferred']['org.freedesktop.impl.portal.Secret'], 'gnome-keyring')


class RuntimeFloorTest(unittest.TestCase):
    """dbus 1.16 passes the caller's ProcessFD (dbus-broker does from 34); PipeWire 1.4 follows a screencast resize."""

    def test_the_package_requires_the_floors(self):
        for recipe in ('packaging/arch/cupertino/PKGBUILD', 'packaging/arch/cupertino-git/PKGBUILD'):
            with self.subTest(recipe):
                depends = re.search(r'^\s+depends=\((.*?)\)', (release.ROOT / recipe).read_text(), re.M | re.S).group(1)
                self.assertIn("'dbus>=1.16'", depends)
                self.assertIn("'pipewire>=1:1.4'", depends)


class ReleaseKeyRootTest(unittest.TestCase):
    """The ISO carries the key every install trusts, so the release signs it and the build takes only the checked-in key."""
    KEYRING = release.ROOT / 'packaging/keyring'

    @staticmethod
    def gpg(home, *args):
        return subprocess.run(['gpg', '--batch', '--quiet', '--pinentry-mode', 'loopback', '--passphrase', '', *args],
                              env={'GNUPGHOME': str(home), 'PATH': '/usr/bin:/bin'}, capture_output=True, text=True, check=True).stdout

    def make_key(self, home, name):
        self.gpg(home, '--quick-gen-key', f'{name} <{name}@example.invalid>', 'ed25519', 'cert', '1d')
        listing = self.gpg(home, '--with-colons', '--list-keys', name)
        fpr = next(line.split(':')[9] for line in listing.splitlines() if line.startswith('fpr'))
        self.gpg(home, '--quick-add-key', fpr, 'ed25519', 'sign', '1d')
        return fpr

    def check(self, key, fpr_file):
        return subprocess.run([self.KEYRING / 'check-key', key, fpr_file], capture_output=True, text=True)

    @unittest.skipUnless(shutil.which('gpg'), 'needs gpg')
    def test_only_the_key_with_the_checked_in_fingerprint_is_accepted(self):
        with tempfile.TemporaryDirectory() as tmp:
            home = Path(tmp, 'home')
            home.mkdir(mode=0o700)
            release_fpr = self.make_key(home, 'release')
            other_fpr = self.make_key(home, 'other')
            files = {}
            for name, args in {
                'release': ['--armor', '--export', release_fpr],
                'both': ['--armor', '--export', release_fpr, other_fpr],
                'other': ['--armor', '--export', other_fpr],
                'secret': ['--armor', '--export-secret-keys', release_fpr],
            }.items():
                files[name] = Path(tmp, f'{name}.asc')
                files[name].write_text(self.gpg(home, *args))
            fpr_file = Path(tmp, 'fpr')
            fpr_file.write_text(release_fpr + '\n')
            self.assertEqual(self.check(files['release'], fpr_file).returncode, 0)
            for name, reason in {'other': 'not the release key', 'both': '2 primary keys', 'secret': 'secret key'}.items():
                with self.subTest(name):
                    result = self.check(files[name], fpr_file)
                    self.assertEqual(result.returncode, 1)
                    self.assertIn(reason, result.stderr)

    def test_the_build_checks_the_embedded_key_against_the_tree(self):
        fingerprint = (self.KEYRING / 'release-key.fpr').read_text()
        self.assertRegex(fingerprint, r'^[0-9A-F]{40}\n$')
        self.assertIn('"$source_root/packaging/keyring/check-key" "$keyring_key" "$source_root/packaging/keyring/release-key.fpr"',
                      (release.ROOT / 'scripts/build-iso.sh').read_text())

    def job_needs(self, job):
        text = (release.ROOT / '.github/workflows/release.yml').read_text()
        block = re.search(rf'^  {job}:\n(.*?)(?=^  \w[\w-]*:\n|\Z)', text, re.M | re.S).group(1)
        return block, re.search(r'needs: \[(.*?)\]', block).group(1).split(', ')

    def test_the_sign_job_waits_for_the_iso_and_signs_its_checksum(self):
        block, needs = self.job_needs('sign')
        self.assertIn('iso', needs)
        self.assertIn("needs.iso.result == 'skipped'", block)
        self.assertIn('name: iso', block)
        self.assertIn('signed-iso', block)
        publish, _ = self.job_needs('publish')
        self.assertIn('name: signed-iso', publish)


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
                release.cmd_pkgbuild(Namespace(tag='v0.1.0-rc1', commit=None, archive=archive, output=Path(tmp, 'out')))


class PackageReuseTest(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        self.packages = self.root / 'packages'
        self.packages.mkdir()
        self.output = self.root / 'iso-input'
        self.runtime = self.packages / 'cupertino-0.1.0-1-x86_64.pkg.tar.zst'
        self.data = self.packages / 'cupertino-data-0.1.0-1-any.pkg.tar.zst'
        self.runtime.write_bytes(b'verified runtime')
        self.data.write_bytes(b'verified data')
        patch = mock.patch.object(release, 'git', return_value='a' * 40)
        patch.start()
        self.addCleanup(patch.stop)
        self.args = Namespace(directory=self.packages, tag='v0.1.0', commit=None, copy_to=self.output)
        release.cmd_record_packages(self.args)

    def test_iso_receives_exact_verified_package_bytes_and_provenance(self):
        release.cmd_verify_packages(self.args)
        self.assertEqual({p.name for p in self.output.iterdir()},
                         {self.runtime.name, self.data.name, 'packages.json'})
        self.assertEqual((self.output / self.runtime.name).read_bytes(), self.runtime.read_bytes())

    def test_another_commit_or_release_is_rejected_before_copying(self):
        with mock.patch.object(release, 'git', return_value='b' * 40), \
                self.assertRaisesRegex(SystemExit, 'not from this commit'):
            release.cmd_verify_packages(self.args)
        self.args.tag = 'v0.1.0-rc1'
        with self.assertRaisesRegex(SystemExit, 'not from this commit and release version'):
            release.cmd_verify_packages(self.args)
        self.assertFalse(self.output.exists())

    def test_changed_bytes_and_extra_packages_are_rejected_before_copying(self):
        self.runtime.write_bytes(b'tampered')
        with self.assertRaisesRegex(SystemExit, 'SHA256 digests differ'):
            release.cmd_verify_packages(self.args)
        release.cmd_record_packages(self.args)
        (self.packages / 'unrelated.pkg.tar.zst').write_bytes(b'extra')
        with self.assertRaisesRegex(SystemExit, 'expected one runtime and one data'):
            release.cmd_verify_packages(self.args)
        self.assertFalse(self.output.exists())

    def test_links_and_path_traversal_in_provenance_are_rejected(self):
        self.runtime.unlink()
        self.runtime.symlink_to(self.data)
        with self.assertRaisesRegex(SystemExit, 'regular files'):
            release.cmd_verify_packages(self.args)
        self.runtime.unlink()
        self.runtime.write_bytes(b'verified runtime')
        record = self.packages / 'packages.json'
        metadata = json.loads(record.read_text())
        metadata['files']['../outside.pkg.tar.zst'] = metadata['files'].pop(self.runtime.name)
        record.write_text(json.dumps(metadata))
        with self.assertRaisesRegex(SystemExit, 'package names or SHA256 digests differ'):
            release.cmd_verify_packages(self.args)


class SourceSnapshotTest(unittest.TestCase):
    def test_source_recipe_and_package_provenance_keep_the_captured_commit(self):
        with tempfile.TemporaryDirectory() as directory, mock.patch.object(release, 'ROOT', Path(directory)):
            root = Path(directory)
            subprocess.run(['git', 'init', '-q', root], check=True)
            manifest = root / 'crates/cupertino-server/Cargo.toml'
            recipe = root / 'packaging/arch/cupertino/PKGBUILD'
            manifest.parent.mkdir(parents=True)
            recipe.parent.mkdir(parents=True)
            manifest.write_text('[package]\nversion="0.1.0"\n')
            recipe.write_text("pkgver=0.1.0\n_tag=v$pkgver\nsha256sums=('UNRELEASED')\n")
            subprocess.run(['git', '-C', root, 'add', '.'], check=True)
            commit = ['git', '-C', str(root), '-c', 'user.name=Test', '-c', 'user.email=test@example.invalid',
                      'commit', '-qm']
            subprocess.run([*commit, 'captured source'], check=True)
            captured = release.git('rev-parse', 'HEAD')
            manifest.write_text('[package]\nversion="0.2.0"\n')
            recipe.write_text('changed recipe\n')
            subprocess.run(['git', '-C', root, 'add', '.'], check=True)
            subprocess.run([*commit, 'new source'], check=True)
            out = root / 'out'
            release.cmd_source(Namespace(commit=captured, tag=None, output=out))
            archive = out / 'cupertino-0.1.0-source.tar.xz'
            self.assertTrue(archive.is_file())
            release.cmd_pkgbuild(Namespace(commit=captured, tag='v0.1.0-rc1', archive=archive, output=out / 'PKGBUILD'))
            self.assertIn('pkgver=0.1.0rc1\n', (out / 'PKGBUILD').read_text())
            for name in ('cupertino-0.1.0-1-x86_64.pkg.tar.zst', 'cupertino-data-0.1.0-1-any.pkg.tar.zst'):
                (out / name).write_bytes(b'from captured source')
            args = Namespace(commit=captured, tag=None, directory=out, copy_to=root / 'iso')
            release.cmd_record_packages(args)
            self.assertEqual(json.loads((out / 'packages.json').read_text())['commit'], captured)
            release.cmd_verify_packages(args)
            args.commit = None
            with self.assertRaisesRegex(SystemExit, 'not from this commit'):
                release.cmd_verify_packages(args)


GIT_RECIPE = Path(__file__).resolve().parent / 'arch/cupertino-git/PKGBUILD'


class TestPamShimTest(unittest.TestCase):
    """The tests' PAM shim (`cupertino-test-pam-shim`, which replaces `pam_start`, `connect` and `syslog` in a process that
    loads it) and its helper never reach a package: the packages build only the binaries, neither crate is in their
    dependencies, and no manifest installs either."""

    TEST_CRATES = ('cupertino-test-pam-shim', 'cupertino-test-pam')

    def test_no_package_installs_the_shim_or_its_helper(self):
        for package, manifest in release.MANIFESTS.items():
            for destination, source, _mode, _conf in manifest():
                for name in (destination, source or ''):
                    self.assertNotIn('test_pam', name, package)
                    self.assertNotIn('test-pam', name, package)

    def test_the_shipped_binaries_do_not_link_the_shim_or_its_helper(self):
        linked = {p['name'] for p in release.linked_packages()}
        self.assertIn('cupertino-login', linked)
        for crate in self.TEST_CRATES:
            self.assertNotIn(crate, linked)
            self.assertNotIn(crate, release.BINARIES)

    def test_the_packages_build_only_the_binaries(self):
        for recipe in (release.ROOT / 'packaging/arch').glob('*/PKGBUILD'):
            builds = [line for line in recipe.read_text().splitlines() if 'cargo build' in line]
            self.assertTrue(builds, recipe)
            for line in builds:
                self.assertIn('--bins', line, recipe)
                self.assertNotIn('--all-targets', line, recipe)
                self.assertNotIn('--lib', line, recipe)

    def test_the_shim_is_a_test_only_library(self):
        manifest = (release.ROOT / 'crates/cupertino-test-pam-shim/Cargo.toml').read_text()
        self.assertIn('crate-type = ["cdylib"]', manifest)
        self.assertIn('publish = false', manifest)
        self.assertNotIn('[[bin]]', manifest)


class LicenceTest(unittest.TestCase):
    def test_a_list_missing_a_shipped_licence_is_reported(self):
        shipped = {'GPL-3.0-or-later', 'MPL-2.0'}
        declared = {'PKGBUILD': {'GPL-3.0-or-later'}, 'cupertino-git PKGBUILD': {'GPL-3.0-or-later', 'MPL-2.0', 'OFL-1.1'}}
        self.assertEqual(release.missing_licenses(shipped, declared), ['PKGBUILD lacks MPL-2.0'])

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
