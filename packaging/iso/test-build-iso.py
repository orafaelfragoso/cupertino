"""Exercise ISO orchestration and cancellation without Docker or compilation."""
import contextlib
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import tempfile
import time
import unittest

ROOT = Path(__file__).resolve().parents[2]

DOCKER = '''#!/usr/bin/env python3
import json, os, signal, sys, time
from pathlib import Path
state = Path(os.environ['TEST_STATE'])
args = sys.argv[1:]
with (state / 'events').open('a') as log:
    log.write(json.dumps(args) + '\\n')
if args[0] == 'create':
    volumes = {}
    for i, arg in enumerate(args):
        if arg == '-v':
            host, destination, *_ = args[i + 1].split(':')
            volumes[destination] = host
    kind = 'iso' if '/w' in volumes else 'keyring'
    cid = 'fixture-' + kind
    data = {'kind': kind, 'volumes': volumes}
    (state / cid).write_text(json.dumps(data))
    (state / ('stopped-' + cid)).unlink(missing_ok=True)
    Path(args[args.index('--cidfile') + 1]).write_text(cid)
    if kind == 'iso':
        work = Path(volumes['/w'])
        assert (work / 'profile/committed-profile').read_text() == 'committed'
        assert (work / 'profile/airootfs/usr/local/bin/cupertino-install').read_text() == 'committed-installer'
    print(cid)
elif args[0] == 'start':
    cid = args[-1]
    data = json.loads((state / cid).read_text())
    if os.environ.get('TEST_STALL') == data['kind']:
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
        (state / 'ready').write_text(cid)
        while not (state / ('stopped-' + cid)).exists():
            time.sleep(0.01)
        sys.exit(0)
    if data['kind'] == 'keyring':
        Path(data['volumes']['/out'], 'cupertino-keyring-1-any.pkg.tar.zst').write_text('keyring')
    else:
        work = Path(data['volumes']['/w'])
        (work / 'iso-out').mkdir()
        (work / 'iso-out/fixture.iso').write_text('iso')
        (work / 'iso-work/iso/arch').mkdir(parents=True)
        (work / 'iso-work/iso/arch/pkglist.x86_64.txt').write_text('fixture-package 1')
        (work / 'archiso-version').write_text('archiso 1')
    if data['kind'] == 'iso':
        sys.exit(int(os.environ.get('TEST_START_STATUS', '0')))
elif args[0] == 'rm':
    cid = args[-1]
    (state / ('stopped-' + cid)).write_text('stopped')
    if os.environ.get('TEST_RM_FAILURE') == cid:
        print('fixture Docker removal failure', file=sys.stderr)
        sys.exit(23)
elif args[0] == 'run':
    if os.environ.get('TEST_CLEANUP_FAILURE') == '1':
        print('fixture Docker filesystem cleanup failure', file=sys.stderr)
        sys.exit(24)
else:
    raise SystemExit('unexpected Docker command: ' + repr(args))
'''

RELEASE = '''import argparse, json, os, shutil
from pathlib import Path
parser = argparse.ArgumentParser()
parser.add_argument('command', choices=['verify-packages'])
parser.add_argument('directory', type=Path)
parser.add_argument('--commit', required=True)
parser.add_argument('--tag')
parser.add_argument('--copy-to', type=Path, required=True)
args = parser.parse_args()
record = json.loads((args.directory / 'packages.json').read_text())
assert record['commit'] == args.commit
state = Path(os.environ['TEST_STATE'])
(state / 'verified-commit').write_text(args.commit)
args.copy_to.mkdir(exist_ok=True)
for path in args.directory.iterdir():
    shutil.copy(path, args.copy_to / path.name)
if os.environ.get('TEST_MUTATE_SOURCE') == '1':
    root = Path(__file__).resolve().parent.parent
    (root / 'packaging/iso/committed-profile').write_text('changed-during-packaging')
    (root / 'packaging/installer/cupertino_install.py').write_text('changed-during-packaging')
    (root / 'packaging/keyring/release-key.fpr').write_text('changed-during-packaging')
'''


class IsoBuildTest(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix='cupertino-iso-test-', dir=Path.home())
        self.addCleanup(self.temporary.cleanup)
        self.base = Path(self.temporary.name)
        self.root = self.base / 'repo'
        self.state = self.base / 'state'
        self.state.mkdir()
        self.bin = self.base / 'bin'
        self.bin.mkdir()
        for directory in ('scripts', 'packaging/iso/airootfs/etc', 'packaging/installer', 'packaging/keyring', 'packaging/ci'):
            (self.root / directory).mkdir(parents=True, exist_ok=True)
        shutil.copy(ROOT / 'scripts/build-iso.sh', self.root / 'scripts/build-iso.sh')
        shutil.copy(ROOT / 'packaging/keyring/build-keyring', self.root / 'packaging/keyring/build-keyring')
        (self.root / 'packaging/iso/committed-profile').write_text('committed')
        (self.root / 'packaging/iso/airootfs-links').write_text('etc/fixture /dev/null\n')
        (self.root / 'packaging/iso/pacman.conf').write_text('[options]\n')
        (self.root / 'packaging/installer/cupertino_install.py').write_text('committed-installer')
        (self.root / 'packaging/keyring/release-key.fpr').write_text('committed-key')
        check = self.root / 'packaging/keyring/check-key'
        check.write_text('#!/bin/sh\nset -eu\n[ "$(cat "$2")" = committed-key ]\nprintf "%s\\n" "$2" > "$TEST_STATE/checked-fingerprint"\n')
        check.chmod(0o755)
        (self.root / 'packaging/release.py').write_text(RELEASE)
        builder = self.root / 'packaging/ci/build-in-container.sh'
        builder.write_text("""#!/usr/bin/env python3
import json, os, subprocess, sys
from pathlib import Path
root = Path(__file__).resolve().parents[2]
if os.environ.get('TEST_ADVANCE_HEAD') == '1':
    (root / 'packaging/iso/committed-profile').write_text('advanced-head')
    subprocess.run(['git', '-C', str(root), 'add', '.'], check=True)
    subprocess.run(['git', '-C', str(root), '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid',
                    '-c', 'commit.gpgsign=false', '-c', 'core.hooksPath=/dev/null', 'commit', '-qm', 'advanced'], check=True)
commit = subprocess.check_output(['git', '-C', str(root), 'rev-parse', 'HEAD'], text=True).strip()
out = Path(sys.argv[2])
out.mkdir()
(out / 'packages.json').write_text(json.dumps({'commit': commit}))
(out / 'cupertino-1-x86_64.pkg.tar.zst').write_text('runtime')
(out / 'cupertino-data-1-any.pkg.tar.zst').write_text('data')
""")
        builder.chmod(0o755)
        docker = self.bin / 'docker'
        docker.write_text(DOCKER)
        docker.chmod(0o755)
        df = self.bin / 'df'
        df.write_text('#!/bin/sh\nprintf "Avail\\n999999999999\\n"\n')
        df.chmod(0o755)
        self.git('init', '-q')
        self.git('add', '.')
        self.git('-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '-qm', 'fixture')
        self.head = self.git('rev-parse', 'HEAD').strip()
        self.packages = self.base / 'packages'
        self.packages.mkdir()
        (self.packages / 'packages.json').write_text(json.dumps({'commit': self.head}))
        (self.packages / 'cupertino-1-x86_64.pkg.tar.zst').write_text('runtime')
        (self.packages / 'cupertino-data-1-any.pkg.tar.zst').write_text('data')
        self.key = self.base / 'public.asc'
        self.key.write_text('public-key')
        self.out = self.base / 'out'
        self.env = {**os.environ, 'HOME': str(self.base / 'home'), 'PATH': str(self.bin) + ':' + os.environ['PATH'],
                    'CUPERTINO_KEYRING_KEY': str(self.key), 'TEST_STATE': str(self.state)}
        for key in ('TEST_STALL', 'TEST_START_STATUS', 'TEST_RM_FAILURE', 'TEST_CLEANUP_FAILURE', 'TEST_MUTATE_SOURCE', 'TEST_ADVANCE_HEAD'):
            self.env.pop(key, None)

    def git(self, *args):
        return subprocess.check_output(['git', '-c', 'commit.gpgsign=false', '-c', 'core.hooksPath=/dev/null',
                                        '-C', str(self.root), *args], text=True, stderr=subprocess.PIPE)

    def run_iso(self, args, extra):
        return subprocess.run(['bash', str(self.root / 'scripts/build-iso.sh'), *args], env={**self.env, **extra},
                              capture_output=True, text=True, timeout=5)

    def events(self):
        path = self.state / 'events'
        return [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []

    def test_packages_without_output_uses_default_and_freezes_source_inputs(self):
        done = self.run_iso(['--packages', str(self.packages)], {'TEST_MUTATE_SOURCE': '1'})
        self.assertEqual(done.returncode, 0, done.stderr)
        output = self.base / 'home/.cache/cupertino-iso'
        self.assertEqual(len(list(output.glob('*.iso'))), 1)
        self.assertEqual((self.state / 'verified-commit').read_text(), self.head)
        self.assertIn('/source/packaging/keyring/release-key.fpr', (self.state / 'checked-fingerprint').read_text())
        self.assertEqual((self.root / 'packaging/iso/committed-profile').read_text(), 'changed-during-packaging')
        self.assertEqual(list(output.glob('work.*')), [])

    def test_new_packages_are_verified_against_the_captured_commit(self):
        done = self.run_iso([str(self.out)], {})
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertEqual((self.state / 'verified-commit').read_text(), self.head)
        self.assertEqual(len(list(self.out.glob('*.iso'))), 1)

    def test_head_advancing_during_package_build_cannot_change_iso_provenance(self):
        done = self.run_iso([str(self.out)], {'TEST_ADVANCE_HEAD': '1'})
        self.assertNotEqual(done.returncode, 0)
        self.assertNotEqual(self.git('rev-parse', 'HEAD').strip(), self.head)
        self.assertEqual(list(self.out.glob('*.iso')), [])
        self.assertEqual(list(self.out.glob('work.*')), [])
        self.assertEqual(self.events(), [])

    def test_missing_key_and_malformed_options_do_not_create_builds_or_call_docker(self):
        for args in (['--packages'], ['--packages', str(self.packages), '--packages', str(self.packages)], ['one', 'two']):
            done = self.run_iso(args, {})
            self.assertEqual(done.returncode, 2, done.stderr)
        done = self.run_iso([str(self.out), '--packages', str(self.packages)], {'CUPERTINO_KEYRING_KEY': str(self.base / 'missing')})
        self.assertEqual(done.returncode, 1)
        self.assertIn('set CUPERTINO_KEYRING_KEY', done.stderr)
        self.assertFalse(self.out.exists())
        self.assertEqual(self.events(), [])

    def test_cleanup_attempts_every_stage_and_preserves_original_build_failure(self):
        done = self.run_iso([str(self.out), '--packages', str(self.packages)],
                            {'TEST_START_STATUS': '17', 'TEST_RM_FAILURE': 'fixture-iso', 'TEST_CLEANUP_FAILURE': '1'})
        self.assertEqual(done.returncode, 17, done.stderr)
        self.assertIn('fixture Docker removal failure', done.stderr)
        self.assertIn('fixture Docker filesystem cleanup failure', done.stderr)
        self.assertEqual(list(self.out.glob('work.*')), [])
        self.assertTrue(any(event[0] == 'run' for event in self.events()))

    def test_cleanup_failure_after_success_is_a_failure(self):
        done = self.run_iso([str(self.out), '--packages', str(self.packages)], {'TEST_RM_FAILURE': 'fixture-iso'})
        self.assertEqual(done.returncode, 1, done.stderr)
        self.assertIn('fixture Docker removal failure', done.stderr)
        self.assertEqual(list(self.out.glob('work.*')), [])

    def test_term_removes_started_iso_and_keyring_containers_before_returning(self):
        for kind in ('iso', 'keyring'):
            with self.subTest(kind=kind):
                ready = self.state / 'ready'
                ready.unlink(missing_ok=True)
                env = {**self.env, 'TEST_STALL': kind}
                process = subprocess.Popen(['bash', str(self.root / 'scripts/build-iso.sh'), str(self.out),
                                            '--packages', str(self.packages)], env=env, start_new_session=True,
                                           stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
                try:
                    deadline = time.monotonic() + 5
                    while not ready.exists() and process.poll() is None and time.monotonic() < deadline:
                        time.sleep(0.01)
                    self.assertTrue(ready.exists(), 'container did not start')
                    process.send_signal(signal.SIGTERM)
                    _, stderr = process.communicate(timeout=5)
                    self.assertEqual(process.returncode, 143, stderr)
                    self.assertTrue((self.state / ('stopped-fixture-' + kind)).exists())
                    self.assertEqual(list(self.out.glob('work.*')), [])
                finally:
                    with contextlib.suppress(ProcessLookupError):
                        os.killpg(process.pid, signal.SIGKILL)
                    process.communicate(timeout=5)


if __name__ == '__main__':
    unittest.main()
