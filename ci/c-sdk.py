#!/usr/bin/env python3
"""Build the C SDK into a staging root the way the cupertino-kit package installs it, then build and run C programs
against that root alone: the samples (as C11 and as C++17, with -Werror), the samples' Makefile, the struct layout
probe, `cupertino sdk info`, `cupertino sdk check` and the projects `cupertino sdk new` writes.

    python3 ci/c-sdk.py [--bindir DIR] [--no-build] [--keep DIR] [--no-cargo]

It builds cupertino-ui-abi's library and the cupertino command with the dev profile into the Cargo target directory
(CARGO_TARGET_DIR or ./target) unless --no-build, and installs the SDK with `packaging/release.py install --package sdk`.
pkg-config reads the root as a sysroot (PKG_CONFIG_SYSROOT_DIR and PKG_CONFIG_LIBDIR) and programs load the library
from it (LD_LIBRARY_PATH); no flag of a compile line may name the checkout. The Rust project of `sdk new` is checked
with `cargo check --offline` against this checkout's cupertino-kit instead of the release tag it names; --no-cargo writes
it and prints that command instead of running it."""
import argparse
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
ABI = ROOT / 'crates/cupertino-ui-abi'
SAMPLES = ABI / 'examples/c'
LAYOUT_PROBE = ABI / 'tests/c/layout.c'
LAYOUT = ABI / 'abi/layout-1.txt'
HEADER = ABI / 'abi/cupertinokit.h'
KIT = ROOT / 'crates/cupertino-kit'
COMPILE_FLAGS = ['-Wall', '-Wextra', '-Werror', '-pthread']
TIMEOUT = 120


class Failed(Exception):
    pass


def abi_version(header=HEADER):
    text = header.read_text()
    major = re.search(r'^#define CUPERTINOKIT_ABI_MAJOR (\d+)$', text, re.M)
    minor = re.search(r'^#define CUPERTINOKIT_ABI_MINOR (\d+)$', text, re.M)
    if not major or not minor:
        raise Failed(f'{header}: no CUPERTINOKIT_ABI_MAJOR and CUPERTINOKIT_ABI_MINOR')
    return int(major[1]), int(minor[1])


def cli_version():
    """The version `cupertino sdk new` writes into a project: the cupertino command's own."""
    match = re.search(r'^version = "([^"]+)"$', (ROOT / 'crates/cupertino-cli/Cargo.toml').read_text(), re.M)
    if not match:
        raise Failed('crates/cupertino-cli/Cargo.toml: no version')
    return match[1]


def target_dir():
    return Path(os.environ.get('CARGO_TARGET_DIR') or ROOT / 'target').resolve()


def sdk_environment(root, base=None):
    """The environment of a build or run against the SDK under `root` alone."""
    env = dict(os.environ if base is None else base)
    env.update(PKG_CONFIG_SYSROOT_DIR=str(root), PKG_CONFIG_LIBDIR=str(root / 'usr/lib/pkgconfig'),
               LD_LIBRARY_PATH=str(root / 'usr/lib'), LC_ALL='C')
    env.pop('PKG_CONFIG_PATH', None)
    env.pop('CPATH', None)
    env.pop('C_INCLUDE_PATH', None)
    env.pop('LIBRARY_PATH', None)
    return env


def flag_problems(flags, root, checkout=ROOT):
    """The compile flags that name a directory outside the SDK root or inside the checkout."""
    problems = []
    for flag in flags:
        for prefix in ('-I', '-L', '-isystem'):
            if flag.startswith(prefix) and len(flag) > len(prefix) and not Path(flag[len(prefix):]).is_relative_to(root):
                problems.append(f'{flag}: not a directory of the SDK root {root}')
        if str(checkout) in flag and not root.is_relative_to(checkout):
            problems.append(f'{flag}: names the checkout {checkout}')
    return sorted(set(problems))


def rust_manifest(text, version, root=ROOT):
    """The Cargo.toml of `sdk new --lang rust` with its git dependencies on this checkout's crates instead of the
    release tag; fails unless each names the tag of `version`."""
    crates = {'cupertino-kit': root / 'crates/cupertino-kit', 'xkbcommon': root / 'crates/cupertino-xkbcommon'}
    for name, path in crates.items():
        line = re.search(rf'^{name} = \{{ git = "[^"]+", tag = "v([^"]+)" \}}$', text, re.M)
        if not line:
            raise Failed(f'the Rust project does not take {name} from a release tag')
        if line[1] != version:
            raise Failed(f'the Rust project takes {name} from v{line[1]}, this release is v{version}')
        text = text[:line.start()] + f'{name} = {{ path = {json.dumps(str(path))} }}' + text[line.end():]
    return text


def run(command, *, cwd=None, env=None, check=True, timeout=TIMEOUT):
    print('$ ' + shlex.join(str(c) for c in command), flush=True)
    result = subprocess.run([str(c) for c in command], cwd=cwd, env=env, text=True, capture_output=True,
                            timeout=timeout)
    if check and result.returncode != 0:
        raise Failed(f'{shlex.join(str(c) for c in command)} exited {result.returncode}:\n{result.stdout}{result.stderr}')
    return result


def same(label, got, want):
    if got != want:
        raise Failed(f'{label}: got\n{got}\nwanted\n{want}')
    print(f'ok: {label}', flush=True)


def build(env):
    run(['cargo', 'build', '--locked', '--all-features', '-p', 'cupertino-ui-abi', '--lib'], cwd=ROOT, env=env,
        timeout=3600)
    run(['cargo', 'build', '--locked', '--all-features', '-p', 'cupertino-cli', '--bin', 'cupertino'], cwd=ROOT, env=env,
        timeout=3600)


def check_sdk(root, work, cupertino, cargo=True):
    major, minor = abi_version()
    env = sdk_environment(root)
    pkg_config = shutil.which('pkg-config') or 'pkg-config'
    same('pkg-config --modversion cupertinokit', run([pkg_config, '--modversion', 'cupertinokit'], env=env).stdout.strip(),
         f'{major}.{minor}.0')
    cflags = shlex.split(run([pkg_config, '--cflags', 'cupertinokit'], env=env).stdout)
    libs = shlex.split(run([pkg_config, '--libs', 'cupertinokit'], env=env).stdout)
    same('the flags name the SDK root alone', flag_problems(cflags + libs, root), [])
    same('the header is the checkout\'s', (root / 'usr/include/cupertinokit-1/cupertinokit.h').read_bytes(),
         HEADER.read_bytes())

    samples = work / 'samples'
    shutil.copytree(SAMPLES, samples)
    expected = (SAMPLES / 'headless.expected').read_text()
    for label, compiler in (('C11', ['cc', '-std=c11']), ('C++17', ['c++', '-x', 'c++', '-std=c++17'])):
        program = samples / f'headless-{label}'
        line = compiler + COMPILE_FLAGS + cflags + [samples / 'headless.c', '-o', program] + libs
        run(line, cwd=work, env=env)
        same(f'the headless sample built as {label} prints its trace', run([program], cwd=work, env=env).stdout, expected)

    make = work / 'make'
    shutil.copytree(SAMPLES, make)
    run(['make', '-C', make, 'CFLAGS=-O2 -Werror'], env=env)
    same('the samples\' Makefile builds them with pkg-config alone', run([make / 'headless'], env=env).stdout, expected)

    probe = work / 'layout'
    run(['cc', '-std=c11'] + COMPILE_FLAGS + cflags + [LAYOUT_PROBE, '-o', probe] + libs, env=env)
    same('the C compiler sees the struct layouts of layout-1.txt', run([probe], env=env).stdout, LAYOUT.read_text())

    info = json.loads(run([cupertino, '--format', 'json', 'sdk', 'info', '--require', '--root', root], env=env).stdout)
    same('cupertino sdk info --require: installed', (info['ok'], info['data']['installed'], info['data']['abi']),
         (True, True, f'{major}.{minor}'))
    checked = json.loads(run([cupertino, '--format', 'json', 'sdk', 'check', '--root', root], env=env).stdout)
    same('cupertino sdk check: the probe builds and runs', (checked['ok'], checked['data']['abi'],
                                                           [s['step'] for s in checked['data']['steps']]),
         (True, f'{major}.{minor}', ['compiler', 'pkg_config', 'compile', 'run']))

    projects = work / 'projects'
    projects.mkdir()
    run([cupertino, 'sdk', 'new', '--lang', 'c', 'demo'], cwd=projects, env=env)
    run(['make', '-C', 'demo', 'CFLAGS=-O2 -Werror'], cwd=projects, env=env)
    same('cupertino sdk new --lang c demo && make -C demo: the starter runs headless',
         run(['./demo/demo'], cwd=projects, env=env).stdout, 'count: 1\ncount: 2\ncount: 3\n')

    run([cupertino, 'sdk', 'new', '--lang', 'rust', 'org.example.Counter', '--dir', 'counter'], cwd=projects, env=env)
    manifest = projects / 'counter/Cargo.toml'
    manifest.write_text(rust_manifest(manifest.read_text(), cli_version()))
    shutil.copyfile(ROOT / 'Cargo.lock', projects / 'counter/Cargo.lock')
    command = ['cargo', 'check', '--offline', '--manifest-path', manifest]
    if not cargo:
        print('not run: ' + shlex.join(str(c) for c in command), flush=True)
        return
    # the checkout's target directory already holds the build scripts and macros cupertino-kit needs
    run(command, env=dict(os.environ, CARGO_TARGET_DIR=str(target_dir())), timeout=3600)
    print('ok: the Rust project of cupertino sdk new builds against this cupertino-kit', flush=True)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--bindir', type=Path, help='where libcupertinokit.so and cupertino are (default: the dev '
                        'profile\'s directory of the Cargo target directory)')
    parser.add_argument('--no-build', action='store_true', help='use what --bindir holds without building')
    parser.add_argument('--keep', type=Path, help='stage the SDK and build the programs in this empty directory, and '
                        'keep it')
    parser.add_argument('--no-cargo', action='store_true', help='write the Rust project of sdk new, and print the cargo '
                        'command that checks it instead of running it')
    args = parser.parse_args(argv)
    bindir = (args.bindir or target_dir() / 'debug').resolve()
    try:
        if not args.no_build:
            build(os.environ)
        for name in ('libcupertinokit.so', 'cupertino'):
            if not (bindir / name).is_file():
                raise Failed(f'{bindir / name} is missing: build it or give --bindir')
        with tempfile.TemporaryDirectory(prefix='cupertino-c-sdk-') as temporary:
            work = Path(temporary) if args.keep is None else args.keep.resolve()
            work.mkdir(parents=True, exist_ok=True)
            if any(work.iterdir()):
                raise Failed(f'{work} is not empty')
            root = work / 'root'
            run([sys.executable, '-I', ROOT / 'packaging/release.py', 'install', '--package', 'sdk', '--destdir', root,
                 '--bindir', bindir])
            check_sdk(root, work, bindir / 'cupertino', cargo=not args.no_cargo)
    except (Failed, subprocess.TimeoutExpired) as error:
        print(f'c-sdk: {error}', file=sys.stderr)
        return 1
    print('c-sdk: the C SDK builds and runs from its staging root alone')
    return 0


if __name__ == '__main__':
    sys.exit(main())
