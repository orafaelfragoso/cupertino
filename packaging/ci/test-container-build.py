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
elif args[:2] == ['image', 'inspect']:
    assert args[2:4] == ['--format', '{{.Id}}']
    print(os.environ.get('TEST_IMAGE_ID', 'sha256:' + 'a' * 64))
elif args[0] == 'create':
    name = args[args.index('--name') + 1]
    container = hashlib.sha256(name.encode()).hexdigest()
    (state / container).write_text(json.dumps(args))
    Path(args[args.index('--cidfile') + 1]).write_text(container)
    print(container)
elif args[0] == 'start':
    container = args[-1]
    created = json.loads((state / container).read_text())
    kind = created[created.index('--name') + 1].rsplit('-', 1)[1]
    install = kind == 'test'
    if os.environ.get('TEST_HOLD') == kind:
        Path(os.environ['TEST_ROOT'], 'started').write_text(container)
        while (state / container).exists():
            time.sleep(0.01)
    if install and os.environ.get('TEST_INSTALL_FAIL'):
        raise SystemExit(19)
    if kind == 'build' and os.environ.get('TEST_COMPILE_FAIL'):
        raise SystemExit(31)
    if kind == 'inspect' and os.environ.get('TEST_INSPECT_FAIL'):
        raise SystemExit(37)
elif args[:2] == ['rm', '--force']:
    if os.environ.get('TEST_REMOVE_FAIL'):
        print('cannot remove owned container', file=sys.stderr)
        raise SystemExit(23)
    (state / args[-1]).unlink()
elif args[0] == 'run':
    owned = args[args.index('chown'):]
    assert owned[:3] == ['chown', '-R', str(os.getuid()) + ':' + str(os.getgid())], args
    assert owned[3:] in (['/dist', '/src', '/cargo-home'], ['/dist', '/src', '/cargo-home', '/cache']), args
else:
    raise AssertionError(args)
''')
        docker.chmod(0o700)
        self.log = self.root / 'docker.jsonl'
        self.cache = self.root / 'cache'
        self.env = dict(os.environ, PATH=str(self.bin) + os.pathsep + os.environ['PATH'],
                        TEST_ROOT=str(self.root), TEST_LOG=str(self.log), HOME=str(self.root / 'home'),
                        CUPERTINO_BUILD_CACHE_DIR=str(self.cache), CUPERTINO_BUILD_LOCK_FD='test-lease',
                        CUPERTINO_CONTAINER_CACHE='')
        for name in ('CARGO_BUILD_JOBS', 'CUPERTINO_BUILD_LOCK_HOLD', 'GITHUB_ACTIONS', 'CUPERTINO_PACKAGE_SCCACHE_SIZE'):
            self.env.pop(name, None)
        self.processors = str(len(os.sched_getaffinity(0)))

    def command(self, distro, mode):
        return ['bash', str(SCRIPT), distro, str(self.root / 'dist'), *mode]

    def run_build(self, mode, overrides, distro):
        return subprocess.run(self.command(distro, mode), env=dict(self.env, **overrides),
                              text=True, capture_output=True)

    def commands(self):
        return [json.loads(line) for line in self.log.read_text().splitlines()]

    def test_caches_image_and_rotates_export_after_success(self):
        first = self.run_build([], {}, 'arch')
        self.assertEqual(first.returncode, 0, first.stderr)
        current = self.cache / 'arch/current'
        old = current.resolve()
        cached_source = self.root / 'target/package-build/arch/cargo-home/cached-source'
        cached_source.write_bytes(b'previously unpacked dependency')
        cached_mtime = cached_source.stat().st_mtime_ns
        second = self.run_build([], {}, 'arch')
        self.assertEqual(second.returncode, 0, second.stderr)
        self.assertNotEqual(old, current.resolve())
        self.assertFalse(old.exists())
        self.assertEqual(cached_source.read_bytes(), b'previously unpacked dependency')
        self.assertEqual(cached_source.stat().st_mtime_ns, cached_mtime)
        commands = self.commands()
        builds = [command for command in commands if command[:2] == ['buildx', 'build']]
        self.assertIn('type=local,src=' + str(current), builds[1])
        creates = [command for command in commands if command[0] == 'create']
        build_run = creates[0]
        self.assertFalse(any(argument.endswith('dst=/src/target') for argument in build_run))
        self.assertEqual(build_run[-4], 'sha256:' + 'a' * 64)
        self.assertIn('export CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/src/target', build_run[-1])
        self.assertIn('\nmkdir /src/target\n', build_run[-1])
        self.assertIn(' /dist /src /cargo-home${SCCACHE_DIR:+ /cache}\' EXIT', build_run[-1])
        self.assertIn('CARGO_BUILD_JOBS=' + self.processors, build_run)
        self.assertIn('CARGO_HOME=/cargo-home', build_run)
        cargo_home = self.root / 'target/package-build/arch/cargo-home'
        self.assertIn('type=bind,src=' + str(cargo_home) + ',dst=/cargo-home', build_run)
        self.assertTrue(cargo_home.is_dir())
        self.assertFalse(any(argument.startswith('PATH=') for argument in build_run))
        self.assertIn('CARGO_BUILD_JOBS=' + self.processors, builds[0])
        self.assertFalse(list((self.root / 'containers').iterdir()))
        inspect_run = creates[1]
        self.assertIn('namcap arch-build/PKGBUILD', inspect_run[-1])
        self.assertNotIn('makepkg --noconfirm\'', inspect_run[-1])
        self.assertNotIn('namcap', build_run[-1])
        self.assertEqual(inspect_run[inspect_run.index('--network') + 1], 'none')
        self.assertFalse(any('/cargo-home' in argument for argument in inspect_run[:-1]))
        install_run = creates[2]
        self.assertIn('docker.io/library/archlinux@sha256:3cbf69d2d5958d6a294f3722c5668e6187c4df8e00fa27f72352f0bf83e495e1', install_run)
        self.assertFalse(any('cupertino-package-builder:' in argument for argument in install_run))
        self.assertFalse(any(command[0] == 'run' for command in commands))

    def test_every_build_compiles_in_a_fresh_target_and_keeps_only_downloads(self):
        first_id, second_id = 'sha256:' + 'a' * 64, 'sha256:' + 'b' * 64
        first = self.run_build([], {'TEST_IMAGE_ID': first_id}, 'arch')
        self.assertEqual(first.returncode, 0, first.stderr)
        cargo_home = self.root / 'target/package-build/arch/cargo-home'
        source = cargo_home / 'cached-source'
        source.write_text('downloaded once')
        stamp = source.stat().st_mtime_ns
        second = self.run_build([], {'TEST_IMAGE_ID': second_id}, 'arch')
        self.assertEqual(second.returncode, 0, second.stderr)
        self.assertEqual(source.read_text(), 'downloaded once')
        self.assertEqual(source.stat().st_mtime_ns, stamp)
        builds = [command for command in self.commands() if command[0] == 'create' and command[-1].startswith('set')
                  and 'makepkg --noconfirm\'' in command[-1]]
        self.assertEqual(len(builds), 2)
        for command, image_id in zip(builds, (first_id, second_id), strict=True):
            self.assertIn('type=bind,src=' + str(cargo_home) + ',dst=/cargo-home', command)
            self.assertFalse(any(argument.endswith('dst=/src/target') or 'dst=/cache' in argument for argument in command))
            self.assertNotIn('-v', [argument for argument in command if argument.endswith(':/src/target')])
            self.assertEqual(command[-4], image_id)
        self.assertFalse((self.root / 'target/package-build/arch/targets').exists())
        self.assertFalse(list((self.root / 'target').glob('container-build.*')))

    def test_invalid_image_identity_cannot_create_a_target_or_container(self):
        result = self.run_build([], {'TEST_IMAGE_ID': 'sha256:../../escape'}, 'arch')
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn('invalid content identity', result.stderr)
        self.assertFalse(any(command[0] == 'create' for command in self.commands()))
        self.assertFalse((self.root / 'target/package-build/arch/targets').exists())

    def test_failed_image_export_preserves_previous_cache(self):
        self.assertEqual(self.run_build([], {}, 'arch').returncode, 0)
        current = self.cache / 'arch/current'
        old = current.resolve()
        result = self.run_build([], {'TEST_BUILD_FAIL': '1'}, 'arch')
        self.assertEqual(result.returncode, 17, result.stderr)
        self.assertEqual(current.resolve(), old)
        self.assertEqual(list((self.cache / 'arch').glob('generation.*')), [old])
        self.assertEqual(sum(command[:2] == ['buildx', 'build'] for command in self.commands()), 2)
        self.assertEqual(sum(command[0] == 'create' for command in self.commands()), 3)

    def test_incomplete_export_is_not_published(self):
        self.assertEqual(self.run_build([], {}, 'arch').returncode, 0)
        current = self.cache / 'arch/current'
        old = current.resolve()
        result = self.run_build([], {'TEST_INCOMPLETE_CACHE': '1'}, 'arch')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('builder cache export is incomplete', result.stderr)
        self.assertEqual(current.resolve(), old)
        self.assertEqual(list((self.cache / 'arch').glob('generation.*')), [old])

    def test_install_failure_propagates(self):
        result = self.run_build([], {'TEST_INSTALL_FAIL': '1'}, 'arch')
        self.assertEqual(result.returncode, 19, result.stderr)

    def test_failed_compilation_repairs_only_its_owned_bind_mounts(self):
        result = self.run_build([], {'TEST_COMPILE_FAIL': '1'}, 'arch')
        self.assertEqual(result.returncode, 31, result.stderr)
        repairs = [command for command in self.commands() if command[0] == 'run']
        self.assertEqual(len(repairs), 1)
        self.assertIn(str(self.root / 'dist') + ':/dist', repairs[0])
        self.assertTrue(any(argument.endswith(':/src') for argument in repairs[0]))
        self.assertIn('type=bind,src=' + str(self.root / 'target/package-build/arch/cargo-home') + ',dst=/cargo-home', repairs[0])
        self.assertFalse(any(command[0] == 'create' and '-inspect' in ' '.join(command) for command in self.commands()))

    def test_failed_inspection_repairs_ownership_and_records_nothing(self):
        result = self.run_build([], {'TEST_INSPECT_FAIL': '1'}, 'arch')
        self.assertEqual(result.returncode, 37, result.stderr)
        commands = self.commands()
        self.assertEqual(len([command for command in commands if command[0] == 'run']), 1)
        self.assertFalse(any(command[0] == 'release' and command[1] == 'record-packages' for command in commands))
        self.assertFalse(any(command[0] == 'create' and '-test' in command[command.index('--name') + 1]
                             for command in commands if command[0] == 'create'))

    def test_test_only_needs_no_image_build_or_cargo_target(self):
        result = self.run_build(['--test-only'], {}, 'arch')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual([command[0] for command in self.commands() if command[0] != 'archive'],
                         ['create', 'start', 'rm'])
        self.assertFalse((self.root / 'target/package-build').exists())
        self.assertFalse(self.cache.exists())

    def test_test_only_acquires_lease_before_creating_container_control_files(self):
        guard = self.root / 'ci/worktree-build.py'
        guard.parent.mkdir()
        guard.write_text('import json, os, sys\n'
                         + 'with open(os.environ["TEST_LOG"], "a") as log:\n'
                         + ' log.write(json.dumps(["guard", *sys.argv[1:]]) + "\\n")\n'
                         + 'raise SystemExit(42)\n')
        result = self.run_build(['--test-only'], {'CUPERTINO_BUILD_LOCK_FD': ''}, 'arch')
        self.assertEqual(result.returncode, 42, result.stderr)
        self.assertEqual(self.commands(), [['guard', 'locked', '--hold', 'none', '--', 'bash',
                                           'packaging/ci/build-in-container.sh', 'arch', str(self.root / 'dist'),
                                           '--test-only']])
        self.assertFalse((self.root / 'target').exists())
        self.assertFalse((self.root / 'containers').exists())

    def test_only_arch_is_packaged(self):
        for distro in ('debian', 'fedora'):
            result = self.run_build([], {}, distro)
            self.assertEqual(result.returncode, 2, result.stderr)
            self.assertIn('only arch is packaged', result.stderr)
        self.assertFalse(self.log.exists())

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
        created = next(command for command in commands if command[0] == 'create')
        self.assertIn('chown -R builder /src/arch-build /src/target /cargo-home', created[-1])
        self.assertIn('CARGO_HOME=/cargo-home', created)

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
        result = self.run_build([], {'TEST_INSTALL_FAIL': '1', 'TEST_REMOVE_FAIL': '1'}, 'arch')
        self.assertEqual(result.returncode, 19)
        self.assertIn('cannot remove owned container', result.stderr)

    def guard(self):
        """A stand-in ci/worktree-build.py that records each phase's hold and then runs the phase."""
        guard = self.root / 'ci/worktree-build.py'
        guard.parent.mkdir(exist_ok=True)
        guard.write_text('import json, os, sys\n'
                         'with open(os.environ["TEST_LOG"], "a") as log:\n'
                         '    log.write(json.dumps(["guard", *sys.argv[1:4]]) + "\\n")\n'
                         'assert sys.argv[1:3] == ["locked", "--hold"] and sys.argv[4] == "--", sys.argv\n'
                         'os.execvp(sys.argv[5], sys.argv[5:])\n')

    def test_only_the_compile_holds_every_slot_and_the_image_build_one(self):
        self.guard()
        for mode in ([], ['--upgrade']):
            with self.subTest(mode=mode):
                self.log.unlink(missing_ok=True)
                result = self.run_build(mode, {'CUPERTINO_BUILD_LOCK_HOLD': 'none'}, 'arch')
                self.assertEqual(result.returncode, 0, result.stderr)
                sequence = []
                for command in self.commands():
                    if command[0] == 'guard':
                        sequence.append('hold ' + command[3])
                    elif command[0] in ('start', 'create') or command[:2] == ['buildx', 'build']:
                        name = command[command.index('--name') + 1].rsplit('-', 1)[1] if command[0] == 'create' else ''
                        sequence.append(command[0] if command[0] != 'buildx' else 'image') if not name else \
                            sequence.append('create ' + name)
                self.assertEqual(sequence, ['hold one', 'image', 'create build', 'hold every', 'start',
                                            'create inspect', 'start', 'create test', 'start'])

    def test_a_caller_holding_every_slot_runs_each_phase_without_asking_again(self):
        self.guard()
        result = self.run_build([], {'CUPERTINO_BUILD_LOCK_HOLD': 'every'}, 'arch')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(any(command[0] == 'guard' for command in self.commands()))

    def test_the_compile_cache_mounts_sccache_and_shared_downloads_into_the_compile_only(self):
        cache = self.root / 'container-cache'
        result = self.run_build([], {'CUPERTINO_CONTAINER_CACHE': str(cache)}, 'arch')
        self.assertEqual(result.returncode, 0, result.stderr)
        creates = {command[command.index('--name') + 1].rsplit('-', 1)[1]: command
                   for command in self.commands() if command[0] == 'create'}
        build = creates['build']
        self.assertIn('type=bind,src=' + str(cache / 'package/sccache') + ',dst=/cache/sccache', build)
        self.assertIn('type=bind,src=' + str(cache / 'package/cargo-home') + ',dst=/cargo-home', build)
        self.assertIn('if [ -d /cache/sccache ]; then\n    export RUSTC_WRAPPER=/usr/bin/sccache SCCACHE_DIR=/cache/sccache '
                      'SCCACHE_CACHE_SIZE=12G', build[-1])
        self.assertIn("su builder -c 'sccache --stop-server'", build[-1])
        self.assertIn("su builder -c 'cd /src/arch-build && makepkg --noconfirm'", build[-1])
        for kind in ('inspect', 'test'):
            self.assertFalse(any('/cache' in argument or '/cargo-home' in argument for argument in creates[kind][:-1]), kind)
        self.assertFalse((self.root / 'target/package-build').exists())
        failed = self.run_build([], {'CUPERTINO_CONTAINER_CACHE': str(cache), 'TEST_COMPILE_FAIL': '1',
                                     'CUPERTINO_PACKAGE_SCCACHE_SIZE': '3G'}, 'arch')
        self.assertEqual(failed.returncode, 31, failed.stderr)
        repair = next(command for command in self.commands() if command[0] == 'run')
        self.assertIn('type=bind,src=' + str(cache / 'package/sccache') + ',dst=/cache/sccache', repair)
        self.assertEqual(repair[-4:], ['/dist', '/src', '/cargo-home', '/cache'])
        self.assertIn('SCCACHE_CACHE_SIZE=3G', [command for command in self.commands() if command[0] == 'create'][-1][-1])

    def test_the_compile_cache_defaults_to_the_user_cache_and_is_off_on_github_actions(self):
        environment = {name: value for name, value in self.env.items() if name != 'CUPERTINO_CONTAINER_CACHE'}
        default = self.root / 'home/.cache/cupertino-build/containers/package'
        for github, expected in (('true', False), ('', True)):
            with self.subTest(github=github):
                self.log.unlink(missing_ok=True)
                result = subprocess.run(self.command('arch', []), env=dict(environment, GITHUB_ACTIONS=github),
                                        text=True, capture_output=True)
                self.assertEqual(result.returncode, 0, result.stderr)
                build = next(command for command in self.commands() if command[0] == 'create')
                self.assertEqual(any(argument.endswith('dst=/cache/sccache') for argument in build), expected)
                self.assertEqual('type=bind,src=' + str(default / 'cargo-home') + ',dst=/cargo-home' in build, expected)
                self.assertEqual((default / 'sccache').is_dir(), expected)

    def interrupt_container(self, hold, interruption, expected):
        containers = self.root / 'containers'
        containers.mkdir()
        unrelated = containers / 'unrelated'
        unrelated.write_text('another build')
        env = dict(self.env, TEST_HOLD=hold, CARGO_BUILD_JOBS='2')
        with subprocess.Popen(self.command('arch', []), env=env, text=True,
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
        repairs = [command for command in commands if command[0] == 'run']
        if hold in ('build', 'inspect'):
            self.assertEqual(len(repairs), 1)
            repair = repairs[0]
            self.assertEqual(repair[1:5], ['--rm', '--network', 'none', '-v'])
            self.assertEqual(repair[-7], 'sha256:' + 'a' * 64)
            self.assertIn('type=bind,src=' + str(self.root / 'target/package-build/arch/cargo-home') + ',dst=/cargo-home', repair)
            removal_index = max(index for index, command in enumerate(commands) if command[:2] == ['rm', '--force'])
            self.assertGreater(commands.index(repair), removal_index)
        else:
            self.assertEqual(repairs, [])
        self.assertFalse(list((self.root / 'target').glob('container-build.*')))

    def test_termination_removes_only_owned_build_container(self):
        self.interrupt_container('build', signal.SIGTERM, 143)

    def test_termination_during_inspection_removes_its_container_and_repairs_ownership(self):
        self.interrupt_container('inspect', signal.SIGTERM, 143)

    def test_interrupt_removes_owned_build_and_test_containers(self):
        self.interrupt_container('test', signal.SIGINT, 130)


if __name__ == '__main__':
    unittest.main()
