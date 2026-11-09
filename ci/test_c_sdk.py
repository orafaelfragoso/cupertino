#!/usr/bin/env python3
"""The helpers of ci/c-sdk.py: the compile flags it accepts, the Rust manifest it rewrites and the environment it builds
in. The script itself runs in the c-sdk CI job."""
import importlib.util
from pathlib import Path
import tempfile
import unittest

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location('c_sdk', HERE / 'c-sdk.py')
c_sdk = importlib.util.module_from_spec(spec)
spec.loader.exec_module(c_sdk)

MANIFEST = '''[package]
name = "demo"

[dependencies]
cupertino-kit = { git = "https://example.invalid/cupertino", tag = "v0.4.0" }

[patch.crates-io]
xkbcommon = { git = "https://example.invalid/cupertino", tag = "v0.4.0" }
'''


class FlagTest(unittest.TestCase):
    def test_flags_inside_the_root_pass(self):
        root = Path('/tmp/stage/root')
        self.assertEqual(c_sdk.flag_problems(['-I/tmp/stage/root/usr/include/cupertinokit-1', '-L/tmp/stage/root/usr/lib',
                                              '-lcupertinokit', '-pthread'], root, Path('/src/cupertino')), [])

    def test_a_directory_outside_the_root_or_in_the_checkout_is_reported(self):
        root = Path('/tmp/stage/root')
        problems = c_sdk.flag_problems(['-I/usr/include/cupertinokit-1', '-L/src/cupertino/target/debug',
                                        '-isystem/opt/include', '-Wl,-rpath,/src/cupertino/target'], root,
                                       Path('/src/cupertino'))
        self.assertEqual(problems, [
            '-I/usr/include/cupertinokit-1: not a directory of the SDK root /tmp/stage/root',
            '-L/src/cupertino/target/debug: names the checkout /src/cupertino',
            '-L/src/cupertino/target/debug: not a directory of the SDK root /tmp/stage/root',
            '-Wl,-rpath,/src/cupertino/target: names the checkout /src/cupertino',
            '-isystem/opt/include: not a directory of the SDK root /tmp/stage/root',
        ])


class ManifestTest(unittest.TestCase):
    def test_the_release_tag_becomes_this_checkouts_crates(self):
        text = c_sdk.rust_manifest(MANIFEST, '0.4.0', Path('/src/cupertino'))
        self.assertIn('\ncupertino-kit = { path = "/src/cupertino/crates/cupertino-kit" }\n', text)
        self.assertIn('\nxkbcommon = { path = "/src/cupertino/crates/cupertino-xkbcommon" }\n', text)
        self.assertNotIn('git =', text)

    def test_another_release_or_no_tag_fails(self):
        with self.assertRaisesRegex(c_sdk.Failed, 'cupertino-kit from v0.4.0, this release is v0.5.0'):
            c_sdk.rust_manifest(MANIFEST, '0.5.0')
        with self.assertRaisesRegex(c_sdk.Failed, 'does not take xkbcommon from a release tag'):
            c_sdk.rust_manifest(MANIFEST.split('[patch')[0], '0.4.0')


class EnvironmentTest(unittest.TestCase):
    def test_only_the_root_is_searched(self):
        root = Path('/tmp/stage/root')
        env = c_sdk.sdk_environment(root, {'PATH': '/usr/bin', 'PKG_CONFIG_PATH': '/usr/local/lib/pkgconfig',
                                           'CPATH': '/usr/local/include', 'LIBRARY_PATH': '/usr/local/lib'})
        self.assertEqual(env, {'PATH': '/usr/bin', 'PKG_CONFIG_SYSROOT_DIR': '/tmp/stage/root',
                               'PKG_CONFIG_LIBDIR': '/tmp/stage/root/usr/lib/pkgconfig',
                               'LD_LIBRARY_PATH': '/tmp/stage/root/usr/lib', 'LC_ALL': 'C'})


class VersionTest(unittest.TestCase):
    def test_the_header_version_and_the_command_version_are_read(self):
        self.assertEqual(c_sdk.abi_version(), (1, 0))
        self.assertRegex(c_sdk.cli_version(), r'^\d+\.\d+\.\d+')
        with tempfile.TemporaryDirectory() as directory:
            header = Path(directory) / 'h.h'
            header.write_text('#define CUPERTINOKIT_ABI_MAJOR 1\n')
            with self.assertRaises(c_sdk.Failed):
                c_sdk.abi_version(header)

    def test_a_used_keep_directory_is_refused(self):
        with tempfile.TemporaryDirectory() as directory:
            (Path(directory) / 'old').write_text('')
            bindir = Path(directory) / 'bin'
            bindir.mkdir()
            for name in ('libcupertinokit.so', 'cupertino'):
                (bindir / name).write_text('')
            self.assertEqual(c_sdk.main(['--no-build', '--bindir', str(bindir), '--keep', directory]), 1)


if __name__ == '__main__':
    unittest.main()
