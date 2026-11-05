#!/usr/bin/env python3
"""Cupertino release tool: the one installation manifest behind every package format.

Usage:
  release.py install --format {arch,deb,rpm} --package {runtime,data} --destdir DIR [--bindir DIR]
  release.py verify --format {arch,deb,rpm} --package {runtime,data} ARTIFACT
  release.py check-metadata
  release.py third-party
  release.py source --output DIR
  release.py verify-tag TAG
  release.py pkgbuild ARCHIVE OUTPUT

install         copies exactly the manifest's files into DIR with explicit modes (Arch package(), portable tarballs).
verify          lists a built package (.pkg.tar.zst, .deb, .rpm) and fails unless its files and modes equal the manifest
                plus the format's own metadata files. It also rejects setuid/setgid bits, wallpapers whose hash differs
                from packaging/data/wallpapers.json and executables that hold a home-directory or checkout path.
check-metadata  compares the deb and rpm asset tables in the crates' Cargo.toml files with the manifest and fails when a licence
                list (PKGBUILDs, rpm, Debian copyright) misses a licence the package ships.
third-party     regenerates packaging/licenses/THIRD-PARTY-LICENSES with cargo-about (0.9.3, on PATH or $CARGO_ABOUT): the
                licence texts and NOTICE files of every registry crate in the normal and build closure of the shipped binaries.
source          writes cupertino-<version>-source.tar.xz from the committed HEAD (git archive: file times are the
                commit time, no untracked files).
verify-tag      fails unless TAG is v<version>, points at HEAD, and HEAD is reachable from origin/main.
pkgbuild        writes the stable Arch PKGBUILD with ARCHIVE's sha256 (the release's source archive).

The runtime version is cupertino-server's, the data version cupertino-assets'. Nothing here reads outside the checkout.
"""
import argparse
import os
import fnmatch
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
FORMATS = ('arch', 'deb', 'rpm')
# The applications among the shipped binaries: each has a desktop entry crates/<app>/data/applications/<app>.desktop.
APPS = ('cupertino-files', 'cupertino-settings')
BINARIES = (
    'cupertino-server',
    'cupertino-dock', 'cupertino-menubar', 'cupertino-launcher', 'cupertino-notifications', 'cupertino-controlcenter',
    'cupertino-osd', 'cupertino-screenshot', 'cupertino-windowmanager', 'cupertino-login', 'cupertino-polkit-agent',
    'cupertino-portal',
    *APPS,
    'cupertino-sessiond', 'cupertino-netd', 'cupertino-btd', 'cupertino-powerd',
    'cupertino-diagnose',
)
# Besides an app's own entry, the only file an app crate may ship there: the session's default applications.
MIMEAPPS = 'cupertino-mimeapps.list'
PAM_SERVICES = ('cupertino-login', 'cupertino-session', 'cupertino-greeter', 'cupertino-autologin')
UNITS = ('cupertino-login', 'cupertino-netd', 'cupertino-btd', 'cupertino-powerd')
THEME_FILES = ('tokens.json', 'materials.json', 'animations.json')
WALLPAPERS = json.loads((ROOT / 'packaging/data/wallpapers.json').read_text())['files']
NAMES = {'runtime': 'cupertino', 'data': 'cupertino-data'}
# Licence texts of the crates linked into the binaries, beside the GPL text (arch and rpm; the deb copyright file holds them).
LICENSE_TEXTS = (('MPL-2.0', 'crates/cupertino-geometry/LICENSE'), ('MIT', 'crates/cupertino-xkbcommon/LICENSE'),
                 ('libxkbcommon', 'third-party/xkbcommon-rs/LICENSE'), ('smithay', 'third-party/smithay/LICENSE.txt'),
                 ('OFL.txt', 'crates/cupertino-assets/fonts/OFL.txt'))
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
# Debian copyright file license names -> SPDX.
DEB_LICENSES = {'GPL-3+': 'GPL-3.0-or-later', 'MPL-2.0': 'MPL-2.0', 'OFL-1.1': 'OFL-1.1', 'Unsplash': 'LicenseRef-Unsplash',
                'LGPL-2.1+': 'LGPL-2.1-or-later', 'MIT': 'MIT', 'MIT-CMU': 'MIT-CMU', 'BSD-3-Clause': 'BSD-3-Clause'}


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


def runtime(fmt):
    """(destination, source relative to the checkout or None for a built binary, mode, config) of `cupertino`."""
    f = 'packaging/files'
    files = [(f'usr/bin/{b}', None, 0o755, False) for b in BINARIES]
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
    files += [(f'etc/pam.d/{s}', f'packaging/pam/{fmt}/{s}', 0o644, True) for s in PAM_SERVICES]
    if fmt != 'deb':  # cargo-deb writes usr/share/doc/cupertino/copyright (packaging/debian/cupertino.copyright)
        files += [('usr/share/licenses/cupertino/LICENSE', 'LICENSE', 0o644, False),
                  *((f'usr/share/licenses/cupertino/{n}', src, 0o644, False) for n, src in LICENSE_TEXTS)]
    # Debian keeps licence documents under doc (the copyright file refers to it there).
    third = 'usr/share/doc/cupertino' if fmt == 'deb' else 'usr/share/licenses/cupertino'
    files.append((f'{third}/THIRD-PARTY-LICENSES', THIRD_PARTY, 0o644, False))
    return files


def data(fmt):
    """`cupertino-data`: the default asset pack and its per-component licences."""
    a, d, fonts = 'usr/share/cupertino/assets', 'packaging/data', 'crates/cupertino-assets/fonts'
    files = [(f'{a}/{p}', f'{d}/{p}', 0o644, False) for p in ('manifest.json', 'icon-map.toml', 'wallpapers/credits.txt')]
    files += [(f'{a}/fonts/{n}', f'{fonts}/{n}', 0o644, False) for n in ('InterVariable-subset.ttf', 'OFL.txt')]
    files += [(f'{a}/wallpapers/{w["file"]}', f'{d}/wallpapers/{w["file"]}', 0o644, False) for w in WALLPAPERS]
    if fmt != 'deb':  # Debian has one machine-readable copyright file instead (packaging/debian/cupertino-data.copyright)
        lic = 'usr/share/licenses/cupertino-data'
        files += [(f'{lic}/DATA', 'packaging/licenses/DATA', 0o644, False),
                  (f'{lic}/Unsplash.txt', 'packaging/licenses/Unsplash.txt', 0o644, False),
                  (f'{lic}/OFL.txt', f'{fonts}/OFL.txt', 0o644, False)]
    return files


MANIFESTS = {'runtime': runtime, 'data': data}
# Files the format tools add themselves; anything else that is not in the manifest fails verification.
EXTRA = {'arch': {'.PKGINFO', '.BUILDINFO', '.MTREE'}, 'deb': {'usr/share/doc/{}/copyright'}, 'rpm': set()}


def fail(msg):
    sys.exit(f'release.py: {msg}')


def version(crate):
    return tomllib.loads((ROOT / 'crates' / crate / 'Cargo.toml').read_text())['package']['version']


def cmd_install(args):
    dest, bindir = Path(args.destdir), Path(args.bindir)
    for rel, src, mode, _conf in MANIFESTS[args.package](args.format):
        source = bindir / Path(rel).name if src is None else ROOT / src
        if not source.is_file():
            fail(f'missing source {source} for /{rel}')
        out = dest / rel
        out.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source, out)
        out.chmod(mode)


def tar_entries(raw):
    """{path: (mode, bytes)} of the regular files in a tar stream; links are refused, directories skipped."""
    found = {}
    with tarfile.open(fileobj=io.BytesIO(raw)) as t:
        for m in t.getmembers():
            name = m.name.removeprefix('./')
            if m.isdir() or not name:
                continue
            if not m.isfile():
                fail(f'unexpected non-regular entry {name}')
            found[name] = (m.mode, t.extractfile(m).read())
    return found


def entries(fmt, path):
    if fmt == 'arch':
        return tar_entries(subprocess.run(['zstd', '-dcq', str(path)], capture_output=True, check=True).stdout)
    if fmt == 'deb':
        return tar_entries(subprocess.run(['dpkg-deb', '--fsys-tarfile', str(path)], capture_output=True, check=True).stdout)
    modes = subprocess.run(['rpm', '-qp', '--qf', '[%{FILEMODES:octal} %{FILENAMES}\n]', str(path)],
                           capture_output=True, text=True, check=True).stdout
    with tempfile.TemporaryDirectory() as tmp:
        subprocess.run(f'rpm2cpio "{path}" | cpio -idm --quiet', shell=True, cwd=tmp, check=True)
        found = {}
        for line in modes.splitlines():
            mode, name = line.split(' ', 1)
            if not stat.S_ISDIR(int(mode, 8)):
                found[name.lstrip('/')] = (int(mode, 8), (Path(tmp) / name.lstrip('/')).read_bytes())
        return found


def cmd_verify(args):
    found = entries(args.format, Path(args.artifact).resolve())
    expected = {rel: mode for rel, _src, mode, _c in MANIFESTS[args.package](args.format)}
    extra = {e.format(NAMES[args.package]) for e in EXTRA[args.format]}
    problems = [f'unexpected file /{r}' for r in sorted(set(found) - set(expected) - extra)]
    problems += [f'missing file /{r}' for r in sorted(set(expected) - set(found))]
    hashes = {f'usr/share/cupertino/assets/wallpapers/{w["file"]}': w['sha256'] for w in WALLPAPERS}
    # A build-tree or home path at the start of a path string; remapped paths (/usr/src/..., /cargo/...) pass.
    leak = re.compile(rb'(?<![\w./-])(' + re.escape(str(ROOT).encode()) + rb'|/home|/root)/')
    for rel, (mode, body) in sorted(found.items()):
        if mode & (stat.S_ISUID | stat.S_ISGID):
            problems.append(f'setuid/setgid bit on /{rel}')
        if rel in expected and stat.S_IMODE(mode) != expected[rel]:
            problems.append(f'/{rel}: mode {stat.S_IMODE(mode):o}, manifest {expected[rel]:o}')
        if rel in hashes and hashlib.sha256(body).hexdigest() != hashes[rel]:
            problems.append(f'/{rel}: sha256 differs from wallpapers.json')
        if body.startswith(b'\x7fELF'):
            hit = leak.search(body)
            if hit:
                problems.append(f'/{rel}: holds the build or home path {body[hit.start():hit.start() + 60]!r}')
    if problems:
        fail(f'{args.artifact}:\n  ' + '\n  '.join(problems))
    size = sum(len(b) for _m, b in found.values())
    print(f'{Path(args.artifact).name}: {len(expected)} manifest files, {size} bytes installed, '
          f'{Path(args.artifact).stat().st_size} bytes packed: ok')


def linked_packages():
    """cargo metadata packages in the normal and build dependency closure of BINARIES on Linux (generators run at build time compile their output
    into the binaries), as the packages build it
    (`--workspace --all-features`): the workspace and path crates (`source` None) and the registry crates."""
    base = ['--offline', '--locked', '--all-features', '--target', 'x86_64-unknown-linux-gnu']
    tree = subprocess.run(['cargo', 'tree', '--workspace', '-e', 'normal,build', '--no-dedupe', '--prefix', 'depth', *base],
                          cwd=ROOT, capture_output=True, text=True, check=True).stdout
    linked, root = set(), None
    for line in tree.splitlines():
        m = re.match(r'(\d+)(\S+) v(\S+)', line)
        if m:
            if m[1] == '0':
                root = m[2]
            if root in BINARIES:
                linked.add((m[2], m[3]))
    meta = json.loads(subprocess.run(['cargo', 'metadata', '--format-version', '1', *base[:3]], cwd=ROOT,
                                     capture_output=True, text=True, check=True).stdout)
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


def notice_licenses():
    """{SPDX identifier: [crate directories]} named by `SPDX-License-Identifier:` lines of the embedded notices."""
    found = {}
    for p in linked_packages():
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
    binaries (they are linked statically), plus the licences named by the embedded protocol notices (runtime) or the pack's own (data)."""
    if package == 'data':
        return set(DATA_LICENSES)
    found = set()
    for p in linked_packages():
        if p['source'] is None:
            found |= set(re.findall(r'[\w.+-]+', re.sub(r'\b(AND|OR|WITH)\b', ' ', p['license'] or '')))
    return found | set(notice_licenses())


def declared_licenses(package):
    """{place: SPDX identifiers} the package's metadata lists, in the PKGBUILDs, the rpm and the Debian copyright file."""
    crate = 'cupertino-server' if package == 'runtime' else 'cupertino-assets'
    meta = tomllib.loads((ROOT / 'crates' / crate / 'Cargo.toml').read_text())['package']['metadata']
    ids = lambda text: set(re.findall(r'[\w.+-]+', re.sub(r'\b(AND|OR|WITH)\b', ' ', text)))
    found = {f'{crate} rpm License': ids(meta['generate-rpm']['license'])}
    copyright = ROOT / meta['deb']['license-file'][0].replace('../../', '', 1)
    names = re.findall(r'^License: (\S+)', copyright.read_text(), re.M)
    found[f'{copyright.relative_to(ROOT)}'] = {DEB_LICENSES.get(n, n) for n in names}
    for pkgbuild in sorted(ROOT.glob('packaging/arch/*/PKGBUILD')):
        text = pkgbuild.read_text()
        data_block = re.search(r'^package_cupertino-data\(\).*?^\}', text, re.M | re.S)
        if package == 'data':
            if data_block:
                found[f'{pkgbuild.relative_to(ROOT)} cupertino-data'] = set(re.findall(r"'([^']+)'", re.search(r'license=\(([^)]*)\)', data_block[0])[1]))
        else:
            head = text[:data_block.start()] if data_block else text
            found[f'{pkgbuild.relative_to(ROOT)}'] = set(re.findall(r"'([^']+)'", re.search(r'^license=\(([^)]*)\)', head, re.M)[1]))
    return found


def missing_licenses(shipped, declared):
    """['<place> lacks <id>, ...'] for each place whose list does not cover every shipped licence."""
    return [f'{place} lacks {", ".join(sorted(shipped - ids))}' for place, ids in sorted(declared.items()) if shipped - ids]


def crate_license_problems(copyright_text, crates, paths):
    """['<file> is <id>, ...'] for each of `paths` in a crate {directory: Cargo.toml licence} with one licence that the
    last matching Files paragraph of the Debian copyright file gives another; protocol XML files keep their own."""
    paragraphs = []
    for block in re.split(r'\n\s*\n', copyright_text):
        files = re.search(r'^Files:(.*(?:\n .*)*)', block, re.M)
        lic = re.search(r'^License: (.+)', block, re.M)
        if files and lic:
            paragraphs.append((files[1].split(), DEB_LICENSES.get(lic[1].strip(), lic[1].strip())))
    problems = []
    for path in sorted(paths):
        parts = path.split('/')
        lic = crates.get(parts[1]) if len(parts) > 2 and parts[0] == 'crates' else None
        if lic is None or not re.fullmatch(r'[\w.+-]+', lic) or fnmatch.fnmatchcase(path, 'crates/*/protocols/*.xml'):
            continue
        got = next((l for files, l in reversed(paragraphs) if any(fnmatch.fnmatchcase(path, f) for f in files)), None)
        if got != lic:
            problems.append(f'{path} is {lic} as its crate, the Debian copyright file gives it {got}')
    return problems


def tracked_crate_files():
    """The files git tracks under crates/."""
    out = subprocess.run(['git', 'ls-files', 'crates'], cwd=ROOT, capture_output=True, text=True, check=True).stdout
    return out.splitlines()


def workspace_crate_licenses():
    """{crate directory: the licence its Cargo.toml declares} of the workspace crates."""
    workspace = tomllib.loads((ROOT / 'Cargo.toml').read_text())['workspace']['package']['license']
    found = {}
    for manifest in sorted(ROOT.glob('crates/*/Cargo.toml')):
        lic = tomllib.loads(manifest.read_text())['package'].get('license')
        found[manifest.parent.name] = workspace if isinstance(lic, dict) else lic
    return {name: lic for name, lic in found.items() if lic}


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
    problems += crate_license_problems((ROOT / 'packaging/debian/cupertino.copyright').read_text(), workspace_crate_licenses(),
                                       tracked_crate_files())
    for package in MANIFESTS:
        problems += [f'{package} licence: {m}' for m in missing_licenses(shipped_licenses(package), declared_licenses(package))]
    for crate, package in (('cupertino-server', 'runtime'), ('cupertino-assets', 'data')):
        meta = tomllib.loads((ROOT / 'crates' / crate / 'Cargo.toml').read_text())['package']['metadata']
        for fmt, table in (('deb', meta['deb']), ('rpm', meta['generate-rpm'])):
            manifest = MANIFESTS[package](fmt)
            got = {}
            for a in table['assets']:
                src, dst, mode = (a[0], a[1], a[2]) if fmt == 'deb' else (a['source'], a['dest'], a['mode'])
                got[dst.lstrip('/')] = (src, int(mode, 8), a.get('config') == 'noreplace' if fmt == 'rpm' else None)
            want = {}
            for rel, src, mode, conf in manifest:
                if src is None:
                    src = f'target/release/{Path(rel).name}'
                elif fmt == 'deb':
                    src = f'../../{src}'  # cargo-deb resolves sources from the crate directory
                want[rel] = (src, mode, conf if fmt == 'rpm' else None)
            for rel in sorted(set(got) | set(want)):
                if got.get(rel) != want.get(rel):
                    problems.append(f'{crate} {fmt} /{rel}: Cargo.toml {got.get(rel)}, manifest {want.get(rel)}')
            if fmt == 'deb':
                conf_files = sorted(c.lstrip('/') for c in table.get('conf-files', []))
                want_conf = sorted(rel for rel, _s, _m, conf in manifest if conf)
                if conf_files != want_conf:
                    problems.append(f'{crate} deb conf-files {conf_files}, manifest {want_conf}')
    if problems:
        fail('package metadata differs from the manifest:\n  ' + '\n  '.join(problems))
    print('deb and rpm metadata match the manifest')


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
    """Exactly the verified Arch runtime and data packages; never copy unrelated release inputs into an ISO."""
    found = sorted(directory.glob('*.pkg.tar.zst'))
    patterns = (rf'cupertino-{re.escape(pkgver)}-\d+-x86_64\.pkg\.tar\.zst',
                rf'cupertino-data-{re.escape(pkgver)}-\d+-any\.pkg\.tar\.zst')
    if len(found) != 2 or any(sum(bool(re.fullmatch(pattern, p.name)) for p in found) != 1 for pattern in patterns):
        fail(f'{directory}: expected one runtime and one data package for {pkgver}')
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
        c.add_argument('--format', choices=FORMATS, required=True)
        c.add_argument('--package', choices=MANIFESTS, required=True)
    sub.choices['install'].add_argument('--destdir', required=True)
    sub.choices['install'].add_argument('--bindir', default=str(ROOT / 'target/release'))
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
