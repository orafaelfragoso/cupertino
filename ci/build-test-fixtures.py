#!/usr/bin/env python3
"""Build integration fixtures once, then run a test command with their exact Cargo artifacts.

Usage: python3 ci/build-test-fixtures.py --locked -- cargo nextest run --locked --workspace --all-features
The test command's own options decide its profile, target and target directory; --release, --profile, --target and
--target-dir here are optional and must agree with it (nextest takes --cargo-profile NAME, cargo test --profile NAME).
Preparation runs two builds. The first builds all features and targets of the packages named by repeated --package
flags (the workspace without them), so tests find the executables and libraries of other packages. The second is
`cargo test --no-run` with the package, feature, target and profile options of a `cargo test` or `cargo nextest run`
command: exactly what that command compiles, so it finds every artifact it builds up to date and never relinks a
fixture. Package selection here and on the test command are therefore independent.
Fixture paths are valid only while the child command runs; tests never invoke Cargo to prepare their own fixtures.
"""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
from typing import NamedTuple


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


class ChildBuild(NamedTuple):
    """What a `cargo test` or `cargo nextest run` command compiles."""
    options: list[str]  # its build options, in `cargo test` spelling
    profile: str
    target: str | None
    target_dir: str | None


# Options that decide what the test command compiles, spelled the same by cargo test and cargo nextest run.
BUILD_FLAGS = {'--workspace', '--all', '--lib', '--bins', '--examples', '--tests', '--benches', '--all-targets',
               '--all-features', '--no-default-features', '--locked', '--frozen', '--offline'}
BUILD_VALUES = {'-p': '--package', '--package': '--package', '--exclude': '--exclude', '--bin': '--bin',
                '--example': '--example', '--test': '--test', '--bench': '--bench', '-F': '--features',
                '--features': '--features', '--target': '--target', '--target-dir': '--target-dir'}
# Options that would build from another manifest, configuration or toolchain than the preparation sees.
REFUSED = {'--manifest-path', '--config', '-Z', '--unit-graph', '--doc'}
LOCK_FLAGS = ('--locked', '--frozen', '--offline')


def child_build(command: list[str]) -> ChildBuild | None:
    """The build of a `cargo test` or `cargo nextest run` command; None for a command that is not Cargo."""
    if Path(command[0]).name != 'cargo':
        return None
    arguments = command[1:]
    options = []
    while arguments and arguments[0].startswith('-'):
        option = arguments.pop(0)
        if option in LOCK_FLAGS:
            options.append(option)
        elif option == '--color':
            arguments = arguments[1:]
        elif not (option.startswith('--color=') or option in ('-q', '--quiet', '--verbose') or
                  option.strip('v') == '-'):
            raise ValueError(f'cannot prepare fixtures for Cargo option {option} before the subcommand')
    if arguments[:2] in (['nextest', 'run'], ['nextest', 'r']):
        arguments, profile_option = arguments[2:], '--cargo-profile'
    elif arguments[:1] == ['test']:
        arguments, profile_option = arguments[1:], '--profile'
    else:
        raise ValueError(f'cannot prepare fixtures for {" ".join(command[:3])}: use cargo test or cargo nextest run')
    profile, target, target_dir = 'dev', None, None
    index = 0
    # Options after -- go to the test binaries. An option this does not know cannot change the build; its value, if
    # any, is not an option name either.
    while index < len(arguments) and arguments[index] != '--':
        argument = arguments[index]
        name, value = argument, None
        if argument.startswith('--') and '=' in argument:
            name, value = argument.split('=', 1)
        elif argument[:2] in ('-p', '-F', '-Z') and len(argument) > 2:
            name, value = argument[:2], argument[2:]
        if name in REFUSED:
            raise ValueError(f'cannot prepare fixtures for a test command with {name}')
        if name in BUILD_VALUES or name == profile_option:
            if value is None:
                index += 1
                if index == len(arguments):
                    raise ValueError(f'{name} needs a value')
                value = arguments[index]
            if name == profile_option:
                profile = value
                options.extend(('--profile', value))
            else:
                options.extend((BUILD_VALUES[name], value))
                if name == '--target':
                    target = value
                elif name == '--target-dir':
                    target_dir = value
        elif name in ('-r', '--release'):
            profile = 'release'
            options.append('--release')
        elif name in BUILD_FLAGS:
            options.append(name)
        index += 1
    return ChildBuild(options, profile, target, target_dir)


def preparation(argv: list[str]) -> tuple[list[list[str]], list[str]]:
    """The preparation builds, in order, and the test command they prepare."""
    parser = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument('--locked', action='store_true')
    mode.add_argument('--frozen', action='store_true')
    profile = parser.add_mutually_exclusive_group()
    profile.add_argument('--release', action='store_true')
    profile.add_argument('--profile')
    parser.add_argument('--target')
    parser.add_argument('--target-dir')
    parser.add_argument('--package', action='append')
    parser.add_argument('command', nargs=argparse.REMAINDER)
    args = parser.parse_args(argv)
    if not args.command or args.command[0] != '--' or len(args.command) == 1:
        parser.error('provide the test command after --')
    command = args.command[1:]
    try:
        child = child_build(command)
    except ValueError as error:
        parser.error(str(error))
    wanted = {'profile': 'release' if args.release else args.profile, 'target': args.target,
              'target_dir': args.target_dir}
    if child is not None:
        for field, value in wanted.items():
            if value is not None and value != getattr(child, field):
                parser.error(f'the test command builds {field.replace("_", " ")} {getattr(child, field)}, '
                             f'not {value}: give it the same option or leave it out here')
        wanted = {field: getattr(child, field) for field in wanted}
    lock = [flag for flag in ('--locked', '--frozen') if getattr(args, flag[2:])]
    cargo = os.environ.get('CARGO', 'cargo')
    build = [cargo, 'build', '--all-features', '--all-targets', '--timings', '--message-format=json-render-diagnostics']
    if args.package is None:
        build.append('--workspace')
    else:
        for package in args.package:
            build.extend(('--package', package))
    build.extend(lock)
    if child is not None:
        build.extend(flag for flag in LOCK_FLAGS if flag in child.options and flag not in lock)
    if wanted['profile'] == 'release':
        build.append('--release')
    elif wanted['profile'] not in (None, 'dev'):
        build.extend(('--profile', wanted['profile']))
    for flag in ('target', 'target-dir'):
        value = wanted[flag.replace('-', '_')]
        if value is not None:
            build.extend(('--' + flag, value))
    builds = [build]
    if child is not None:
        # Last, so every artifact the test command builds is the one it finds up to date.
        builds.append([cargo, 'test', '--no-run', '--message-format=json-render-diagnostics', *child.options,
                       *(flag for flag in lock if flag not in child.options)])
    return builds, command


def compile_artifacts(build: list[str], inherited: tuple[int, ...]) -> tuple[int, list[dict]]:
    # Stream diagnostics through stderr, retaining only artifact messages rather than a potentially large build log.
    messages = []
    with subprocess.Popen(build, cwd=ROOT, stdout=subprocess.PIPE, text=True, pass_fds=inherited) as cargo:
        for line in cargo.stdout:
            message = json.loads(line)
            if message.get('reason') == 'compiler-artifact':
                messages.append(message)
        status = cargo.wait()
    return status, messages


def main() -> int:
    builds, command = preparation(sys.argv[1:])
    os.environ["CARGO_INCREMENTAL"] = "0"
    lease = os.environ.get('CUPERTINO_BUILD_LOCK_FD')
    inherited = () if lease is None else tuple(map(int, lease.split(',')))
    for descriptor in inherited:
        os.fstat(descriptor)
    messages = []
    for build in builds:
        status, built = compile_artifacts(build, inherited)
        if status:
            return status
        messages.extend(built)
    artifacts = collect_artifacts(messages)
    if not artifacts:
        raise ValueError('Cargo produced no workspace fixture artifacts')
    with tempfile.TemporaryDirectory(prefix='cupertino-test-fixtures-') as directory:
        return subprocess.run(command, cwd=ROOT,
                              env=fixture_environment(artifacts, Path(directory)), pass_fds=inherited).returncode


if __name__ == '__main__':
    sys.exit(main())
