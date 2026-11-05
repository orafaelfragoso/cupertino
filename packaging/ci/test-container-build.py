#!/usr/bin/env python3
"""Exercise package-builder orchestration without Docker, network access or a Rust build."""
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import unittest


HERE = Path(__file__).resolve().parent
SCRIPT = HERE / 'build-in-container.sh'


class ContainerBuild(unittest.TestCase):
    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory()
        self.addCleanup(self.scratch.cleanup)
        self.root = Path(self.scratch.name)
        (self.root / 'head').write_text('a' * 40)
        (self.root / 'packaging/ci').mkdir(parents=True)
        shutil.copyfile(HERE / 'Dockerfile', self.root / 'packaging/ci/Dockerfile')
        release = self.root / 'packaging/release.py'
        release.write_text('''import json, os, sys
from pathlib import Path
args = sys.argv[1:]
with open(os.environ['TEST_LOG'], 'a') as log:
    log.write(json.dumps(['release', *args]) + '\\n')
if args[0] == 'source':
    path = Path(args[args.index('--output') + 1]) / 'source.tar.xz'
    path.write_text('source')
    print(path)
elif args[0] == 'pkgbuild':
    Path(args[-1]).write_text('pkgrel=1\\n')
elif args[0] != 'record-packages':
    raise AssertionError(args)
''')
        self.bin = self.root / 'bin'
        self.bin.mkdir()
        git = self.bin / 'git'
        git.write_text('#!' + sys.executable + '''
import io, json, os, sys, tarfile
from pathlib import Path
if '--show-toplevel' in sys.argv:
    print(os.environ['TEST_ROOT'])
elif 'rev-parse' in sys.argv:
    print(Path(os.environ['TEST_ROOT'], 'head').read_text())
else:
    with open(os.environ['TEST_LOG'], 'a') as log:
        log.write(json.dumps(['archive', sys.argv[-1]]) + '\\n')
    with tarfile.open(fileobj=sys.stdout.buffer, mode='w|') as archive:
        directory = tarfile.TarInfo('src/')
        directory.type = tarfile.DIRTYPE
        directory.mode = 0o755
        archive.addfile(directory)
''')
        git.chmod(0o700)
        docker = self.bin / 'docker'
        docker.write_text('#!' + sys.executable + '''
import hashlib, json, os, sys, time
from pathlib import Path
args = sys.argv[1:]
with open(os.environ['TEST_LOG'], 'a') as log:
    log.write(json.dumps(args) + '\\n')
state = Path(os.environ['TEST_ROOT']) / 'containers'
state.mkdir(exist_ok=True)
if args[:2] == ['buildx', 'build']:
    if os.environ.get('TEST_HEAD_DRIFT'):
        Path(os.environ['TEST_ROOT'], 'head').write_text('b' * 40)
    for option in args:
        if option.startswith('type=local,dest='):
            directory = Path(option.removeprefix('type=local,dest=').removesuffix(',mode=max'))
            if not os.environ.get('TEST_INCOMPLETE_CACHE'):
                (directory / 'index.json').write_text('{}')
    if os.environ.get('TEST_BUILD_FAIL'):
        raise SystemExit(17)
elif args[0] == 'create':
    name = args[args.index('--name') + 1]
    container = hashlib.sha256(name.encode()).hexdigest()
    (state / container).write_text(json.dumps(args))
    Path(args[args.index('--cidfile') + 1]).write_text(container)
    print(container)
elif args[0] == 'start':
    container = args[-1]
    created = json.loads((state / container).read_text())
    install = not any(':/src' in argument for argument in created)
    if os.environ.get('TEST_HOLD') == ('test' if install else 'build'):
        Path(os.environ['TEST_ROOT'], 'started').write_text(container)
        while (state / container).exists():
            time.sleep(0.01)
    if install and os.environ.get('TEST_INSTALL_FAIL'):
        raise SystemExit(19)
elif args[:2] == ['rm', '--force']:
    if os.environ.get('TEST_REMOVE_FAIL'):
        print('cannot remove owned container', file=sys.stderr)
        raise SystemExit(23)
    (state / args[-1]).unlink()
else:
    raise AssertionError(args)
''')
        docker.chmod(0o700)
        self.log = self.root / 'docker.jsonl'
        self.cache = self.root / 'cache'
        self.env = dict(os.environ, PATH=str(self.bin) + os.pathsep + os.environ['PATH'],
                        TEST_ROOT=str(self.root), TEST_LOG=str(self.log),
                        CUPERTINO_BUILD_CACHE_DIR=str(self.cache), CUPERTINO_BUILD_LOCK_FD='test-lease')

    def command(self, distro, mode):
        return ['bash', str(SCRIPT), distro, str(self.root / 'dist'), *mode]

    def run_build(self, mode, overrides, distro):
        return subprocess.run(self.command(distro, mode), env=dict(self.env, **overrides),
                              text=True, capture_output=True)

    def commands(self):
        return [json.loads(line) for line in self.log.read_text().splitlines()]

    def test_caches_image_and_rotates_export_after_success(self):
        first = self.run_build([], {}, 'debian')
        self.assertEqual(first.returncode, 0, first.stderr)
        current = self.cache / 'debian/current'
        old = current.resolve()
        second = self.run_build([], {}, 'debian')
        self.assertEqual(second.returncode, 0, second.stderr)
        self.assertNotEqual(old, current.resolve())
        self.assertFalse(old.exists())
        commands = self.commands()
        builds = [command for command in commands if command[:2] == ['buildx', 'build']]
        self.assertIn('type=local,src=' + str(current), builds[1])
        creates = [command for command in commands if command[0] == 'create']
        build_run = creates[0]
        self.assertIn('type=bind,src=' + str(self.root / 'target/package-build/debian') + ',dst=/src/target', build_run)
        self.assertIn('export CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/src/target', build_run[-1])
        self.assertIn('CARGO_BUILD_JOBS=4', build_run)
        self.assertIn('CARGO_BUILD_JOBS=4', builds[0])
        self.assertFalse(list((self.root / 'containers').iterdir()))
        install_run = creates[1]
        self.assertIn('docker.io/library/debian@sha256:4c25275446ca371766b8b093c696782eabce06263fccacb8d5d813b3dc44b308', install_run)
        self.assertFalse(any('cupertino-package-builder:' in argument for argument in install_run))

    def test_failed_image_export_preserves_previous_cache(self):
        self.assertEqual(self.run_build([], {}, 'debian').returncode, 0)
        current = self.cache / 'debian/current'
        old = current.resolve()
        result = self.run_build([], {'TEST_BUILD_FAIL': '1'}, 'debian')
        self.assertEqual(result.returncode, 17, result.stderr)
        self.assertEqual(current.resolve(), old)
        self.assertEqual(list((self.cache / 'debian').glob('generation.*')), [old])
        self.assertEqual(sum(command[:2] == ['buildx', 'build'] for command in self.commands()), 2)
        self.assertEqual(sum(command[0] == 'create' for command in self.commands()), 2)

    def test_incomplete_export_is_not_published(self):
        self.assertEqual(self.run_build([], {}, 'debian').returncode, 0)
        current = self.cache / 'debian/current'
        old = current.resolve()
        result = self.run_build([], {'TEST_INCOMPLETE_CACHE': '1'}, 'debian')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('builder cache export is incomplete', result.stderr)
        self.assertEqual(current.resolve(), old)
        self.assertEqual(list((self.cache / 'debian').glob('generation.*')), [old])

    def test_install_failure_propagates(self):
        result = self.run_build([], {'TEST_INSTALL_FAIL': '1'}, 'debian')
        self.assertEqual(result.returncode, 19, result.stderr)

    def test_test_only_needs_no_image_build_or_cargo_target(self):
        result = self.run_build(['--test-only'], {}, 'debian')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual([command[0] for command in self.commands() if command[0] != 'archive'],
                         ['create', 'start', 'rm'])
        self.assertFalse((self.root / 'target/package-build').exists())
        self.assertFalse(self.cache.exists())

    def test_arch_test_only_does_not_relabel_existing_packages(self):
        result = self.run_build(['--test-only'], {}, 'arch')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(any(command[0] == 'release' for command in self.commands()))

    def test_arch_new_build_records_only_after_success(self):
        result = self.run_build([], {}, 'arch')
        self.assertEqual(result.returncode, 0, result.stderr)
        commands = self.commands()
        record = ['release', 'record-packages', str(self.root / 'dist'), '--commit', 'a' * 40]
        self.assertIn(record, commands)
        last_start = max(index for index, command in enumerate(commands) if command[0] == 'start')
        self.assertGreater(commands.index(record), last_start)

    def test_arch_source_and_provenance_stay_on_captured_commit_when_head_changes(self):
        result = self.run_build([], {'TEST_HEAD_DRIFT': '1'}, 'arch')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((self.root / 'head').read_text(), 'b' * 40)
        commands = self.commands()
        self.assertIn(['archive', 'a' * 40], commands)
        release_commands = [command for command in commands if command[0] == 'release']
        self.assertEqual([command[1] for command in release_commands], ['source', 'pkgbuild', 'record-packages'])
        for command in release_commands:
            self.assertEqual(command[command.index('--commit') + 1], 'a' * 40)

    def test_arch_install_failure_does_not_record_packages(self):
        result = self.run_build([], {'TEST_INSTALL_FAIL': '1'}, 'arch')
        self.assertEqual(result.returncode, 19, result.stderr)
        self.assertFalse(any(command[:2] == ['release', 'record-packages'] for command in self.commands()))

    def test_cleanup_failure_preserves_original_failure(self):
        result = self.run_build([], {'TEST_INSTALL_FAIL': '1', 'TEST_REMOVE_FAIL': '1'}, 'debian')
        self.assertEqual(result.returncode, 19)
        self.assertIn('cannot remove owned container', result.stderr)

    def interrupt_container(self, hold, interruption, expected):
        containers = self.root / 'containers'
        containers.mkdir()
        unrelated = containers / 'unrelated'
        unrelated.write_text('another build')
        env = dict(self.env, TEST_HOLD=hold, CARGO_BUILD_JOBS='2')
        with subprocess.Popen(self.command('debian', []), env=env, text=True,
                              stdout=subprocess.PIPE, stderr=subprocess.PIPE) as process:
            deadline = time.monotonic() + 5
            while not (self.root / 'started').exists():
                if process.poll() is not None:
                    self.fail(process.communicate()[1])
                if time.monotonic() > deadline:
                    process.kill()
                    self.fail('the fake build container did not start')
                time.sleep(0.01)
            process.send_signal(interruption)
            _, errors = process.communicate(timeout=5)
            self.assertEqual(process.returncode, expected, errors)
        self.assertEqual(list(containers.iterdir()), [unrelated])
        commands = self.commands()
        created = next(command for command in commands if command[0] == 'create')
        self.assertIn('CARGO_BUILD_JOBS=2', created)
        removed = [command[-1] for command in commands if command[:2] == ['rm', '--force']]
        started = [command[-1] for command in commands if command[0] == 'start']
        self.assertEqual(removed, started)
        self.assertFalse(list((self.root / 'target').glob('container-build.*')))

    def test_termination_removes_only_owned_build_container(self):
        self.interrupt_container('build', signal.SIGTERM, 143)

    def test_interrupt_removes_owned_build_and_test_containers(self):
        self.interrupt_container('test', signal.SIGINT, 130)


if __name__ == '__main__':
    unittest.main()
