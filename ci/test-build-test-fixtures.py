#!/usr/bin/env python3
"""Exercise fixture preparation without compiling the workspace."""
import importlib.util
import json
import os
from pathlib import Path
import shlex
import subprocess
import sys
import tempfile
import unittest
import unittest.mock


SCRIPT = Path(__file__).with_name('build-test-fixtures.py')
SPEC = importlib.util.spec_from_file_location('fixtures', SCRIPT)
fixtures = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(fixtures)


class Artifacts(unittest.TestCase):
    def message(self, package, kind, name, executable, filenames, test):
        return {'reason': 'compiler-artifact', 'manifest_path': str(fixtures.ROOT / 'crates' / package / 'Cargo.toml'),
                'profile': {'test': test}, 'target': {'kind': [kind], 'crate_types': [kind], 'name': name},
                'executable': executable, 'filenames': filenames}

    def test_uses_exact_artifacts_and_excludes_test_harnesses_and_dependencies(self):
        client = self.message('cupertino-testclient', 'bin', 'cupertino-testclient', '/build/client', [], False)
        example = self.message('cupertino-kit', 'example', 'hello-world', '/build/example', [], False)
        library = self.message('cupertino-ui-abi', 'cdylib', 'cupertinokit', None, ['/build/lib.so', '/build/lib.rlib'], False)
        harness = self.message('cupertino-kit', 'example', 'hello-world', '/build/example-test', [], True)
        dependency = dict(client, manifest_path='/cargo/registry/client/Cargo.toml')
        self.assertEqual(fixtures.collect_artifacts([client, example, library, harness, dependency]), {
            fixtures.artifact_key('cupertino-testclient', 'bin', 'cupertino-testclient'): Path('/build/client'),
            fixtures.artifact_key('cupertino-kit', 'example', 'hello-world'): Path('/build/example'),
            fixtures.artifact_key('cupertino-ui-abi', 'cdylib', 'cupertinokit'): Path('/build/lib.so'),
        })

    def test_rejects_ambiguous_cdylib_artifacts(self):
        message = self.message('cupertino-ui-abi', 'cdylib', 'cupertinokit', None, ['/build/one.so', '/build/two.so'], False)
        with self.assertRaisesRegex(ValueError, 'expected one cdylib'):
            fixtures.collect_artifacts([message])

    def test_command_receives_fresh_artifacts_and_session_expires(self):
        with tempfile.TemporaryDirectory() as directory:
            scratch = Path(directory)
            binary = scratch / 'client'
            binary.write_text('current artifact')
            message = self.message('cupertino-testclient', 'bin', 'cupertino-testclient', str(binary), [], False)
            cargo = scratch / 'cargo'
            log = scratch / 'arguments.json'
            cargo.write_text('#!' + sys.executable + '\nimport json, sys\nfrom pathlib import Path\n'
                             + f'Path({str(log)!r}).write_text(json.dumps(sys.argv[1:]))\n'
                             + f'print({json.dumps(message)!r})\n')
            cargo.chmod(0o700)
            key = fixtures.artifact_key('cupertino-testclient', 'bin', 'cupertino-testclient')
            child = 'import json, os; print(json.dumps({k:v for k,v in os.environ.items() if k.startswith("CUPERTINO_TEST_FIXTURE_")}))'
            result = subprocess.run([sys.executable, str(SCRIPT), '--frozen', '--release', '--', sys.executable, '-c', child],
                                    env=dict(os.environ, CARGO=str(cargo), CUPERTINO_TEST_FIXTURE_OBSOLETE='stale'),
                                    text=True, capture_output=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            env = json.loads(result.stdout)
            self.assertEqual(env[key], str(binary))
            self.assertEqual(env[key + '_STAMP'], f'{binary.stat().st_size}:{binary.stat().st_mtime_ns}')
            self.assertNotIn('CUPERTINO_TEST_FIXTURE_OBSOLETE', env)
            self.assertFalse(Path(env[fixtures.PREFIX + 'SESSION']).exists())
            self.assertEqual(json.loads(log.read_text()), ['build', '--all-features', '--all-targets', '--timings',
                                                           '--message-format=json-render-diagnostics', '--workspace', '--frozen', '--release'])

    def test_failed_build_does_not_run_tests(self):
        with tempfile.TemporaryDirectory() as directory:
            cargo = Path(directory) / 'cargo'
            cargo.write_text('#!' + sys.executable + '\nraise SystemExit(23)\n')
            cargo.chmod(0o700)
            result = subprocess.run([sys.executable, str(SCRIPT), '--', sys.executable, '-c', 'raise SystemExit(99)'],
                                    env=dict(os.environ, CARGO=str(cargo)), capture_output=True)
            self.assertEqual(result.returncode, 23, result.stderr)

    def test_child_failure_is_reported(self):
        with tempfile.TemporaryDirectory() as directory:
            cargo = Path(directory) / 'cargo'
            message = self.message('cupertino-testclient', 'bin', 'cupertino-testclient', str(cargo), [], False)
            cargo.write_text('#!' + sys.executable + '\n' + f'print({json.dumps(message)!r})\n')
            cargo.chmod(0o700)
            result = subprocess.run([sys.executable, str(SCRIPT), '--', sys.executable, '-c', 'raise SystemExit(31)'],
                                    env=dict(os.environ, CARGO=str(cargo)), capture_output=True)
            self.assertEqual(result.returncode, 31, result.stderr)

    def test_resource_lease_reaches_cargo_and_test_children(self):
        with tempfile.TemporaryDirectory() as directory:
            scratch = Path(directory)
            cargo = scratch / 'cargo'
            message = self.message('cupertino-testclient', 'bin', 'cupertino-testclient', str(cargo), [], False)
            # Every descriptor in the list, as the wrapper passes a worktree lock and one or more slots.
            each = 'for d in os.environ["CUPERTINO_BUILD_LOCK_FD"].split(","): os.write(int(d), b"{}\\n")\n'
            cargo.write_text('#!' + sys.executable + '\nimport os\n' + each.format('cargo')
                             + f'print({json.dumps(message)!r})\n')
            cargo.chmod(0o700)
            child = 'import os\n' + each.format('tests')
            with (scratch / 'lease').open('w+b') as lease, (scratch / 'slot').open('w+b') as slot:
                descriptors = (lease.fileno(), slot.fileno())
                result = subprocess.run([sys.executable, str(SCRIPT), '--locked', '--', sys.executable, '-c', child],
                                        env=dict(os.environ, CARGO=str(cargo),
                                                 CUPERTINO_BUILD_LOCK_FD=','.join(map(str, descriptors))),
                                        pass_fds=descriptors, capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stderr)
                for stream in (lease, slot):
                    stream.seek(0)
                    self.assertEqual(stream.read(), b'cargo\ntests\n')


# A model of Cargo's feature unification: an artifact's contents follow the package and feature selection of the
# command that built it. A test command rebuilds and relinks it when its own selection differs, then checks the
# stamp the preparation recorded, as ci/test-fixtures.rs does.
MODEL_CARGO = """
import json, os, sys
from pathlib import Path
arguments = sys.argv[1:]
artifact = Path(os.environ['MODEL_ARTIFACT'])
end = arguments.index('--') if '--' in arguments else len(arguments)
options = arguments[:end]
packages = sorted(value for flag, value in zip(options, options[1:]) if flag in ('-p', '--package'))
unit = json.dumps([packages or ['workspace'], '--all-features' in options])
with open(os.environ['MODEL_LOG'], 'a') as log:
    log.write(json.dumps(arguments) + '\\n')
if not artifact.exists() or artifact.read_text() != unit:
    previous = artifact.stat().st_mtime_ns if artifact.exists() else 0
    artifact.write_text(unit)
    os.utime(artifact, ns=(previous + 10**9, previous + 10**9))
if arguments[:2] == ['nextest', 'run']:
    key = os.environ['MODEL_KEY']
    stamp = artifact.stat()
    if os.environ[key + '_STAMP'] != f'{stamp.st_size}:{stamp.st_mtime_ns}':
        sys.exit('fixture changed after preparation: ' + str(artifact))
    sys.exit(0)
print(json.dumps({'reason': 'compiler-artifact', 'manifest_path': os.environ['MODEL_MANIFEST'],
                  'profile': {'test': False}, 'target': {'kind': ['example'], 'crate_types': ['bin'],
                  'name': 'simple-window'}, 'executable': str(artifact), 'filenames': [str(artifact)]}))
"""


class Selection(unittest.TestCase):
    def run_model(self, wrapper, child):
        with tempfile.TemporaryDirectory() as directory:
            scratch = Path(directory)
            cargo = scratch / 'cargo'
            cargo.write_text('#!' + sys.executable + '\n' + MODEL_CARGO)
            cargo.chmod(0o700)
            log = scratch / 'log'
            env = dict(os.environ, CARGO=str(cargo), MODEL_ARTIFACT=str(scratch / 'simple-window'), MODEL_LOG=str(log),
                       MODEL_KEY=fixtures.artifact_key('cupertino-wayland-client', 'example', 'simple-window'),
                       MODEL_MANIFEST=str(fixtures.ROOT / 'crates' / 'cupertino-wayland-client' / 'Cargo.toml'))
            result = subprocess.run([sys.executable, str(SCRIPT), *wrapper, '--', str(cargo), *child], env=env,
                                    text=True, capture_output=True)
            return result, [json.loads(line) for line in log.read_text().splitlines()]

    def test_a_package_test_command_finds_the_fixtures_it_builds_unchanged(self):
        # The transport's compositor job: fixtures for the workspace, tests of one package with its own features.
        child = ['nextest', 'run', '--locked', '-p', 'cupertino-wayland-client', '--run-ignored', 'only',
                 '--no-fail-fast']
        for wrapper in (['--locked'], ['--locked', '--package', 'cupertino-wayland-client'],
                        ['--locked', '--package', 'cupertino-kit', '--package', 'cupertino-wayland-client']):
            with self.subTest(wrapper=wrapper):
                result, calls = self.run_model(wrapper, child)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(calls[-2], ['test', '--no-run', '--message-format=json-render-diagnostics', '--locked',
                                             '--package', 'cupertino-wayland-client'])
                self.assertEqual(calls[-1], child)

    def test_the_test_command_build_is_prepared_last_with_its_own_options(self):
        builds, command = fixtures.preparation([
            '--frozen', '--release', '--', 'cargo', '--locked', 'nextest', 'run', '--release', '-pcupertino-kit',
            '--package=cupertino-server', '--exclude', 'x', '-F', 'app', '--features=a,b', '--no-default-features',
            '--test', 'goldens', '-P', 'ci', '--profile', 'ci', '-E', 'package(cupertino-kit)', '--timings',
            '--target-dir', 'out', '--', '--include-ignored', '-p', 'ignored'])
        cargo = os.environ.get('CARGO', 'cargo')
        self.assertEqual(command[0], 'cargo')
        self.assertEqual(builds, [
            [cargo, 'build', '--all-features', '--all-targets', '--timings', '--message-format=json-render-diagnostics',
             '--workspace', '--frozen', '--locked', '--release', '--target-dir', 'out'],
            [cargo, 'test', '--no-run', '--message-format=json-render-diagnostics', '--locked', '--release',
             '--package', 'cupertino-kit', '--package', 'cupertino-server', '--exclude', 'x', '--features', 'app',
             '--features', 'a,b', '--no-default-features', '--test', 'goldens', '--target-dir', 'out', '--frozen'],
        ])
        builds, _ = fixtures.preparation(['--', 'cargo', 'test', '--profile', 'ci', '--', '--profile'])
        self.assertEqual(builds[1][4:], ['--profile', 'ci'])
        self.assertEqual(builds[0][-2:], ['--profile', 'ci'])
        builds, _ = fixtures.preparation(['--profile', 'dev', '--', 'cargo', 'nextest', 'run', '--cargo-profile', 'dev'])
        self.assertEqual(builds[1][4:], ['--profile', 'dev'])

    def test_a_profile_target_or_directory_the_test_command_does_not_build_is_refused(self):
        for wrapper, child in (
            (['--release'], ['nextest', 'run', '--workspace']),
            (['--profile', 'dev'], ['nextest', 'run', '--release']),
            (['--profile', 'ci'], ['test', '--cargo-profile', 'ci']),
            (['--target', 'aarch64-unknown-linux-gnu'], ['test']),
            (['--target-dir', 'a'], ['test', '--target-dir', 'b']),
        ):
            with self.subTest(wrapper=wrapper, child=child), self.assertRaises(SystemExit), \
                    unittest.mock.patch('sys.stderr'):
                fixtures.preparation([*wrapper, '--', 'cargo', *child])

    def test_a_cargo_command_whose_build_cannot_be_reproduced_is_refused(self):
        for child in (['build'], ['nextest', 'list'], ['+nightly', 'test'], ['test', '--doc'],
                      ['test', '--config', 'profile.dev.opt-level=1'], ['nextest', 'run', '--manifest-path', 'x'],
                      ['test', '-Zbuild-std'], ['test', '--package']):
            with self.subTest(child=child), self.assertRaises(SystemExit), unittest.mock.patch('sys.stderr'):
                fixtures.preparation(['--', 'cargo', *child])

    def test_every_tracked_invocation_prepares_its_test_command(self):
        sources = [*sorted((fixtures.ROOT / '.github' / 'workflows').glob('*.yml')),
                   *sorted((fixtures.ROOT / 'ci').glob('*.sh')),
                   *sorted((fixtures.ROOT / 'packaging').glob('**/PKGBUILD'))]
        found = 0
        for source in sources:
            text = source.read_text().replace('\\\n', ' ')
            for line in text.splitlines():
                if 'ci/build-test-fixtures.py' not in line or line.lstrip().startswith('#'):
                    continue
                words = shlex.split(line[line.index('ci/build-test-fixtures.py'):].split('>')[0])
                with self.subTest(source=source.name, command=' '.join(words)):
                    builds, command = fixtures.preparation(words[1:])
                    found += 1
                    self.assertEqual(Path(command[0]).name, 'cargo')
                    self.assertEqual(builds[-1][1:3], ['test', '--no-run'])
        self.assertGreaterEqual(found, 3)


if __name__ == '__main__':
    unittest.main()
