#!/usr/bin/env python3
"""Check docs/architecture/contracts.yaml against the source. Usage: python3 ci/contracts.py ROOT.

Checks the registry itself (fields, enums, duplicates, every source and test_owner path exists, every source_match
occurs in its source file, and the test_owner of a socket or a binary names it), then compares it with the code: every
`#[zbus::interface(name = ...)]` outside tests, examples and `#[cfg(test)]` modules, every org.cupertino.* name in the
sources (test modules included) and every binary target of the workspace has an entry. Every crate that builds a
`cdylib` has a `c-library` entry, whose name is the soname its build script links with and whose version is the
MAJOR.MINOR of the ABI header under the crate's abi/. The Wayland globals are compared at run time, by
crates/cupertino-server/tests/contracts.rs."""
import json
from pathlib import Path, PurePosixPath
import re
import subprocess
import sys

REGISTRY = "docs/architecture/contracts.yaml"
KINDS = {"wayland-global", "dbus-interface", "binary", "socket-protocol", "c-library"}
AREAS = {"compositor", "rendering", "design", "apps", "services", "packaging"}
SCOPES = {"public", "shell-private", "system", "unknown"}
COMMON = {"name", "kind", "version", "owner_crate", "owner_area", "scope", "test_owner", "source"}
EXTRA = {
    "wayland-global": {"role", "advertised", "source_match"},
    "dbus-interface": {"bus", "bus_name", "source_match"},
    "binary": {"shipped"},
    "socket-protocol": {"source_match"},
    "c-library": set(),
}
OPTIONAL = {"note", "version_without_render_node", "visible_to"}
# Any attribute with an `interface` token that is not a `key = value` argument (`proxy(interface = "...")`) and not inside
# a string: `zbus::interface`, `::zbus::interface`, an imported `interface`, and either inside a `cfg_attr`.
ATTRIBUTE = re.compile(r'^[ \t]*#\[[^\]"]*?\binterface\b(?!\s*=)(.*?)\]', re.M | re.S)
LITERAL_NAME = re.compile(r'\(\s*name\s*=\s*"([^"]+)"\s*[,)]')
# A module that is compiled only for tests (`#[cfg(test)]` on the line before `mod name {` or `mod name;`, at one
# indentation): its fakes serve nothing.
TEST_ATTRIBUTE = re.compile(r'^([ \t]*)#\[cfg\(test\)\][ \t]*$')
TEST_MODULE = re.compile(r'^([ \t]*)(?:pub(?:\([^)]*\))?[ \t]+)?mod[ \t]+(\w+)[ \t]*([{;])')
# Entries without a test file. It only goes down: give an entry a test_owner and lower this number.
UNKNOWN_TEST_OWNERS = 15
UNKNOWN_SCOPES = 1
# Kinds whose test_owner must name the entry: a socket path, binary name or soname is a string its tests use. Wayland
# globals need not, since crates/cupertino-server/tests/contracts.rs compares them with the registry at run time; D-Bus
# interfaces are reached through typed proxies. A substring heuristic: a comment, or a longer name that contains this
# one (cupertino-dock in cupertino-dock-helper), also passes.
NAMED_BY_TEST = {"socket-protocol", "binary", "c-library"}
NAME = re.compile(r'"(org\.cupertino\.[A-Za-z0-9.]+)"')
# A window's `app_id` is a desktop entry name, not a bus name.
APP_ID = re.compile(r'\bapp_id\b')


def parse(text):
    entries = []
    for number, line in enumerate(text.splitlines(), 1):
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        if line.startswith("- "):
            entries.append({"line": number})
            line = "  " + line[2:]
        match = re.fullmatch(r"  ([a-z_]+): (.+)", line)
        if not match or not entries:
            raise ValueError(f"{REGISTRY}:{number}: not a `key: value` line of an entry: {line!r}")
        entries[-1][match.group(1)] = match.group(2)
    return entries


def sources(root):
    """(path relative to ROOT, text) of every Rust file under a crate's src; tests/ and examples/ are not read."""
    for path in sorted(root.glob("crates/*/src/**/*.rs")):
        yield path.relative_to(root).as_posix(), path.read_text()


def module_dir(path):
    """The directory that holds the files of the child modules of the module in `path`."""
    path = PurePosixPath(path)
    return path.parent if path.name in ("lib.rs", "main.rs", "mod.rs") else path.with_suffix("")


def without_tests(path, text):
    """`text` with each inline `#[cfg(test)] mod` blanked line by line, so the rest keeps its line numbers, and the
    directories the files of all such modules (inline or `mod name;`) are in. Only the exact `#[cfg(test)]` counts:
    `cfg(any(test, ...))` builds outside tests too. A module whose closing brace at its own indentation is not found
    (code that is not rustfmt's) is left as it is."""
    lines = text.splitlines(keepends=True)
    dirs, number = [], 0
    while number < len(lines):
        attribute = TEST_ATTRIBUTE.match(lines[number])
        module = attribute and number + 1 < len(lines) and TEST_MODULE.match(lines[number + 1])
        if not module or module.group(1) != attribute.group(1):
            number += 1
            continue
        dirs.append(module_dir(path) / module.group(2))
        if module.group(3) == ";":
            number += 2
            continue
        close = next((n for n in range(number + 2, len(lines)) if lines[n].rstrip() == module.group(1) + "}"), None)
        if close is None:
            number += 2
            continue
        for n in range(number, close + 1):
            lines[n] = "\n"
        number = close + 1
    return "".join(lines), dirs


def served(root):
    """(path, text) of every source as `check_dbus` reads it for served interfaces: without its test modules, and none
    of the files of a test module."""
    read = [(path, *without_tests(path, text)) for path, text in sources(root)]
    test_dirs = {d for _, _, dirs in read for d in dirs}
    for path, text, _ in read:
        file = PurePosixPath(path)
        if not any(file.with_suffix("") == d or d in file.parents for d in test_dirs):
            yield path, text


def workspace_libraries(root):
    """{crate: path of its manifest relative to root} of every workspace crate that builds a C shared library."""
    meta = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--no-deps", "--format-version", "1", "--offline"], cwd=root))
    return {
        package["name"]: Path(package["manifest_path"]).relative_to(root).as_posix()
        for package in meta["packages"] for target in package["targets"] if "cdylib" in target["crate_types"]
    }


def workspace_binaries(root):
    meta = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--no-deps", "--format-version", "1", "--offline"], cwd=root))
    return {
        target["name"]: (package["name"], Path(target["src_path"]).relative_to(root).as_posix())
        for package in meta["packages"] for target in package["targets"] if "bin" in target["kind"]
    }


def shipped_binaries(root):
    text = (root / "packaging/release.py").read_text()
    apps = re.search(r"^APPS = \(([^)]*)\)", text, re.M).group(1)
    binaries = re.search(r"^BINARIES = \((.*?)^\)", text, re.M | re.S).group(1)
    names = set(re.findall(r"'([a-z-]+)'", binaries.replace("*APPS", apps)))
    return names | set(re.findall(r"'([a-z-]+)'", apps))


def check_entries(entries, root):
    problems, seen = [], set()
    for e in entries:
        where = f"{REGISTRY}:{e['line']}"
        kind = e.get("kind")
        if kind not in KINDS:
            problems.append(f"{where}: kind {kind!r} is not one of {sorted(KINDS)}")
            continue
        allowed = COMMON | EXTRA[kind] | OPTIONAL | {"line"}
        for key in sorted((COMMON | EXTRA[kind]) - e.keys()):
            problems.append(f"{where}: {e.get('name')} has no {key}")
        for key in sorted(e.keys() - allowed):
            problems.append(f"{where}: {e.get('name')} has the unknown field {key}")
        if (kind, e.get("name")) in seen:
            problems.append(f"{where}: {kind} {e.get('name')} is listed twice")
        seen.add((kind, e.get("name")))
        if e.get("owner_area") not in AREAS:
            problems.append(f"{where}: owner_area {e.get('owner_area')!r} is not one of {sorted(AREAS)}")
        if e.get("visible_to", "unsandboxed") != "unsandboxed":
            problems.append(f"{where}: visible_to {e.get('visible_to')!r} is not unsandboxed")
        if e.get("scope") not in SCOPES:
            problems.append(f"{where}: scope {e.get('scope')!r} is not one of {sorted(SCOPES)}")
        if e.get("test_owner", "unknown") != "unknown":
            test = root / e["test_owner"]
            if not test.is_file():
                problems.append(f"{where}: test_owner {e['test_owner']} does not exist")
            elif kind in NAMED_BY_TEST and e.get("name", "") not in test.read_text():
                problems.append(f"{where}: test_owner {e['test_owner']} does not name {e.get('name')}")
        file = root / e.get("source", "")
        if not e.get("source") or not file.is_file():
            problems.append(f"{where}: source {e.get('source')!r} is not an existing file")
        elif e.get("source_match") and e["source_match"] not in file.read_text():
            problems.append(f"{where}: source_match {e['source_match']!r} does not occur in {e['source']}")
    return problems


def interfaces(text):
    """(line, name) of every `interface` attribute in `text`; name is None when it is not a string literal."""
    found = []
    for match in ATTRIBUTE.finditer(text):
        literal = LITERAL_NAME.match(match.group(1))
        found.append((text.count("\n", 0, match.start()) + 1, literal.group(1) if literal else None))
    return found


def check_ratchet(entries):
    scopes = sum(e.get("scope") == "unknown" for e in entries)
    problems = []
    if scopes > UNKNOWN_SCOPES:
        problems.append(f"{REGISTRY}: {scopes} entries have scope unknown, at most {UNKNOWN_SCOPES} are allowed")
    if scopes < UNKNOWN_SCOPES:
        problems.append(f"ci/contracts.py: only {scopes} entries have scope unknown, lower UNKNOWN_SCOPES from "
                        f"{UNKNOWN_SCOPES}")
    unknown = sum(e.get("test_owner") == "unknown" for e in entries)
    if unknown > UNKNOWN_TEST_OWNERS:
        problems.append(f"{REGISTRY}: {unknown} entries have test_owner unknown, at most {UNKNOWN_TEST_OWNERS} are allowed")
    if unknown < UNKNOWN_TEST_OWNERS:
        problems.append(f"ci/contracts.py: only {unknown} entries have test_owner unknown, lower UNKNOWN_TEST_OWNERS from "
                        f"{UNKNOWN_TEST_OWNERS}")
    return problems


def check_dbus(entries, root):
    problems, found = [], set()
    registered = {e["name"]: e for e in entries if e.get("kind") == "dbus-interface"}
    names = set(registered) | {e["bus_name"] for e in registered.values() if e.get("bus_name") != "none"}
    for path, text in served(root):
        for number, name in interfaces(text):
            if name is None:
                problems.append(f"{path}:{number}: unparsed interface attribute: the name must be a string literal")
                continue
            found.add(name)
            entry = registered.get(name)
            if not entry:
                problems.append(f"{path}:{number}: D-Bus interface {name} has no entry in {REGISTRY}")
            elif entry["source"] != path:
                problems.append(f"{path}:{number}: {name} is registered in {entry['source']}")
    # names are checked in test modules too: a fake's bus name is one its tests call
    for path, text in sources(root):
        for number, line in enumerate(text.splitlines(), 1):
            if APP_ID.search(line):
                continue
            for name in NAME.findall(line):
                name = re.sub(r"\.Error.*", "", name)
                if name != "org.cupertino" and not any(name == n or name.startswith(n + ".") for n in names):
                    problems.append(f"{path}:{number}: bus name {name} has no entry in {REGISTRY}")
    for name, entry in registered.items():
        if name not in found:
            problems.append(f"{REGISTRY}:{entry['line']}: {name} is not served by any crate")
    return problems


def check_binaries(entries, root, built=None, shipped=None):
    built = workspace_binaries(root) if built is None else built
    shipped = shipped_binaries(root) if shipped is None else shipped
    problems = []
    registered = {e["name"]: e for e in entries if e.get("kind") == "binary"}
    for name, (crate, source) in sorted(built.items()):
        entry = registered.get(name)
        if not entry:
            problems.append(f"{crate}: binary {name} ({source}) has no entry in {REGISTRY}")
            continue
        if entry["owner_crate"] != crate:
            problems.append(f"{REGISTRY}:{entry['line']}: {name} is built by {crate}, not {entry['owner_crate']}")
        if entry["source"] != source:
            problems.append(f"{REGISTRY}:{entry['line']}: {name} is built from {source}, not {entry['source']}")
        if entry.get("shipped") != ("yes" if name in shipped else "no"):
            problems.append(f"{REGISTRY}:{entry['line']}: {name} shipped is {entry.get('shipped')}, packaging says "
                            f"{'yes' if name in shipped else 'no'}")
    for name, entry in sorted(registered.items()):
        if name not in built:
            problems.append(f"{REGISTRY}:{entry['line']}: binary {name} is not built by any workspace crate")
    for name in sorted(shipped - built.keys()):
        problems.append(f"packaging/release.py ships {name}, which no workspace crate builds")
    return problems


ABI_DEFINE = re.compile(r"^#define \w+_ABI_(MAJOR|MINOR) (\d+)$", re.M)
SONAME = re.compile(r"-soname,([^\s\")]+)")


def abi_header(root, crate):
    """(path, "MAJOR.MINOR") of the header under crates/<crate>/abi/ that defines its ABI version, or None."""
    for header in sorted((root / "crates" / crate / "abi").glob("*.h")):
        version = dict(ABI_DEFINE.findall(header.read_text()))
        if version.keys() == {"MAJOR", "MINOR"}:
            return header.relative_to(root).as_posix(), f"{version['MAJOR']}.{version['MINOR']}"
    return None


def check_libraries(entries, root, libraries=None):
    libraries = workspace_libraries(root) if libraries is None else libraries
    problems = []
    registered = [e for e in entries if e.get("kind") == "c-library"]
    owners = {e.get("owner_crate") for e in registered}
    for crate, manifest in sorted(libraries.items()):
        if crate not in owners:
            problems.append(f"{manifest}: {crate} builds a cdylib without a c-library entry in {REGISTRY}")
    for e in registered:
        where, crate = f"{REGISTRY}:{e['line']}", e.get("owner_crate")
        if crate not in libraries:
            problems.append(f"{where}: {e.get('name')} names {crate}, which builds no cdylib")
            continue
        header = abi_header(root, crate)
        if header is None:
            problems.append(f"{where}: crates/{crate}/abi/ has no header defining its ABI MAJOR and MINOR")
        elif e.get("version") != header[1]:
            problems.append(f"{where}: {e.get('name')} version is {e.get('version')}, {header[0]} says {header[1]}")
        build = root / "crates" / crate / "build.rs"
        sonames = SONAME.findall(build.read_text()) if build.is_file() else []
        if sonames != [e.get("name")]:
            problems.append(f"{where}: {e.get('name')} is not the soname crates/{crate}/build.rs links with "
                            f"({', '.join(sonames) or 'none'})")
    return problems


def check(root, built=None, shipped=None, libraries=None):
    entries = parse((root / REGISTRY).read_text())
    return (check_entries(entries, root) + check_dbus(entries, root) + check_ratchet(entries) +
            check_binaries(entries, root, built, shipped) + check_libraries(entries, root, libraries))


def main(argv):
    if len(argv) != 2:
        print(__doc__.splitlines()[0], file=sys.stderr)
        return 2
    problems = check(Path(argv[1]).resolve())
    for problem in problems:
        print(problem, file=sys.stderr)
    print(f"contracts: {len(problems)} problem(s)")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
