#!/usr/bin/env python3
"""Cupertino release tool: the one installation manifest behind every package format.

Usage:
  release.py install --format {arch,deb,rpm} --package {runtime,data} --destdir DIR [--bindir DIR]
  release.py verify --format {arch,deb,rpm} --package {runtime,data} ARTIFACT
  release.py check-metadata
  release.py source --output DIR
  release.py verify-tag TAG
  release.py pkgbuild ARCHIVE OUTPUT

install         copies exactly the manifest's files into DIR with explicit modes (Arch package(), portable tarballs).
verify          lists a built package (.pkg.tar.zst, .deb, .rpm) and fails unless its files and modes equal the manifest
                plus the format's own metadata files. It also rejects setuid/setgid bits, wallpapers whose hash differs
                from packaging/data/wallpapers.json and executables that hold a home-directory or checkout path.
check-metadata  compares the deb and rpm asset tables in the crates' Cargo.toml files with the manifest.
source          writes cupertino-<version>-source.tar.xz from the committed HEAD (git archive: file times are the
                commit time, no untracked files).
verify-tag      fails unless TAG is v<version>, points at HEAD, and HEAD is reachable from origin/main.
pkgbuild        writes the stable Arch PKGBUILD with ARCHIVE's sha256 (the release's source archive).

The runtime version is cupertino-server's, the data version cupertino-assets'. Nothing here reads outside the checkout.
"""
import argparse
import hashlib
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
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
FORMATS = ('arch', 'deb', 'rpm')
# The applications among the shipped binaries: each has a desktop entry crates/<app>/data/applications/<app>.desktop.
APPS = ('cupertino-files', 'cupertino-settings')
BINARIES = (
    'cupertino-server',
    'cupertino-dock', 'cupertino-menubar', 'cupertino-launcher', 'cupertino-notifications', 'cupertino-controlcenter',
    'cupertino-osd', 'cupertino-screenshot', 'cupertino-windowmanager', 'cupertino-login',
    *APPS,
    'cupertino-sessiond', 'cupertino-netd', 'cupertino-btd', 'cupertino-powerd',
)
# Besides an app's own entry, the only file an app crate may ship there: the session's default applications.
MIMEAPPS = 'cupertino-mimeapps.list'
PAM_SERVICES = ('cupertino-login', 'cupertino-session', 'cupertino-greeter', 'cupertino-autologin')
UNITS = ('cupertino-login', 'cupertino-netd', 'cupertino-btd', 'cupertino-powerd')
THEME_FILES = ('tokens.json', 'materials.json', 'animations.json')
WALLPAPERS = json.loads((ROOT / 'packaging/data/wallpapers.json').read_text())['files']
NAMES = {'runtime': 'cupertino', 'data': 'cupertino-data'}


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
        ('usr/lib/sysusers.d/cupertino-greeter.conf', 0o644, False),
        ('usr/share/dbus-1/system.d/org.cupertino.conf', 0o644, False),
        ('etc/cupertino/login.conf', 0o644, True),
        ('etc/cupertino/greeter.json', 0o644, True),
    )]
    # The shipped apps' desktop entries (the menu bar and the dock take an app's name and icon from it) and
    # cupertino-mimeapps.list; applications() refuses anything else.
    try:
        apps = applications(ROOT, APPS)
    except ValueError as e:
        fail(f'crates/*/data/applications:\n  {e}')
    files += [(f'usr/share/applications/{a.name}', a.relative_to(ROOT).as_posix(), 0o644, False) for a in apps]
    files += [(f'usr/lib/systemd/system/{u}.service', f'{f}/usr/lib/systemd/system/{u}.service', 0o644, False)
              for u in UNITS]
    files += [(f'usr/share/cupertino/theme/{n}', f'data/theme/{n}', 0o644, False) for n in THEME_FILES]
    files += [(f'etc/pam.d/{s}', f'packaging/pam/{fmt}/{s}', 0o644, True) for s in PAM_SERVICES]
    if fmt != 'deb':  # cargo-deb writes usr/share/doc/cupertino/copyright from the same file
        files.append(('usr/share/licenses/cupertino/LICENSE', 'LICENSE', 0o644, False))
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


def cmd_check_metadata(_args):
    problems = []
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


def cmd_source(args):
    v = release_version(args.tag)
    out = Path(args.output)
    out.mkdir(parents=True, exist_ok=True)
    tar = subprocess.run(['git', '-C', str(ROOT), 'archive', '--format=tar', f'--prefix=cupertino-{v}/', 'HEAD'],
                         capture_output=True, check=True).stdout
    dest = out / f'cupertino-{v}-source.tar.xz'
    dest.write_bytes(lzma.compress(tar, preset=9))
    print(dest)


def cmd_pkgbuild(args):
    """Write the stable PKGBUILD with the release source archive's checksum in place of UNRELEASED."""
    text = (ROOT / 'packaging/arch/cupertino/PKGBUILD').read_text()
    if "sha256sums=('UNRELEASED')" not in text:
        fail('packaging/arch/cupertino/PKGBUILD has no UNRELEASED checksum to fill')
    digest = hashlib.sha256(Path(args.archive).read_bytes()).hexdigest()
    text = text.replace("sha256sums=('UNRELEASED')", f"sha256sums=('{digest}')")
    if args.tag:
        text = text.replace(f'pkgver={version("cupertino-server")}\n', f'pkgver={release_version(args.tag)}\n', 1)
        text = text.replace('_tag=v$pkgver\n', f'_tag={args.tag}\n', 1)
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
    if git('rev-list', '-n1', args.tag) != git('rev-parse', 'HEAD'):
        fail(f'{args.tag} does not point at HEAD')
    if subprocess.run(['git', '-C', str(ROOT), 'merge-base', '--is-ancestor', 'HEAD', 'origin/main']).returncode:
        fail('HEAD is not reachable from origin/main')
    print(f'{args.tag}: version {v}, reachable from origin/main')


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
    c = sub.add_parser('source')
    c.add_argument('--output', required=True)
    c.add_argument('--tag')
    sub.add_parser('verify-tag').add_argument('tag')
    k = sub.add_parser('pkgbuild')
    k.add_argument('--tag')
    k.add_argument('archive')
    k.add_argument('output')
    args = p.parse_args()
    {'install': cmd_install, 'verify': cmd_verify, 'check-metadata': cmd_check_metadata, 'source': cmd_source,
     'verify-tag': cmd_verify_tag, 'pkgbuild': cmd_pkgbuild}[args.command](args)


if __name__ == '__main__':
    main()
