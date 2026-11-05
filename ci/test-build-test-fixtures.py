#!/usr/bin/env python3
"""Exercise fixture preparation without compiling the workspace."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


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
            self.assertEqual(json.loads(log.read_text()), ['build', '--workspace', '--all-features', '--all-targets',
                                                           '--message-format=json-render-diagnostics', '--frozen', '--release'])

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
            cargo.write_text('#!' + sys.executable + '\nimport os\n'
                             + 'os.write(int(os.environ["CUPERTINO_BUILD_LOCK_FD"]), b"cargo\\n")\n'
                             + f'print({json.dumps(message)!r})\n')
            cargo.chmod(0o700)
            child = 'import os; os.write(int(os.environ["CUPERTINO_BUILD_LOCK_FD"]), b"tests\\n")'
            with (scratch / 'lease').open('w+b') as lease:
                descriptor = lease.fileno()
                result = subprocess.run([sys.executable, str(SCRIPT), '--locked', '--', sys.executable, '-c', child],
                                        env=dict(os.environ, CARGO=str(cargo), CUPERTINO_BUILD_LOCK_FD=str(descriptor)),
                                        pass_fds=(descriptor,), capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stderr)
                lease.seek(0)
                self.assertEqual(lease.read(), b'cargo\ntests\n')


if __name__ == '__main__':
    unittest.main()
