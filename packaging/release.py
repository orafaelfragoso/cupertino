#!/usr/bin/env python3
"""Cupertino release tool: the one installation manifest behind the Arch packages.

Usage:
  release.py install [--format arch] --package {runtime,data,sdk} --destdir DIR [--bindir DIR] [--gendir DIR]
  release.py verify [--format arch] --package {runtime,data,sdk} [--bindir DIR | --gendir DIR] ARTIFACT
  release.py check-metadata
  release.py third-party
  release.py source --output DIR
  release.py verify-tag TAG
  release.py pkgbuild ARCHIVE OUTPUT

install         copies exactly the manifest's files into DIR with explicit modes (Arch package(), portable tarballs):
                built binaries and libraries from --bindir, the files the built `cupertino` generated (manual pages,
                completions) from --gendir. It makes the manifest's relative symbolic links (the library's soname
                links) and writes the files it generates itself (each package's build record, the pkg-config file).
verify          lists a built package (.pkg.tar.zst) and fails unless its files, links and modes equal the manifest plus
                pacman's own metadata files. It also rejects setuid/setgid bits, wallpapers whose hash differs
                from packaging/data/wallpapers.json, and executables and generated texts that hold a home-directory or
                checkout path. For the runtime package, --bindir generates the pages and completions again with the
                built `cupertino` and compares them (a page's date aside); --gendir takes the names from its MANIFEST.
                For the SDK it reads the library with readelf: its exports must equal the ABI's symbol list, with no
                version definitions, the soname libcupertinokit.so.<major> and NODELETE; the header and every other
                copied file must equal the checkout's.
check-metadata  fails when THIRD-PARTY-LICENSES is stale, a shipped licence text is missing or a PKGBUILD licence list
                misses a licence the package ships.
third-party     regenerates packaging/licenses/THIRD-PARTY-LICENSES with cargo-about (0.9.3, on PATH or $CARGO_ABOUT): the
                licence texts and NOTICE files of every registry crate in the normal and build closure of the shipped
                binaries and libraries.
source          writes cupertino-<version>-source.tar.xz from the committed HEAD (git archive: file times are the
                commit time, no untracked files).
verify-tag      fails unless TAG is v<version>, points at HEAD, and HEAD is reachable from origin/main.
pkgbuild        writes the stable Arch PKGBUILD with ARCHIVE's sha256 (the release's source archive).

The runtime version is cupertino-server's, the data version cupertino-assets'; the SDK package (cupertino-kit) shares
the runtime's, and its library is named by the C ABI version of its header. Nothing here reads outside the checkout.
"""
import argparse
import os
import hashlib
import html
import io
import lzma
import re
import shutil
import stat
import subprocess
import sys
import tarfile
import tempfile
import tomllib
import json
import textwrap
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
# Arch is the only package format; --format stays so a recipe can name it.
FORMATS = ('arch',)
# The applications among the shipped binaries: each has a desktop entry crates/<app>/data/applications/<app>.desktop.
APPS = ('cupertino-files', 'cupertino-settings')
BINARIES = (
    'cupertino-server',
    'cupertino-dock', 'cupertino-menubar', 'cupertino-launcher', 'cupertino-notifications', 'cupertino-controlcenter',
    'cupertino-osd', 'cupertino-screenshot', 'cupertino-control', 'cupertino-windowmanager', 'cupertino-login', 'cupertino-polkit-agent',
    'cupertino-keyring-prompter', 'cupertino-portal',
    *APPS,
    'cupertino-sessiond', 'cupertino-netd', 'cupertino-btd', 'cupertino-powerd',
    'cupertino-diagnose',
    'cupertino',
)
# The crate of a shipped binary whose name is not its crate's; the licence closure starts at the crates.
BINARY_CRATES = {'cupertino': 'cupertino-cli', 'cupertino-keyring-prompter': 'cupertino-polkit-agent'}
# The completions the built `cupertino` writes into <gendir>/completions/<shell>, and where each shell finds them.
COMPLETIONS = (('bash', 'usr/share/bash-completion/completions/cupertino'),
               ('zsh', 'usr/share/zsh/site-functions/_cupertino'),
               ('fish', 'usr/share/fish/vendor_completions.d/cupertino.fish'))
# Besides an app's own entry, the only file an app crate may ship there: the session's default applications.
MIMEAPPS = 'cupertino-mimeapps.list'
PAM_SERVICES = ('cupertino-login', 'cupertino-session', 'cupertino-greeter', 'cupertino-autologin')
UNITS = ('cupertino-login', 'cupertino-netd', 'cupertino-btd', 'cupertino-powerd')
THEME_FILES = ('tokens.json', 'materials.json', 'animations.json')
WALLPAPERS = json.loads((ROOT / 'packaging/data/wallpapers.json').read_text())['files']
# The workspace crates whose shared libraries ship (the SDK), beside BINARIES: the other roots of the licence closure.
LIBRARIES = ('cupertino-ui-abi',)
# CupertinoKit's C ABI: the header names its version, the symbol list its exports.
ABI_DIR = 'crates/cupertino-ui-abi'
ABI_HEADER = f'{ABI_DIR}/abi/cupertinokit.h'
ABI_SYMBOLS = f'{ABI_DIR}/abi/symbols-1.txt'
# The file cargo builds the library as, in --bindir.
ABI_ARTIFACT = 'libcupertinokit.so'
# Licence texts of the crates linked into the binaries, beside the GPL text.
LICENSE_TEXTS = (('MPL-2.0', 'crates/cupertino-geometry/LICENSE'), ('MIT', 'crates/cupertino-xkbcommon/LICENSE'),
                 ('libxkbcommon', 'third-party/xkbcommon-rs/LICENSE'), ('smithay', 'third-party/smithay/LICENSE.txt'),
                 ('OFL.txt', 'crates/cupertino-assets/fonts/OFL.txt'))
# Licence texts of the crates linked into the SDK's library, by the SPDX identifier their crates declare (OFL.txt for
# OFL-1.1) or, for a path crate with a licence file and no identifier, by that file.
SDK_LICENSE_TEXTS = (('MPL-2.0', f'{ABI_DIR}/LICENSE'), ('MIT', 'crates/cupertino-xkbcommon/LICENSE'),
                     ('libxkbcommon', 'third-party/xkbcommon-rs/LICENSE'), ('OFL.txt', 'crates/cupertino-assets/fonts/OFL.txt'))
# Registry crates that carry licensed input without a licence file: the build script of smithay compiles bindings
# generated from these files, whose leading comment holds the Khronos licence and copyright notice.
EMBEDDED_NOTICES = {'khronos_api': ('api_egl/api/egl.xml', 'api/xml/gl.xml')}
# wayland-scanner compiles the interface tables of the protocol XML files that these crates name in their sources
# (every `"....xml"` literal there is an input of wayland_protocol!, generate_interfaces! or generate_client_code!);
# each file's <copyright> holds the licence notice.
SCANNER_CRATES = ('wayland-client', 'wayland-server', 'wayland-protocols', 'wayland-protocols-wlr',
                  'wayland-protocols-plasma', 'wayland-protocols-misc')
THIRD_PARTY = 'packaging/licenses/THIRD-PARTY-LICENSES'
ABOUT_CONFIG = 'packaging/licenses/about.toml'
# What the data package ships besides crate code: the pack's own files, the font and the photographs.
DATA_LICENSES = {'GPL-3.0-or-later', 'OFL-1.1', 'LicenseRef-Unsplash'}


def applications(root, apps):
    """The files of crates/*/data/applications to install, or ValueError listing every problem: a shipped app without
    its <app>.desktop, any other file there (only an app crate's <app>.desktop and cupertino-mimeapps.list belong),
    or one file name from two crates."""
    problems, owner = [], {}
    found = sorted(root.glob('crates/*/data/applications/*'))
    for f in found:
        crate = f.parent.parent.parent.name
        rel = f.relative_to(root).as_posix()
        if f.name in owner:
            problems.append(f'{rel}: {f.name} also comes from crates/{owner[f.name]}')
        owner.setdefault(f.name, crate)
        if not (f.is_file() and crate in apps and f.name in (f'{crate}.desktop', MIMEAPPS)):
            problems.append(f'{rel}: not a desktop entry of a shipped app ({", ".join(apps)}) or {MIMEAPPS}')
    for app in apps:
        if not (root / 'crates' / app / 'data/applications' / f'{app}.desktop').is_file():
            problems.append(f'crates/{app}/data/applications/{app}.desktop: missing for the shipped app {app}')
    if problems:
        raise ValueError('\n  '.join(problems))
    return found


class Generated(str):
    """A source relative to --gendir: a file the built `cupertino` wrote at build time."""


class BuildRecord(str):
    """A source `install` writes itself: the build record of the package it names, read by `cupertino version`."""


def binary_crates():
    """The crates the shipped binaries are built from."""
    return {BINARY_CRATES.get(b, b) for b in BINARIES}


def generated(gendir):
    """(destination, Generated source, mode, config) of what the built `cupertino` wrote into gendir: the manual pages
    man/*.1 (cupertino.1 and one per command group, as listed there: the groups are the command's to name) and
    completions/<shell>."""
    pages = sorted(p.name for p in (Path(gendir) / 'man').glob('*.1'))
    if 'cupertino.1' not in pages:
        fail(f'{gendir}/man has no cupertino.1: run `cupertino dev manual {gendir}/man` after the build')
    files = [(f'usr/share/man/man1/{p}', Generated(f'man/{p}'), 0o644, False) for p in pages]
    files += [(dest, Generated(f'completions/{shell}'), 0o644, False) for shell, dest in COMPLETIONS]
    return files


def runtime(gendir=None):
    """(destination, source, mode, config) of `cupertino`: a source is relative to the checkout, None for a built
    binary, a Generated file under gendir (left out without one) or a BuildRecord."""
    f = 'packaging/files'
    files = [(f'usr/bin/{b}', None, 0o755, False) for b in BINARIES]
    files += generated(gendir) if gendir is not None else []
    files.append(('usr/share/cupertino/build-info.json', BuildRecord('cupertino'), 0o644, False))
    files += [(p, f'{f}/{p}', mode, conf) for p, mode, conf in (
        ('usr/bin/cupertino-session', 0o755, False),
        ('usr/lib/cupertino/greeter', 0o755, False),
        ('usr/share/wayland-sessions/cupertino.desktop', 0o644, False),
        ('usr/lib/systemd/user/cupertino-session.target', 0o644, False),
        ('usr/lib/sysusers.d/cupertino-greeter.conf', 0o644, False),
        ('usr/share/dbus-1/system.d/org.cupertino.conf', 0o644, False),
        ('usr/share/xdg-desktop-portal/cupertino-portals.conf', 0o644, False),
        ('etc/cupertino/login.conf', 0o644, True),
        ('etc/cupertino/greeter.json', 0o644, True),
        ('etc/cupertino/compositor.json', 0o644, True),
    )]
    # The shipped apps' desktop entries (the menu bar and the dock take an app's name and icon from it) and
    # cupertino-mimeapps.list; applications() refuses anything else.
    try:
        apps = applications(ROOT, APPS)
    except ValueError as e:
        fail(f'crates/*/data/applications:\n  {e}')
    files.append(('usr/share/xdg-desktop-portal/portals/cupertino.portal', 'crates/cupertino-portal/data/cupertino.portal', 0o644,
                  False))
    files += [(f'usr/share/applications/{a.name}', a.relative_to(ROOT).as_posix(), 0o644, False) for a in apps]
    files += [(f'usr/lib/systemd/system/{u}.service', f'{f}/usr/lib/systemd/system/{u}.service', 0o644, False)
              for u in UNITS]
    files += [(f'usr/share/cupertino/theme/{n}', f'data/theme/{n}', 0o644, False) for n in THEME_FILES]
    files += [(f'etc/pam.d/{s}', f'packaging/pam/arch/{s}', 0o644, True) for s in PAM_SERVICES]
    files += [('usr/share/licenses/cupertino/LICENSE', 'LICENSE', 0o644, False),
              *((f'usr/share/licenses/cupertino/{n}', src, 0o644, False) for n, src in LICENSE_TEXTS),
              ('usr/share/licenses/cupertino/THIRD-PARTY-LICENSES', THIRD_PARTY, 0o644, False)]
    return files


def data(gendir=None):
    """`cupertino-data`: the default asset pack, its build record and its per-component licences."""
    a, d, fonts = 'usr/share/cupertino/assets', 'packaging/data', 'crates/cupertino-assets/fonts'
    files = [(f'{a}/{p}', f'{d}/{p}', 0o644, False) for p in ('manifest.json', 'icon-map.toml', 'wallpapers/credits.txt')]
    files.append((f'{a}/build-info.json', BuildRecord('cupertino-data'), 0o644, False))
    files += [(f'{a}/fonts/{n}', f'{fonts}/{n}', 0o644, False) for n in ('InterVariable-subset.ttf', 'OFL.txt')]
    files += [(f'{a}/wallpapers/{w["file"]}', f'{d}/wallpapers/{w["file"]}', 0o644, False) for w in WALLPAPERS]
    original = ROOT / 'data/assets/cupertino'
    files += [(f'{a}/cupertino/{p.relative_to(original).as_posix()}', p.relative_to(ROOT).as_posix(), 0o644, False)
              for p in sorted(original.rglob('*')) if p.is_file() and p.suffix != '.py']
    lic = 'usr/share/licenses/cupertino-data'
    files += [(f'{lic}/DATA', 'packaging/licenses/DATA', 0o644, False),
              (f'{lic}/Unsplash.txt', 'packaging/licenses/Unsplash.txt', 0o644, False),
              (f'{lic}/OFL.txt', f'{fonts}/OFL.txt', 0o644, False)]
    return files


class Link(str):
    """A symbolic link: the string is its target, relative to the link's directory."""


class Built(str):
    """A file of --bindir with this name, installed under another one."""


class PkgConfig(str):
    """A pkg-config template of the checkout, installed with its @VERSION@ filled in."""


def abi_version(header=None):
    """(major, minor) of the C ABI, from CUPERTINOKIT_ABI_MAJOR and CUPERTINOKIT_ABI_MINOR in the header's text."""
    text = (ROOT / ABI_HEADER).read_text() if header is None else header
    found = [re.search(rf'^#define CUPERTINOKIT_ABI_{part} (\d+)$', text, re.M) for part in ('MAJOR', 'MINOR')]
    if not all(found):
        fail(f'{ABI_HEADER} defines no CUPERTINOKIT_ABI_MAJOR and CUPERTINOKIT_ABI_MINOR')
    return int(found[0][1]), int(found[1][1])


def soname():
    return f'libcupertinokit.so.{abi_version()[0]}'


def pkg_config_text(template):
    """The pkg-config file of a template: Version is the ABI's, <major>.<minor>.0, as the library's file name."""
    major, minor = abi_version()
    text = template.replace('@VERSION@', f'{major}.{minor}.0')
    if '@' in text:
        fail(f'{ABI_DIR}/cupertinokit.pc.in holds a placeholder other than @VERSION@')
    return text


def sdk(gendir=None):
    """`cupertino-kit`: CupertinoKit's C library with its soname links, header, pkg-config file and build record. The
    library is libcupertinokit.so.<major>.<minor>.0, which libcupertinokit.so.<major> (the soname) and the link-time
    name libcupertinokit.so point to; the header lives in an include directory of its major version."""
    major, minor = abi_version()
    real, lic = f'libcupertinokit.so.{major}.{minor}.0', 'usr/share/licenses/cupertino-kit'
    return [(f'usr/lib/{real}', Built(ABI_ARTIFACT), 0o755, False),
            (f'usr/lib/{soname()}', Link(real), 0o777, False),
            (f'usr/lib/{ABI_ARTIFACT}', Link(soname()), 0o777, False),
            (f'usr/include/cupertinokit-{major}/cupertinokit.h', ABI_HEADER, 0o644, False),
            ('usr/lib/pkgconfig/cupertinokit.pc', PkgConfig(f'{ABI_DIR}/cupertinokit.pc.in'), 0o644, False),
            ('usr/share/cupertino-kit/build-info.json', BuildRecord('cupertino-kit'), 0o644, False),
            *((f'{lic}/{n}', src, 0o644, False) for n, src in SDK_LICENSE_TEXTS),
            (f'{lic}/THIRD-PARTY-LICENSES', THIRD_PARTY, 0o644, False)]


MANIFESTS = {'runtime': runtime, 'data': data, 'sdk': sdk}
# Files makepkg adds itself; anything else that is not in the manifest fails verification.
EXTRA = {'.PKGINFO', '.BUILDINFO', '.MTREE'}


def fail(msg):
    sys.exit(f'release.py: {msg}')


def version(crate):
    return tomllib.loads((ROOT / 'crates' / crate / 'Cargo.toml').read_text())['package']['version']


# The version each package's build record names: the runtime is versioned as the server, the data as the assets, and
# the SDK, a split package of the runtime's recipe, as the runtime.
RECORD_CRATES = {'cupertino': 'cupertino-server', 'cupertino-data': 'cupertino-assets', 'cupertino-kit': 'cupertino-server'}
COMMIT_VARIABLE, ARCHIVE_COMMIT = 'CUPERTINO_BUILD_COMMIT', 'data/build-commit'
RECORD_FIELDS = {'package', 'version', 'commit', 'source_date_epoch'}


def full_commit(text):
    """A full commit hash in lower case (40 or 64 hexadecimal digits), else None."""
    text = (text or '').strip()
    return text.lower() if len(text) in (40, 64) and re.fullmatch(r'[0-9a-fA-F]+', text) else None


def build_commit(root=ROOT, environ=os.environ):
    """The commit the sources come from, in the order the build-info crate uses: CUPERTINO_BUILD_COMMIT (which must
    be a full hash), the data/build-commit a source archive carries, the HEAD of a checkout, else None."""
    if COMMIT_VARIABLE in environ:
        commit = full_commit(environ[COMMIT_VARIABLE])
        if commit is None:
            fail(f'{COMMIT_VARIABLE} is {environ[COMMIT_VARIABLE]!r}, not a full commit hash')
        return commit
    archive = root / ARCHIVE_COMMIT
    commit = full_commit(archive.read_text()) if archive.is_file() else None
    if commit is None and (root / '.git').exists():
        head = subprocess.run(['git', '-C', str(root), 'rev-parse', 'HEAD'], capture_output=True, text=True)
        commit = full_commit(head.stdout) if head.returncode == 0 else None
    return commit


def build_record(package, root=ROOT, environ=os.environ):
    """The text of a package's build record: {package, version, commit, source_date_epoch}."""
    epoch = environ.get('SOURCE_DATE_EPOCH', '').strip()
    record = {'package': package, 'version': version(RECORD_CRATES[package]), 'commit': build_commit(root, environ),
              'source_date_epoch': int(epoch) if epoch.isdigit() else None}
    return json.dumps(record, indent=2) + '\n'


def cmd_install(args):
    dest, bindir = Path(args.destdir), Path(args.bindir)
    gendir = Path(args.gendir) if args.gendir else None
    if args.package == 'runtime' and gendir is None:
        fail('the runtime package needs --gendir: the manual pages and completions the built cupertino wrote')
    for rel, src, mode, _conf in MANIFESTS[args.package](gendir):
        out = dest / rel
        out.parent.mkdir(parents=True, exist_ok=True)
        if isinstance(src, Link):
            if out.is_symlink() or out.exists():
                out.unlink()
            out.symlink_to(src)
            continue
        if isinstance(src, BuildRecord):
            out.write_text(build_record(src))
            out.chmod(mode)
            continue
        if isinstance(src, Generated):
            source = gendir / src
        elif src is None or isinstance(src, Built):
            source = bindir / (src or Path(rel).name)
        else:
            source = ROOT / src
        if not source.is_file():
            fail(f'missing source {source} for /{rel}')
        if isinstance(src, PkgConfig):
            out.write_text(pkg_config_text(source.read_text()))
        else:
            shutil.copyfile(source, out)
        out.chmod(mode)


def tar_entries(raw, links=None):
    """{path: (mode, bytes)} of the regular files in a tar stream, directories skipped. A symbolic link is accepted
    only when `links` ({path: target}) names it with that target, and is listed with empty bytes; any other link or
    non-regular entry is refused."""
    found, links = {}, links or {}
    with tarfile.open(fileobj=io.BytesIO(raw)) as t:
        for m in t.getmembers():
            name = m.name.removeprefix('./')
            if m.isdir() or not name:
                continue
            if m.issym() and name in links:
                if m.linkname != links[name]:
                    fail(f'link {name} points to {m.linkname}, the manifest says {links[name]}')
                found[name] = (m.mode, b'')
                continue
            if not m.isfile():
                fail(f'unexpected non-regular entry {name}' + (f' (a link to {m.linkname})' if m.issym() else ''))
            found[name] = (m.mode, t.extractfile(m).read())
    return found


def entries(path, links=None):
    """{path: (mode, bytes)} of the files and the manifest's links in a .pkg.tar.zst."""
    return tar_entries(subprocess.run(['zstd', '-dcq', str(path)], capture_output=True, check=True).stdout, links)


def readelf_problems(output, symbols, want_soname):
    """What is wrong with a library whose `readelf --dyn-syms -W -d` output this is: global defined symbols other
    than `symbols`, version definitions, another soname, no NODELETE."""
    exported, problems = set(), []
    for line in output.splitlines():
        # Num: Value Size Type Bind Vis [other flags] Ndx Name, where a name may carry @VERSION or @@VERSION and an
        # undefined one the version's index, `(3)`
        parts = line.split()
        if len(parts) < 7 or not re.fullmatch(r'\d+:', parts[0]):
            continue
        at = 6
        while at < len(parts) and parts[at].startswith('['):
            at += next((i for i, p in enumerate(parts[at:], 1) if p.endswith(']')), len(parts))
        if at + 1 < len(parts) and parts[4] in ('GLOBAL', 'WEAK') and parts[at] != 'UND' \
                and parts[3] not in ('SECTION', 'FILE'):
            exported.add(parts[at + 1].split('@')[0])
    if extra := sorted(exported - set(symbols)):
        problems.append(f'exports {", ".join(extra)}, which {ABI_SYMBOLS} does not list')
    if missing := sorted(set(symbols) - exported):
        problems.append(f'does not export {", ".join(missing)}')
    if re.search(r'\((VERDEF|VERDEFNUM)\)', output):
        problems.append('defines symbol versions; ABI 1 has none')
    name = re.search(r'\(SONAME\)\s+Library soname: \[(.*)\]', output)
    if not name or name[1] != want_soname:
        problems.append(f'soname {name[1] if name else "(none)"}, not {want_soname}')
    flags = re.search(r'\(FLAGS_1\)\s+Flags: (.*)', output)
    if not flags or 'NODELETE' not in flags[1].split():
        problems.append('is not linked with -z nodelete (no NODELETE in FLAGS_1)')
    return problems


def sdk_problems(found, manifest):
    """The SDK checks of a package's files: the library as readelf reads it, and every file copied or generated from the
    checkout byte-equal to what `install` writes."""
    problems = []
    for rel, src, _mode, _conf in manifest:
        if rel not in found or isinstance(src, Link):
            continue
        body = found[rel][1]
        if isinstance(src, Built):
            with tempfile.NamedTemporaryFile(suffix='.so') as library:
                library.write(body)
                library.flush()
                readelf = subprocess.run(['readelf', '--dyn-syms', '-W', '-d', library.name], capture_output=True,
                                         text=True)
            if readelf.returncode:
                problems.append(f'/{rel}: readelf cannot read it: {readelf.stderr.strip()}')
                continue
            symbols = (ROOT / ABI_SYMBOLS).read_text().split()
            problems += [f'/{rel}: {p}' for p in readelf_problems(readelf.stdout, symbols, soname())]
        elif isinstance(src, PkgConfig):
            if body.decode(errors='replace') != pkg_config_text((ROOT / src).read_text()):
                problems.append(f'/{rel}: differs from what {src} gives for ABI {".".join(map(str, abi_version()))}')
        elif isinstance(src, BuildRecord):
            try:
                record = json.loads(body)
            except ValueError:
                record = None
            if not isinstance(record, dict) or set(record) != RECORD_FIELDS or record['package'] != src:
                problems.append(f'/{rel}: not the build record of {src}')
        elif src is not None and body != (ROOT / src).read_bytes():
            problems.append(f'/{rel}: differs from {src}')
    return problems


def generate(cli, gendir):
    """Writes into gendir what a package build writes with the built `cupertino` (as the PKGBUILDs' build() does):
    the manual pages in man/, each shell's completions in completions/<shell>, and MANIFEST."""
    gendir = Path(gendir)
    (gendir / 'completions').mkdir(parents=True, exist_ok=True)
    env = {'PATH': '/usr/bin:/bin', 'CUPERTINO_ASSET_SEARCH_PATH': ''}
    subprocess.run([str(cli), 'dev', 'manual', str(gendir / 'man')], env=env, check=True, capture_output=True)
    for shell, _dest in COMPLETIONS:
        script = subprocess.run([str(cli), 'completions', shell], env=env, check=True, capture_output=True).stdout
        (gendir / 'completions' / shell).write_bytes(script)
    write_gen_manifest(gendir)


def write_gen_manifest(gendir):
    names = sorted(p.relative_to(gendir).as_posix() for p in Path(gendir).rglob('*') if p.is_file() and p.name != 'MANIFEST')
    (Path(gendir) / 'MANIFEST').write_text(''.join(f'{n}\n' for n in names))


def undated(page):
    """A manual page with the date of its .TH line left out: the date is the build's SOURCE_DATE_EPOCH."""
    return re.sub(rb'^(\.TH \S+ \S+) \S+', rb'\1 DATE', page, count=1)


def expected_generated(args, tmp):
    """The runtime manifest's generated entries and, with --bindir, the bytes the built cupertino writes for each."""
    if args.bindir:
        gendir = Path(tmp) / 'gen'
        generate(Path(args.bindir) / 'cupertino', gendir)
        bodies = {rel: undated((gendir / src).read_bytes()) for rel, src, _m, _c in generated(gendir)}
        return gendir, bodies
    if args.gendir:
        listed = (Path(args.gendir) / 'MANIFEST').read_text().split()
        gendir = Path(tmp) / 'gen'
        for name in listed:
            (gendir / name).parent.mkdir(parents=True, exist_ok=True)
            (gendir / name).touch()
        return gendir, {}
    fail('verifying the runtime package needs --bindir (the built cupertino) or --gendir (its MANIFEST)')


def build_path(root):
    """A build-tree (`root`) or home path at the start of a path string. Remapped paths (/usr/src/..., /cargo/...) pass,
    and so does a path under some home written from `~` (`--root ~/src/cupertino` in an example), whatever the build
    tree is called."""
    return re.compile(rb'(?<![\w./~-])(' + re.escape(str(root).encode()) + rb'|/home|/root)/')


def cmd_verify(args):
    bodies, gendir = {}, None
    with tempfile.TemporaryDirectory() as tmp:
        if args.package == 'runtime':
            gendir, bodies = expected_generated(args, tmp)
        manifest = MANIFESTS[args.package](gendir)
    links = {rel: src for rel, src, _m, _c in manifest if isinstance(src, Link)}
    found = entries(Path(args.artifact).resolve(), links)
    expected = {rel: mode for rel, _src, mode, _c in manifest}
    texts = {rel for rel, src, _m, _c in manifest if isinstance(src, (Generated, BuildRecord))}
    problems = [f'unexpected file /{r}' for r in sorted(set(found) - set(expected) - EXTRA)]
    problems += [f'missing file /{r}' for r in sorted(set(expected) - set(found))]
    for rel, body in sorted(bodies.items()):
        if rel in found and undated(found[rel][1]) != body:
            problems.append(f'/{rel}: differs from what the built cupertino generates')
    hashes = {f'usr/share/cupertino/assets/wallpapers/{w["file"]}': w['sha256'] for w in WALLPAPERS}
    leak = build_path(ROOT)
    for rel, (mode, body) in sorted(found.items()):
        if mode & (stat.S_ISUID | stat.S_ISGID):
            problems.append(f'setuid/setgid bit on /{rel}')
        # a link's own mode means nothing on Linux; its target was checked when the package was read
        if rel in expected and rel not in links and stat.S_IMODE(mode) != expected[rel]:
            problems.append(f'/{rel}: mode {stat.S_IMODE(mode):o}, manifest {expected[rel]:o}')
        if rel in hashes and hashlib.sha256(body).hexdigest() != hashes[rel]:
            problems.append(f'/{rel}: sha256 differs from wallpapers.json')
        if body.startswith(b'\x7fELF') or rel in texts:
            hit = leak.search(body)
            if hit:
                problems.append(f'/{rel}: holds the build or home path {body[hit.start():hit.start() + 60]!r}')
        if rel in texts and rel.endswith('build-info.json'):
            record = json.loads(body)
            if set(record) != RECORD_FIELDS:
                problems.append(f'/{rel}: fields {sorted(record)}')
    if args.package == 'sdk':
        problems += sdk_problems(found, manifest)
    if problems:
        fail(f'{args.artifact}:\n  ' + '\n  '.join(problems))
    size = sum(len(b) for _m, b in found.values())
    print(f'{Path(args.artifact).name}: {len(expected)} manifest files, {size} bytes installed, '
          f'{Path(args.artifact).stat().st_size} bytes packed: ok')


def closure_roots(package=None):
    """The workspace crates whose closure a package links: the shipped binaries (runtime), the shipped libraries (sdk),
    or both (THIRD-PARTY-LICENSES, which the runtime and the SDK carry)."""
    binaries, libraries = binary_crates(), set(LIBRARIES)
    return {'runtime': binaries, 'sdk': libraries}.get(package, binaries | libraries)


_GRAPH = {}


def dependency_graph():
    """(cargo tree text, cargo metadata) of the workspace as the packages build it, read once."""
    if not _GRAPH:
        base = ['--offline', '--locked', '--all-features', '--target', 'x86_64-unknown-linux-gnu']
        _GRAPH['tree'] = subprocess.run(
            ['cargo', 'tree', '--workspace', '-e', 'normal,build', '--no-dedupe', '--prefix', 'depth', *base],
            cwd=ROOT, capture_output=True, text=True, check=True).stdout
        _GRAPH['meta'] = json.loads(subprocess.run(['cargo', 'metadata', '--format-version', '1', *base[:3]], cwd=ROOT,
                                                   capture_output=True, text=True, check=True).stdout)
    return _GRAPH['tree'], _GRAPH['meta']


def linked_packages(roots=None):
    """cargo metadata packages in the normal and build dependency closure of `roots` (closure_roots() by default) on
    Linux (generators run at build time compile their output into the binaries), as the packages build it
    (`--workspace --all-features`): the workspace and path crates (`source` None) and the registry crates."""
    roots = closure_roots() if roots is None else set(roots)
    tree, meta = dependency_graph()
    linked, root = set(), None
    for line in tree.splitlines():
        m = re.match(r'(\d+)(\S+) v(\S+)', line)
        if m:
            if m[1] == '0':
                root = m[2]
            if root in roots:
                linked.add((m[2], m[3]))
    return sorted((p for p in meta['packages'] if (p['name'], p['version']) in linked), key=lambda p: p['id'])


def notice_files(package):
    """NOTICE files at the top of a crate's source directory."""
    return sorted(f for f in Path(package['manifest_path']).parent.iterdir()
                  if f.is_file() and re.fullmatch(r'NOTICES?(\..+)?', f.name, re.I))


def embedded_notice_files(package):
    """Paths, relative to the crate's source directory, of the files whose notice must ship."""
    root = Path(package['manifest_path']).parent
    if package['name'] in SCANNER_CRATES:
        names = {Path(x).as_posix().removeprefix('./') for f in sorted((root / 'src').rglob('*.rs'))
                 for x in re.findall(r'"([^"\s]+\.xml)"', f.read_text())}
        return sorted(names)
    return list(EMBEDDED_NOTICES.get(package['name'], ()))


def embedded_notices(package):
    """[(file, text)] of the licence notice of each file from embedded_notice_files (the leading comment of a registry
    file, else its <copyright>); ValueError when a file or its notice is missing."""
    found = []
    for name in embedded_notice_files(package):
        path = Path(package['manifest_path']).parent / name
        if not path.is_file():
            raise ValueError(f'{package["name"]} {package["version"]}: {name} is missing')
        xml = path.read_text()
        m = (re.search(r'<registry>\s*(?:<!--(.*?)-->|<comment>(.*?)</comment>)', xml, re.S)
             or re.search(r'<copyright>(.*?)</copyright>', xml, re.S))
        text = textwrap.dedent(html.unescape(next((g for g in m.groups() if g), ''))).strip() if m else ''
        if 'opyright' in text:
            found.append((name, text))
        elif package['name'] not in SCANNER_CRATES:  # a protocol file may carry no notice; a registry file must
            raise ValueError(f'{package["name"]} {package["version"]}: {name} has no copyright notice')
    return found


def notice_licenses(roots=None):
    """{SPDX identifier: [crate directories]} named by `SPDX-License-Identifier:` lines of the embedded notices in the
    closure of `roots`."""
    found = {}
    for p in linked_packages(roots):
        if p['source']:
            for _name, body in embedded_notices(p):
                for expr in re.findall(r'SPDX-License-Identifier:[ \t]*([^\n]+)', body):
                    for ident in re.findall(r'[\w.+-]+', re.sub(r'\b(AND|OR|WITH)\b', ' ', expr)):
                        found.setdefault(ident, set()).add(Path(p['manifest_path']).parent)
    return found


def notice_license_text(ident, dirs):
    """The text of licence `ident` from a LICENSES/<ident>.txt in the crates that name it, else ValueError."""
    for d in sorted(dirs):
        for f in sorted(d.rglob(f'LICENSES/{ident}.txt')):
            return f.read_text().strip()
    raise ValueError(f'no LICENSES/{ident}.txt in {", ".join(sorted(str(d) for d in dirs))}')


def third_party_crates():
    """['name version'] of the registry crates in the closure."""
    return sorted(f'{p["name"]} {p["version"]}' for p in linked_packages() if p['source'])


def shipped_licenses(package):
    """SPDX identifiers the package ships: those of the workspace crates in the normal and build dependency closure of its
    binaries (runtime) or its library (sdk), which are linked statically, plus the licences named by the embedded
    protocol notices there; or the pack's own (data)."""
    if package == 'data':
        return set(DATA_LICENSES)
    roots, found = closure_roots(package), set()
    for p in linked_packages(roots):
        if p['source'] is None:
            found |= set(re.findall(r'[\w.+-]+', re.sub(r'\b(AND|OR|WITH)\b', ' ', p['license'] or '')))
    return found | set(notice_licenses(roots))


def declared_licenses(package):
    """{place: SPDX identifiers} the package's licence lists in the PKGBUILDs: the recipe's own list for the runtime,
    the license=() of package_cupertino-data() and of package_cupertino-kit() (or package_cupertino-kit-git()). A recipe
    without the SDK's function is reported, since both recipes build it."""
    found = {}
    for pkgbuild in sorted(ROOT.glob('packaging/arch/*/PKGBUILD')):
        text = pkgbuild.read_text()
        data_block = re.search(r'^package_cupertino-data\(\).*?^\}', text, re.M | re.S)
        if package == 'sdk':
            kit = re.search(r'^package_(cupertino-kit(?:-git)?)\(\).*?^\}', text, re.M | re.S)
            listed = kit and re.search(r'^\s+license=\(([^)]*)\)', kit[0], re.M)
            found[f'{pkgbuild.relative_to(ROOT)} {kit[1] if kit else "cupertino-kit"}'] = \
                set(re.findall(r"'([^']+)'", listed[1])) if listed else set()
            continue
        if package == 'data':
            if data_block:
                found[f'{pkgbuild.relative_to(ROOT)} cupertino-data'] = set(re.findall(r"'([^']+)'", re.search(r'license=\(([^)]*)\)', data_block[0])[1]))
        else:
            head = text[:data_block.start()] if data_block else text
            found[f'{pkgbuild.relative_to(ROOT)}'] = set(re.findall(r"'([^']+)'", re.search(r'^license=\(([^)]*)\)', head, re.M)[1]))
    return found


def sdk_text_problems():
    """Licences of the SDK's closure without their text in the package: an SPDX identifier of a workspace crate needs the
    text of that name (OFL.txt for OFL-1.1), and a path crate with only a licence file needs that file."""
    names = {n: src for n, src in SDK_LICENSE_TEXTS}
    problems = [f'SDK licence text {src} for /usr/share/licenses/cupertino-kit/{n} does not exist'
                for n, src in SDK_LICENSE_TEXTS if not (ROOT / src).is_file()]
    for p in linked_packages(closure_roots('sdk')):
        if p['source'] is not None:
            continue
        for ident in re.findall(r'[\w.+-]+', re.sub(r'\b(AND|OR|WITH)\b', ' ', p['license'] or '')):
            if {'OFL-1.1': 'OFL.txt'}.get(ident, ident) not in names:
                problems.append(f'the SDK ships {p["name"]} ({ident}) without the text of {ident}')
        if not p['license'] and p.get('license_file'):
            file = (Path(p['manifest_path']).parent / p['license_file']).resolve()
            if file not in {(ROOT / src).resolve() for src in names.values()}:
                problems.append(f'the SDK ships {p["name"]} without its licence file {p["license_file"]}')
    return problems


def missing_licenses(shipped, declared):
    """['<place> lacks <id>, ...'] for each place whose list does not cover every shipped licence."""
    return [f'{place} lacks {", ".join(sorted(shipped - ids))}' for place, ids in sorted(declared.items()) if shipped - ids]


def cmd_third_party(_args):
    about = os.environ.get('CARGO_ABOUT', 'cargo-about')
    crates = third_party_crates()
    # The packages build the whole workspace with all features, so features unify across the binaries.
    out = subprocess.run([about, 'about', 'generate', '--format', 'json', '--workspace', '--all-features', '--locked',
                          '-c', str(ROOT / ABOUT_CONFIG), '--manifest-path', str(ROOT / 'Cargo.toml')],
                         capture_output=True, text=True, check=True).stdout
    texts = {}
    for lic in json.loads(out)['licenses']:
        for use in lic['used_by']:
            c = use['crate']
            if f'{c["name"]} {c["version"]}' in crates:
                texts.setdefault((lic['name'], lic['id'], lic['text']), set()).add(f'{c["name"]} {c["version"]}')
    covered = set().union(*texts.values())
    if set(crates) != covered:
        fail(f'cargo-about lacks a licence for: {sorted(set(crates) - covered)}')
    have = {ident for _name, ident, _text in texts}
    for ident, dirs in sorted(notice_licenses().items()):
        if ident not in have:
            try:
                texts[(ident, ident, notice_license_text(ident, dirs))] = {f'the {ident} notices of embedded protocol files'}
            except ValueError as e:
                fail(str(e))
    notices = {}  # notice text -> the crates it comes from
    for p in linked_packages():
        if p['source']:
            try:
                bodies = [f.read_text().strip() for f in notice_files(p)] + [t for _n, t in embedded_notices(p)]
            except ValueError as e:
                fail(str(e))
            for body in bodies:
                notices.setdefault(body, set()).add(f'{p["name"]} {p["version"]}')
    bar = '=' * 78
    out = ['Third-party software in the Cupertino binaries',
           'Generated by `python3 packaging/release.py third-party` with cargo-about; do not edit.',
           '', f'CRATES ({len(crates)})', bar, *crates, '', 'NOTICES', bar]
    out += [] if notices else ['(none)', '']
    for body, users in sorted(notices.items(), key=lambda kv: (sorted(kv[1]), kv[0])):
        out += ['--- ' + ', '.join(sorted(users)), body, '']
    out += ['LICENSES', bar]
    for (name, ident, text), users in sorted(texts.items(), key=lambda kv: (kv[0][1], kv[0][2])):
        out += [f'--- {name} ({ident})', 'Used by: ' + ', '.join(sorted(users)), '', text.strip(), '']
    (ROOT / THIRD_PARTY).write_text('\n'.join(out) + '\n')
    print(f'{THIRD_PARTY}: {len(crates)} crates, {len(texts)} licence texts, {len(notices)} notices')


def third_party_problems():
    """What makes THIRD-PARTY-LICENSES stale: a crate of the closure missing from it or no longer in the closure, or
    a NOTICE file whose text it lacks."""
    path = ROOT / THIRD_PARTY
    if not path.is_file():
        return [f'{THIRD_PARTY} is missing: run `python3 packaging/release.py third-party`']
    text = path.read_text()
    listed = re.search(r'^CRATES \(\d+\)\n=+\n(.*?)\n\nNOTICES', text, re.M | re.S)
    have = set(listed[1].splitlines()) if listed else set()
    want = set(third_party_crates())
    problems = [f'{THIRD_PARTY} lacks {c}' for c in sorted(want - have)]
    problems += [f'{THIRD_PARTY} lists {c}, which is not linked' for c in sorted(have - want)]
    for p in linked_packages():
        if not p['source']:
            continue
        for f in notice_files(p):
            if f.read_text().strip() not in text:
                problems.append(f'{THIRD_PARTY} lacks the {f.name} of {p["name"]} {p["version"]}')
        try:
            for name, body in embedded_notices(p):
                if body not in text:
                    problems.append(f'{THIRD_PARTY} lacks the licence comment of {name} in {p["name"]} {p["version"]}')
        except ValueError as e:
            problems.append(str(e))
    for ident in sorted(notice_licenses()):
        if not re.search(rf'^--- .*\({re.escape(ident)}\)$', text, re.M):
            problems.append(f'{THIRD_PARTY} has no text of {ident}, which an embedded notice names')
    return problems


def cmd_check_metadata(_args):
    problems = third_party_problems()
    problems += [f'licence text {src} for /usr/share/licenses/cupertino/{n} does not exist'
                for n, src in LICENSE_TEXTS if not (ROOT / src).is_file()]
    for package in MANIFESTS:
        problems += [f'{package} licence: {m}' for m in missing_licenses(shipped_licenses(package), declared_licenses(package))]
    problems += sdk_text_problems()
    if problems:
        fail('package metadata is incomplete:\n  ' + '\n  '.join(problems))
    print('package licence lists and third-party notices are complete')


def git(*a):
    return subprocess.run(['git', '-C', str(ROOT), *a], capture_output=True, text=True, check=True).stdout.strip()


TAG = re.compile(r'v(\d+\.\d+\.\d+)(?:-(rc\d+))?')


def release_version(tag):
    """The Arch pkgver of a v<version> or v<version>-rc<N> tag: 0.1.0 or 0.1.0rc1 (no hyphen; sorts before 0.1.0)."""
    m = TAG.fullmatch(tag or f'v{version("cupertino-server")}')
    if not m:
        fail(f'tag {tag} is neither v<version> nor v<version>-rc<N>')
    return m[1] + (m[2] or '')


def source_text(commit, path):
    return (ROOT / path).read_text() if commit is None else git('show', f'{commit}:{path}')


def source_version(commit):
    return version('cupertino-server') if commit is None else \
        tomllib.loads(source_text(commit, 'crates/cupertino-server/Cargo.toml'))['package']['version']


def cmd_source(args):
    commit = args.commit or 'HEAD'
    v = release_version(args.tag or f'v{source_version(args.commit)}')
    out = Path(args.output)
    out.mkdir(parents=True, exist_ok=True)
    tar = subprocess.run(['git', '-C', str(ROOT), 'archive', '--format=tar', f'--prefix=cupertino-{v}/', commit],
                         capture_output=True, check=True).stdout
    dest = out / f'cupertino-{v}-source.tar.xz'
    dest.write_bytes(lzma.compress(tar, preset=9))
    print(dest)


def cmd_pkgbuild(args):
    """Write the stable PKGBUILD with the release source archive's checksum in place of UNRELEASED."""
    text = source_text(args.commit, 'packaging/arch/cupertino/PKGBUILD')
    if "sha256sums=('UNRELEASED')" not in text:
        fail('packaging/arch/cupertino/PKGBUILD has no UNRELEASED checksum to fill')
    digest = hashlib.sha256(Path(args.archive).read_bytes()).hexdigest()
    text = text.replace("sha256sums=('UNRELEASED')", f"sha256sums=('{digest}')")
    if args.tag:
        for old, new in ((f'pkgver={source_version(args.commit)}\n', f'pkgver={release_version(args.tag)}\n'),
                         ('_tag=v$pkgver\n', f'_tag={args.tag}\n')):
            if text.count(old) != 1:
                fail(f'packaging/arch/cupertino/PKGBUILD must have exactly one line {old.strip()!r}')
            text = text.replace(old, new)
    out = Path(args.output)
    out.write_text(text)
    print(out)


def cmd_verify_tag(args):
    v = version('cupertino-server')
    m = TAG.fullmatch(args.tag)
    if not m or m[1] != v:
        fail(f'tag {args.tag} does not match the cupertino-server version {v}')
    if version('cupertino-assets') != v:
        fail(f'cupertino-assets is {version("cupertino-assets")}, cupertino-server {v}')
    if subprocess.run(['git', '-C', str(ROOT), 'rev-parse', '--verify', '--quiet', f'refs/tags/{args.tag}']).returncode:
        fail(f'tag {args.tag} does not exist')
    if git('rev-list', '-n1', args.tag) != git('rev-parse', 'HEAD'):
        fail(f'{args.tag} does not point at HEAD')
    if subprocess.run(['git', '-C', str(ROOT), 'merge-base', '--is-ancestor', 'HEAD', 'origin/main']).returncode:
        fail('HEAD is not reachable from origin/main')
    print(f'{args.tag}: version {v}, reachable from origin/main')


def package_files(directory, pkgver):
    """Exactly the verified Arch runtime, data and SDK packages; never copy unrelated release inputs into an ISO."""
    found = sorted(directory.glob('*.pkg.tar.zst'))
    patterns = (rf'cupertino-{re.escape(pkgver)}-\d+-x86_64\.pkg\.tar\.zst',
                rf'cupertino-data-{re.escape(pkgver)}-\d+-any\.pkg\.tar\.zst',
                rf'cupertino-kit-{re.escape(pkgver)}-\d+-x86_64\.pkg\.tar\.zst')
    if len(found) != len(patterns) or \
            any(sum(bool(re.fullmatch(pattern, p.name)) for p in found) != 1 for pattern in patterns):
        fail(f'{directory}: expected one runtime, one data and one SDK package for {pkgver}')
    if any(p.is_symlink() or not p.is_file() for p in found):
        fail(f'{directory}: package inputs must be regular files')
    return found


def cmd_record_packages(args):
    """Record provenance only after the build, manifest verification and clean-container install test succeed."""
    directory = Path(args.directory)
    commit = git('rev-parse', '--verify', '--end-of-options', f'{args.commit or "HEAD"}^{{commit}}')
    pkgver = release_version(args.tag or f'v{source_version(commit)}')
    files = package_files(directory, pkgver)
    record = {'schema': 1, 'commit': commit, 'version': pkgver,
              'files': {p.name: package_digest(p) for p in files}}
    (directory / 'packages.json').write_text(json.dumps(record, indent=2, sort_keys=True) + '\n')


def cmd_verify_packages(args):
    directory = Path(args.directory)
    record = json.loads((directory / 'packages.json').read_text())
    commit = git('rev-parse', '--verify', '--end-of-options', f'{args.commit or "HEAD"}^{{commit}}')
    pkgver = release_version(args.tag or f'v{source_version(commit)}')
    if record.get('schema') != 1 or record.get('commit') != commit or record.get('version') != pkgver:
        fail(f'{directory}/packages.json: packages are not from this commit and release version')
    files = package_files(directory, pkgver)
    actual = {p.name: package_digest(p) for p in files}
    if actual != record.get('files'):
        fail(f'{directory}/packages.json: package names or SHA256 digests differ')
    destination = Path(args.copy_to)
    destination.mkdir(parents=True, exist_ok=True)
    for package in files:
        shutil.copy2(package, destination / package.name)
    shutil.copy2(directory / 'packages.json', destination / 'packages.json')
    print(f'reused {len(files)} verified Arch packages from {record["commit"]}')


def package_digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def main():
    p = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    sub = p.add_subparsers(dest='command', required=True)
    for name in ('install', 'verify'):
        c = sub.add_parser(name)
        c.add_argument('--format', choices=FORMATS, default='arch')
        c.add_argument('--package', choices=MANIFESTS, required=True)
    sub.choices['install'].add_argument('--destdir', required=True)
    sub.choices['install'].add_argument('--bindir', default=str(ROOT / 'target/release'))
    sub.choices['install'].add_argument('--gendir', help='the files the built cupertino generated (runtime)')
    built = sub.choices['verify'].add_mutually_exclusive_group()
    built.add_argument('--bindir', help='the built binaries: the runtime check generates the pages again')
    built.add_argument('--gendir', help='the generated files whose MANIFEST names the pages')
    sub.choices['verify'].add_argument('artifact')
    sub.add_parser('check-metadata')
    sub.add_parser('third-party')
    c = sub.add_parser('source')
    c.add_argument('--output', required=True)
    c.add_argument('--tag')
    c.add_argument('--commit')
    sub.add_parser('verify-tag').add_argument('tag')
    k = sub.add_parser('pkgbuild')
    k.add_argument('--tag')
    k.add_argument('--commit')
    k.add_argument('archive')
    k.add_argument('output')
    for name in ('record-packages', 'verify-packages'):
        c = sub.add_parser(name)
        c.add_argument('directory')
        c.add_argument('--tag')
        c.add_argument('--commit')
    sub.choices['verify-packages'].add_argument('--copy-to', required=True)
    args = p.parse_args()
    {'install': cmd_install, 'verify': cmd_verify, 'check-metadata': cmd_check_metadata, 'source': cmd_source,
     'verify-tag': cmd_verify_tag, 'third-party': cmd_third_party, 'pkgbuild': cmd_pkgbuild,
     'record-packages': cmd_record_packages, 'verify-packages': cmd_verify_packages}[args.command](args)


if __name__ == '__main__':
    main()
