#!/usr/bin/env python3
"""Exercise makepkg's build/check environment without compiling Rust or starting a session bus."""
from itertools import product
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
            for supplied, target_mode in product(('', '-C target-cpu=x86-64-v2', 'existing-remap'),
                                                 ('default', 'relative', 'absolute')):
                with self.subTest(recipe=recipe, supplied=supplied, target=target_mode), tempfile.TemporaryDirectory() as directory:
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
                    # the workspace build leaves a stand-in cupertino, which build() runs for the pages and completions
                    stand_in = logger + r'''
if sys.argv[1:2] == ['build'] and '--bins' in sys.argv:
    release = os.path.join(os.environ.get('CARGO_TARGET_DIR') or 'target', 'release')
    os.makedirs(release, exist_ok=True)
    cli = os.path.join(release, 'cupertino')
    with open(cli, 'w') as f:
        f.write('#!/bin/sh\ncase "$1" in\n  dev) mkdir -p "$3" && echo page > "$3/cupertino.1" ;;\n'
                '  completions) echo "# $2" ;;\n  *) exit 2 ;;\nesac\n')
    os.chmod(cli, 0o700)
'''
                    for command, text in (('cargo', stand_in), ('python3', logger)):
                        stub = commands / command
                        stub.write_text(text)
                        stub.chmod(0o700)
                    bus = commands / 'dbus-run-session'
                    bus.write_text('#!/bin/sh\n[ "$1" = -- ] || exit 2\nshift\nexec "$@"\n')
                    bus.chmod(0o700)
                    name = 'cupertino' if recipe == 'cupertino' else 'cupertino-git'
                    source_remap = f'--remap-path-prefix={source}=/usr/src/debug/{name}'
                    cargo_remap = f'--remap-path-prefix={root / "cargo-home"}=/cargo'
                    target = {'default': None, 'relative': '../cargo-output',
                              'absolute': str(root / 'cargo-output')}[target_mode]
                    target_dir = (tree / (target if target is not None else 'target')).resolve()
                    target_remap = f'--remap-path-prefix={target_dir}=/usr/src/debug/{name}/target'
                    flags = source_remap if supplied == 'existing-remap' else supplied
                    log = root / 'commands.jsonl'
                    env = dict(os.environ, PATH=str(commands) + os.pathsep + os.environ['PATH'],
                               TEST_LOG=str(log), CARGO_HOME=str(root / 'cargo-home'), RUSTFLAGS=flags)
                    env.pop('CUPERTINO_SPEC_DIR', None)
                    env.pop('CARGO_TARGET_DIR', None)
                    if target is not None:
                        env['CARGO_TARGET_DIR'] = target
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
                    # what the built cupertino wrote, for package_cupertino()'s --gendir
                    self.assertEqual((source / 'gen/MANIFEST').read_text(),
                                     'completions/bash\ncompletions/fish\ncompletions/zsh\nman/cupertino.1\n')
                    self.assertEqual((source / 'gen/completions/zsh').read_text(), '# zsh\n')
                    records = [json.loads(line) for line in log.read_text().splitlines()]
                    # each pass: the workspace's binaries, the SDK's library, the tests and the doctests
                    self.assertEqual(len(records), 8)
                    self.assertEqual(records[1]['command'][1:], ['build', '--frozen', '--release', '-p', 'cupertino-ui-abi',
                                                                 '--lib', '--all-features'])
                    expected = ' '.join([part for part in (flags, source_remap if flags != source_remap else '', cargo_remap, target_remap) if part])
                    self.assertEqual({record['env']['RUSTFLAGS'] for record in records}, {expected})
                    self.assertEqual(shlex.split(expected).count(source_remap), 1)
                    self.assertEqual(shlex.split(expected).count(cargo_remap), 1)
                    self.assertEqual(shlex.split(expected).count(target_remap), 1)
                    for record in records:
                        self.assertEqual(record['env']['CARGO_INCREMENTAL'], '0')
                        self.assertEqual(record['env']['CARGO_PROFILE_RELEASE_DEBUG'], '0')
                        self.assertEqual(record['env']['CUPERTINO_THEME_DIR'], '/usr/share/cupertino/theme')
                    self.assertIsNone(records[0]['env']['CUPERTINO_SPEC_DIR'])
                    self.assertIsNone(records[1]['env']['CUPERTINO_SPEC_DIR'])
                    self.assertEqual(records[2]['env']['CUPERTINO_SPEC_DIR'], str(tree / 'data/theme'))


if __name__ == '__main__':
    unittest.main()
