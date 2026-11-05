#!/usr/bin/env python3
"""Build integration fixtures once, then run a test command with their exact Cargo artifacts.

Usage: python3 ci/build-test-fixtures.py --locked -- cargo nextest run --locked --workspace --all-features
Pass --release, --target and --target-dir to both commands when needed. For a custom Cargo profile, pass
--profile NAME here and --cargo-profile NAME to nextest (--profile NAME to cargo test).
The wrapper always builds the workspace with all features, including test targets so dev-dependency features match.
Fixture paths are valid only while the child command runs; tests never invoke Cargo to prepare their own fixtures.
"""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile


PREFIX = 'CUPERTINO_TEST_FIXTURE_'
ROOT = Path(__file__).resolve().parent.parent


def artifact_key(package: str, kind: str, name: str) -> str:
    return PREFIX + '_'.join((package, kind, name)).upper().replace('-', '_')


def collect_artifacts(messages: list[dict]) -> dict[str, Path]:
    artifacts = {}
    for message in messages:
        if message.get('reason') != 'compiler-artifact' or message['profile']['test']:
            continue
        target = message['target']
        # Cargo's package id carries a source URL; the manifest identifies workspace packages unambiguously.
        manifest = Path(message['manifest_path'])
        if manifest.parent.parent != ROOT / 'crates':
            continue
        package = manifest.parent.name
        if message['executable'] and any(kind in target['kind'] for kind in ('bin', 'example')):
            kind = 'example' if 'example' in target['kind'] else 'bin'
            artifacts[artifact_key(package, kind, target['name'])] = Path(message['executable'])
        if 'cdylib' in target['crate_types']:
            libraries = [Path(path) for path in message['filenames'] if path.endswith('.so')]
            if len(libraries) != 1:
                raise ValueError(f'expected one cdylib artifact for {package}: {libraries}')
            artifacts[artifact_key(package, 'cdylib', target['name'])] = libraries[0]
    return artifacts


def fixture_environment(artifacts: dict[str, Path], session: Path) -> dict[str, str]:
    env = {key: value for key, value in os.environ.items() if not key.startswith(PREFIX)}
    env[PREFIX + 'SESSION'] = str(session)
    for key, path in artifacts.items():
        path = path.resolve(strict=True)
        stamp = path.stat()
        env[key] = str(path)
        env[key + '_STAMP'] = f'{stamp.st_size}:{stamp.st_mtime_ns}'
    return env


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument('--locked', action='store_true')
    mode.add_argument('--frozen', action='store_true')
    profile = parser.add_mutually_exclusive_group()
    profile.add_argument('--release', action='store_true')
    profile.add_argument('--profile')
    parser.add_argument('--target')
    parser.add_argument('--target-dir')
    parser.add_argument('command', nargs=argparse.REMAINDER)
    args = parser.parse_args()
    if not args.command or args.command[0] != '--' or len(args.command) == 1:
        parser.error('provide the test command after --')
    os.environ["CARGO_INCREMENTAL"] = "0"
    build = [os.environ.get('CARGO', 'cargo'), 'build', '--workspace', '--all-features', '--all-targets',
             '--message-format=json-render-diagnostics']
    for flag in ('locked', 'frozen', 'release'):
        if getattr(args, flag):
            build.append('--' + flag)
    for flag in ('profile', 'target', 'target-dir'):
        value = getattr(args, flag.replace('-', '_'))
        if value is not None:
            build.extend(('--' + flag, value))
    # Stream diagnostics through stderr, retaining only artifact messages rather than a potentially large build log.
    messages = []
    with subprocess.Popen(build, cwd=ROOT, stdout=subprocess.PIPE, text=True) as cargo:
        for line in cargo.stdout:
            message = json.loads(line)
            if message.get('reason') == 'compiler-artifact':
                messages.append(message)
        status = cargo.wait()
    if status:
        return status
    artifacts = collect_artifacts(messages)
    if not artifacts:
        raise ValueError('Cargo produced no workspace fixture artifacts')
    with tempfile.TemporaryDirectory(prefix='cupertino-test-fixtures-') as directory:
        return subprocess.run(args.command[1:], cwd=ROOT,
                              env=fixture_environment(artifacts, Path(directory))).returncode


if __name__ == '__main__':
    sys.exit(main())
