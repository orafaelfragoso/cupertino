#!/usr/bin/env python3
"""Exercise makepkg's build/check environment without compiling Rust or starting a session bus."""
import json
import os
from pathlib import Path
import shlex
import subprocess
import sys
import tempfile
import unittest


HERE = Path(__file__).resolve().parent


class BuildEnvironment(unittest.TestCase):
    def test_build_and_check_share_flags_and_preserve_supplied_flags(self):
        for recipe in ('cupertino', 'cupertino-git'):
            for supplied in ('', '-C target-cpu=x86-64-v2', 'existing-remap'):
                with self.subTest(recipe=recipe, supplied=supplied), tempfile.TemporaryDirectory() as directory:
                    root = Path(directory)
                    source = root / 'source'
                    source.mkdir()
                    tree = source / ('cupertino-0.1.0' if recipe == 'cupertino' else 'cupertino')
                    tree.mkdir()
                    commands = root / 'bin'
                    commands.mkdir()
                    logger = '#!' + sys.executable + r'''
import json, os, sys
with open(os.environ['TEST_LOG'], 'a') as log:
    log.write(json.dumps({'command': sys.argv, 'env': {name: os.environ.get(name) for name in
              ('RUSTFLAGS', 'CARGO_INCREMENTAL', 'CARGO_PROFILE_RELEASE_DEBUG', 'CUPERTINO_THEME_DIR', 'CUPERTINO_SPEC_DIR')}}) + '\n')
'''
                    for command in ('cargo', 'python3'):
                        stub = commands / command
                        stub.write_text(logger)
                        stub.chmod(0o700)
                    bus = commands / 'dbus-run-session'
                    bus.write_text('#!/bin/sh\n[ "$1" = -- ] || exit 2\nshift\nexec "$@"\n')
                    bus.chmod(0o700)
                    name = 'cupertino' if recipe == 'cupertino' else 'cupertino-git'
                    source_remap = f'--remap-path-prefix={source}=/usr/src/debug/{name}'
                    cargo_remap = f'--remap-path-prefix={root / "cargo-home"}=/cargo'
                    flags = source_remap if supplied == 'existing-remap' else supplied
                    log = root / 'commands.jsonl'
                    env = dict(os.environ, PATH=str(commands) + os.pathsep + os.environ['PATH'],
                               TEST_LOG=str(log), CARGO_HOME=str(root / 'cargo-home'), RUSTFLAGS=flags)
                    env.pop('CUPERTINO_SPEC_DIR', None)
                    shell = '''set -euo pipefail
source "$1"
srcdir=$2
cd "$srcdir"
build
cd "$srcdir"
check
cd "$srcdir"
build
cd "$srcdir"
check
'''
                    result = subprocess.run(['bash', '-c', shell, 'bash', str(HERE / recipe / 'PKGBUILD'), str(source)],
                                            env=env, text=True, capture_output=True)
                    self.assertEqual(result.returncode, 0, result.stderr)
                    records = [json.loads(line) for line in log.read_text().splitlines()]
                    self.assertEqual(len(records), 6)
                    expected = ' '.join([part for part in (flags, source_remap if flags != source_remap else '', cargo_remap) if part])
                    self.assertEqual({record['env']['RUSTFLAGS'] for record in records}, {expected})
                    self.assertEqual(shlex.split(expected).count(source_remap), 1)
                    self.assertEqual(shlex.split(expected).count(cargo_remap), 1)
                    for record in records:
                        self.assertEqual(record['env']['CARGO_INCREMENTAL'], '0')
                        self.assertEqual(record['env']['CARGO_PROFILE_RELEASE_DEBUG'], '0')
                        self.assertEqual(record['env']['CUPERTINO_THEME_DIR'], '/usr/share/cupertino/theme')
                    self.assertIsNone(records[0]['env']['CUPERTINO_SPEC_DIR'])
                    self.assertEqual(records[1]['env']['CUPERTINO_SPEC_DIR'], str(tree / 'data/theme'))


if __name__ == '__main__':
    unittest.main()
