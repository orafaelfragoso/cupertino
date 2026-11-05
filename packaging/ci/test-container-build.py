#!/usr/bin/env python3
"""Exercise package-builder orchestration without Docker, network access or a Rust build."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest


HERE = Path(__file__).resolve().parent
SCRIPT = HERE / 'build-in-container.sh'


class ContainerBuild(unittest.TestCase):
    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory()
        self.addCleanup(self.scratch.cleanup)
        self.root = Path(self.scratch.name)
        (self.root / 'packaging/ci').mkdir(parents=True)
        shutil.copyfile(HERE / 'Dockerfile', self.root / 'packaging/ci/Dockerfile')
        self.bin = self.root / 'bin'
        self.bin.mkdir()
        git = self.bin / 'git'
        git.write_text('#!' + sys.executable + '''
import io, os, sys, tarfile
if 'rev-parse' in sys.argv:
    print(os.environ['TEST_ROOT'])
else:
    with tarfile.open(fileobj=sys.stdout.buffer, mode='w|') as archive:
        directory = tarfile.TarInfo('src/')
        directory.type = tarfile.DIRTYPE
        directory.mode = 0o755
        archive.addfile(directory)
''')
        git.chmod(0o700)
        docker = self.bin / 'docker'
        docker.write_text('#!' + sys.executable + '''
import json, os, sys
from pathlib import Path
args = sys.argv[1:]
with open(os.environ['TEST_LOG'], 'a') as log:
    log.write(json.dumps(args) + '\\n')
if args[:2] == ['buildx', 'build']:
    for option in args:
        if option.startswith('type=local,dest='):
            directory = Path(option.removeprefix('type=local,dest=').removesuffix(',mode=max'))
            if not os.environ.get('TEST_INCOMPLETE_CACHE'):
                (directory / 'index.json').write_text('{}')
    if os.environ.get('TEST_BUILD_FAIL'):
        raise SystemExit(17)
else:
    # Build containers have /src mounts; fresh install containers carry only /dist.
    install = not any(':/src' in argument for argument in args)
    if install and os.environ.get('TEST_INSTALL_FAIL'):
        raise SystemExit(19)
''')
        docker.chmod(0o700)
        self.log = self.root / 'docker.jsonl'
        self.cache = self.root / 'cache'
        self.env = dict(os.environ, PATH=str(self.bin) + os.pathsep + os.environ['PATH'],
                        TEST_ROOT=str(self.root), TEST_LOG=str(self.log),
                        CUPERTINO_BUILD_CACHE_DIR=str(self.cache), CUPERTINO_BUILD_LOCK_FD='test-lease')

    def run_build(self, mode, overrides):
        env = dict(self.env, **overrides)
        return subprocess.run(['bash', str(SCRIPT), 'debian', str(self.root / 'dist'), *mode],
                              env=env, text=True, capture_output=True)

    def commands(self):
        return [json.loads(line) for line in self.log.read_text().splitlines()]

    def test_caches_image_and_rotates_export_after_success(self):
        first = self.run_build([], {})
        self.assertEqual(first.returncode, 0, first.stderr)
        current = self.cache / 'debian/current'
        old = current.resolve()
        second = self.run_build([], {})
        self.assertEqual(second.returncode, 0, second.stderr)
        self.assertNotEqual(old, current.resolve())
        self.assertFalse(old.exists())
        commands = self.commands()
        self.assertIn('type=local,src=' + str(current), commands[3])
        build_run = commands[1]
        self.assertIn('type=bind,src=' + str(self.root / 'target/package-build/debian') + ',dst=/src/target', build_run)
        self.assertIn('export CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/src/target', build_run[-1])
        install_run = commands[2]
        self.assertIn('docker.io/library/debian@sha256:4c25275446ca371766b8b093c696782eabce06263fccacb8d5d813b3dc44b308', install_run)
        self.assertFalse(any('cupertino-package-builder:' in argument for argument in install_run))

    def test_failed_image_export_preserves_previous_cache(self):
        self.assertEqual(self.run_build([], {}).returncode, 0)
        current = self.cache / 'debian/current'
        old = current.resolve()
        result = self.run_build([], {'TEST_BUILD_FAIL': '1'})
        self.assertEqual(result.returncode, 17, result.stderr)
        self.assertEqual(current.resolve(), old)
        self.assertEqual(list((self.cache / 'debian').glob('generation.*')), [old])
        self.assertEqual(len(self.commands()), 4)

    def test_incomplete_export_is_not_published(self):
        self.assertEqual(self.run_build([], {}).returncode, 0)
        current = self.cache / 'debian/current'
        old = current.resolve()
        result = self.run_build([], {'TEST_INCOMPLETE_CACHE': '1'})
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('builder cache export is incomplete', result.stderr)
        self.assertEqual(current.resolve(), old)
        self.assertEqual(list((self.cache / 'debian').glob('generation.*')), [old])

    def test_install_failure_propagates(self):
        result = self.run_build([], {'TEST_INSTALL_FAIL': '1'})
        self.assertEqual(result.returncode, 19, result.stderr)

    def test_test_only_needs_no_image_build_or_cargo_target(self):
        result = self.run_build(['--test-only'], {})
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(len(self.commands()), 1)
        self.assertFalse((self.root / 'target/package-build').exists())
        self.assertFalse(self.cache.exists())


if __name__ == '__main__':
    unittest.main()
