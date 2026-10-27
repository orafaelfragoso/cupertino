#!/usr/bin/env python3
"""Check docs/architecture/contracts.yaml against the source. Usage: python3 ci/contracts.py ROOT.

Checks the registry itself (fields, enums, duplicates, every source and test_owner path exists), then compares it with
the code: every `#[zbus::interface(name = ...)]` outside tests and examples, every org.cupertino.* name in the sources
and every binary target of the workspace has an entry. The Wayland globals are compared at run time, by
crates/cupertino-server/tests/contracts.rs."""
import json
from pathlib import Path
import re
import subprocess
import sys

REGISTRY = "docs/architecture/contracts.yaml"
KINDS = {"wayland-global", "dbus-interface", "binary"}
AREAS = {"compositor", "rendering", "design", "apps", "services", "packaging"}
SCOPES = {"public", "shell-private", "system", "unknown"}
COMMON = {"name", "kind", "version", "owner_crate", "owner_area", "scope", "test_owner", "source"}
EXTRA = {
    "wayland-global": {"role", "advertised"},
    "dbus-interface": {"bus", "bus_name"},
    "binary": {"shipped"},
}
OPTIONAL = {"note"}
ATTRIBUTE = re.compile(r"^[ \t]*#\[\s*(?:cfg_attr\([^,]*,\s*)?(?:zbus\s*::\s*)?interface\b(.*?)\]", re.M | re.S)
LITERAL_NAME = re.compile(r'\(\s*name\s*=\s*"([^"]+)"\s*[,)]')
# Entries without a test file. It only goes down: give an entry a test_owner and lower this number.
UNKNOWN_TEST_OWNERS = 32
NAME = re.compile(r'"(org\.cupertino\.[A-Za-z0-9.]+)"')


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
    """(path relative to ROOT, text) of every Rust file under a crate's src, which has no tests or examples."""
    for path in sorted(root.glob("crates/*/src/**/*.rs")):
        yield path.relative_to(root).as_posix(), path.read_text()


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
        if e.get("scope") not in SCOPES:
            problems.append(f"{where}: scope {e.get('scope')!r} is not one of {sorted(SCOPES)}")
        if e.get("test_owner", "unknown") != "unknown" and not (root / e["test_owner"]).is_file():
            problems.append(f"{where}: test_owner {e['test_owner']} does not exist")
        path, _, line = e.get("source", "").rpartition(":")
        file = root / path
        if not file.is_file() or not line.isdigit() or not 1 <= int(line) <= len(file.read_text().splitlines()):
            problems.append(f"{where}: source {e.get('source')!r} is not path:line of an existing line")
    return problems


def interfaces(text):
    """(line, name) of every `interface` attribute in `text`; name is None when it is not a string literal."""
    found = []
    for match in ATTRIBUTE.finditer(text):
        literal = LITERAL_NAME.match(match.group(1))
        found.append((text.count("\n", 0, match.start()) + 1, literal.group(1) if literal else None))
    return found


def check_ratchet(entries):
    unknown = sum(e.get("test_owner") == "unknown" for e in entries)
    if unknown > UNKNOWN_TEST_OWNERS:
        return [f"{REGISTRY}: {unknown} entries have test_owner unknown, at most {UNKNOWN_TEST_OWNERS} are allowed"]
    if unknown < UNKNOWN_TEST_OWNERS:
        return [f"ci/contracts.py: only {unknown} entries have test_owner unknown, lower UNKNOWN_TEST_OWNERS from "
                f"{UNKNOWN_TEST_OWNERS}"]
    return []


def check_dbus(entries, root):
    problems, found = [], set()
    registered = {e["name"]: e for e in entries if e.get("kind") == "dbus-interface"}
    names = set(registered) | {e["bus_name"] for e in registered.values() if e.get("bus_name") != "none"}
    for path, text in sources(root):
        for number, name in interfaces(text):
            if name is None:
                problems.append(f"{path}:{number}: unparsed interface attribute: the name must be a string literal")
                continue
            found.add(name)
            entry = registered.get(name)
            if not entry:
                problems.append(f"{path}:{number}: D-Bus interface {name} has no entry in {REGISTRY}")
            elif entry["source"] != f"{path}:{number}":
                problems.append(f"{path}:{number}: {name} is registered at {entry['source']}")
        for number, line in enumerate(text.splitlines(), 1):
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
        if not entry["source"].startswith(source + ":"):
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


def check(root, built=None, shipped=None):
    entries = parse((root / REGISTRY).read_text())
    return (check_entries(entries, root) + check_dbus(entries, root) + check_ratchet(entries) + check_binaries(entries, root, built, shipped))


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
