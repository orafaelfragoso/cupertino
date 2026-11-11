#!/usr/bin/env python3
"""Select changed workspace crates and their dependents, or run a check on that selection."""
import argparse
from dataclasses import dataclass
import json
import os
from pathlib import Path
import re
import subprocess
import sys


# Every test that looks up a prepared artifact includes this helper; ci/build-test-fixtures.py supplies the artifacts.
FIXTURE_HELPER = "ci/test-fixtures.rs"
# A lookup that names the package it needs, as in `artifact("cupertino-files", "bin", "cupertino-files")`.
FIXTURE_LOOKUP = re.compile(r'\bartifact\(\s*"([A-Za-z0-9_-]+)"')


@dataclass(frozen=True)
class Fixtures:
    """How a package's sources use prepared artifacts."""
    tests: bool = False  # its sources include the fixture helper
    library: bool = False  # its library includes it, so its dependents' tests look artifacts up through it
    providers: frozenset[str] = frozenset()  # packages whose artifacts its lookups name


@dataclass(frozen=True)
class Package:
    name: str
    directory: str
    dependencies: frozenset[str]
    executable: bool
    fixtures: Fixtures = Fixtures()


def fixture_use(directory: Path) -> Fixtures:
    tests = library = False
    providers: set[str] = set()
    for path in sorted(directory.rglob("*.rs")):
        text = path.read_text(errors="replace")
        if FIXTURE_HELPER in text:
            tests = True
            library = library or path.relative_to(directory).parts[0] == "src"
        providers.update(FIXTURE_LOOKUP.findall(text))
    return Fixtures(tests, library, frozenset(providers))


def workspace() -> list[Package]:
    metadata = json.loads(subprocess.check_output([
        "cargo", "metadata", "--no-deps", "--format-version", "1", "--all-features", "--locked", "--offline",
    ]))
    root = Path(metadata["workspace_root"])
    members = {p["id"] for p in metadata["packages"]} & set(metadata["workspace_members"])
    packages = [p for p in metadata["packages"] if p["id"] in members]
    names = {p["name"] for p in packages}
    paths = {str(Path(p["manifest_path"]).parent): p["name"] for p in packages}
    return [Package(
        p["name"],
        Path(p["manifest_path"]).parent.relative_to(root).as_posix(),
        frozenset(
            paths[d["path"]] if d.get("path") in paths else d["name"]
            for d in p["dependencies"]
            if d.get("path") in paths or d["name"] in names
        ),
        any(set(t["kind"]) & {"bin", "example"} for t in p["targets"]),
        fixture_use(Path(p["manifest_path"]).parent),
    ) for p in packages]


# Agent skills and assistant host configuration, outside every crate.
AGENT_DIRECTORIES = (".agents/", ".claude/", ".codex/")
AGENT_FILES = {".mcp.json"}


def affected(packages: list[Package], changed: list[str]) -> list[str]:
    """Include all dependency kinds, optional features, and compositor runtime integration tests."""
    names = {p.name for p in packages}
    selected: set[str] = set()
    for path in changed:
        if Path(path).name == "Cargo.toml":
            print(f"Full workspace: manifest changed ({path})", file=sys.stderr)
            return sorted(names)
        owner = next((p for p in packages if path.startswith(p.directory + "/")), None)
        if owner is not None:
            # Only the crate's own README is documentation; nested Markdown can be a test fixture.
            if path != owner.directory + "/README.md":
                selected.add(owner.name)
        elif path == "docs/architecture/contracts.yaml":
            selected.add("cupertino-server")
        elif path.startswith(("docs/", ".github/ISSUE_TEMPLATE/")) or ("/" not in path and path.endswith(".md")):
            continue
        elif path.startswith(AGENT_DIRECTORIES) or path in AGENT_FILES:
            # Agent skills and assistant configuration are documentation: no crate builds or tests them.
            continue
        else:
            print(f"Full workspace: shared input changed ({path})", file=sys.stderr)
            return sorted(names)
    dependents = {name: set() for name in names}
    for package in packages:
        for dependency in package.dependencies:
            dependents[dependency].add(package.name)
        # Server integration tests build and launch other crates' binaries and examples through cargo.
        if package.executable and "cupertino-server" in names:
            dependents[package.name].add("cupertino-server")
    # Text tests include the built-in font from the assets crate without a Cargo dependency.
    if {"cupertino-assets", "cupertino-ui-text"} <= names:
        dependents["cupertino-assets"].add("cupertino-ui-text")
    pending = list(selected)
    while pending:
        for dependent in dependents[pending.pop()] - selected:
            selected.add(dependent)
            pending.append(dependent)
    return sorted(selected)


def checks(changed: list[str]) -> dict[str, bool]:
    """Select cheap gates by their inputs; workflow changes exercise every gate."""
    workflow = any(p.startswith((".github/", "ci/")) for p in changed)
    manifests = any(Path(p).name == "Cargo.toml" or p == "Cargo.lock" for p in changed)
    licences = any(Path(p).name.startswith(("LICENSE", "COPYING")) or Path(p).name == "OFL.txt" or
                   p.startswith("packaging/licenses/") for p in changed)
    toolchain = any(p in ("rust-toolchain.toml", "rust-toolchain", ".cargo/config.toml") for p in changed)
    return {
        "docs": workflow or any(p.startswith("docs/") for p in changed),
        "deny": workflow or manifests or licences or "deny.toml" in changed or ".cargo/config.toml" in changed,
        "fmt": workflow or toolchain or manifests or any(p.endswith(".rs") or Path(p).name in
                                                       ("rustfmt.toml", ".rustfmt.toml") for p in changed),
        "layers": workflow or toolchain or manifests or licences or
                  any(p.startswith("crates/cupertino-layers/") for p in changed),
    }


def changed_paths(base: str, head: str) -> list[str]:
    return [p for p in subprocess.check_output([
        "git", "diff", "--name-only", "--no-renames", "-z", f"{base}...{head}",
    ]).decode().split("\0") if p]


def fixture_packages(packages: list[Package], selected: list[str]) -> list[str] | None:
    """The selection with every package whose artifacts its tests look up, or None when no test looks one up.

    Tests and fixture preparation take the same package set, so both builds unify features alike.
    """
    by_name = {package.name: package for package in packages}
    executables = {package.name for package in packages if package.executable}
    expanded = set(selected)
    pending = sorted(expanded)
    consumers = False
    while pending:
        package = by_name.get(pending.pop())
        if package is None:
            continue
        reached, stack = set(), list(package.dependencies)
        while stack:
            dependency = by_name.get(stack.pop())
            if dependency is not None and dependency.name not in reached:
                reached.add(dependency.name)
                stack.extend(dependency.dependencies)
        libraries = [by_name[name] for name in sorted(reached) if by_name[name].fixtures.library]
        if not (package.fixtures.tests or libraries):
            continue
        consumers = True
        wanted = set(package.fixtures.providers).union(*(library.fixtures.providers for library in libraries))
        # These look artifacts up by computed names: the compositor's tests launch every shell component and client,
        # and the transport's compositor tests launch Kit examples.
        if package.name == "cupertino-server":
            wanted |= executables
        if package.name == "cupertino-wayland-client":
            wanted.add("cupertino-kit")
        for name in sorted((wanted & by_name.keys()) - expanded):
            expanded.add(name)
            pending.append(name)
    return sorted(expanded) if consumers else None


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    select = commands.add_parser("select")
    select.add_argument("base")
    select.add_argument("head")
    gates = commands.add_parser("gates")
    gates.add_argument("base")
    gates.add_argument("head")
    commands.add_parser("full")
    run = commands.add_parser("run")
    run.add_argument("check", choices=("clippy", "tests", "doctests"))
    args = parser.parse_args()
    if args.command == "gates":
        print(json.dumps(checks(changed_paths(args.base, args.head)), separators=(",", ":")))
        return 0
    if args.command == "run":
        packages = json.loads(os.environ["CUPERTINO_CI_PACKAGES"])
        if not isinstance(packages, list) or not packages or not all(isinstance(p, str) and p for p in packages):
            parser.error("CUPERTINO_CI_PACKAGES must be a non-empty JSON array of package names")
        prepared = fixture_packages(workspace(), packages) if args.check == "tests" else None
        if prepared is not None:
            packages = prepared
        cargo_checks = {
            "clippy": ["clippy", "--all-targets", "--all-features", "--timings"],
            "tests": ["nextest", "run", "--all-features", "--no-fail-fast", "--timings"],
            "doctests": ["test", "--all-features", "--doc"],
        }
        command = ["cargo", *cargo_checks[args.check]]
        for package in packages:
            command.extend(["-p", package])
        if args.check == "clippy":
            command.extend(["--", "-D", "warnings"])
        elif prepared is not None:
            fixture_options = [option for package in packages for option in ("--package", package)]
            command = [sys.executable, str(Path(__file__).resolve().with_name("build-test-fixtures.py")),
                       "--locked", *fixture_options, "--", *command]
        return subprocess.run(command, check=False).returncode
    packages = workspace()
    if args.command == "full":
        selected = sorted(p.name for p in packages)
    else:
        selected = affected(packages, changed_paths(args.base, args.head))
    print(json.dumps(selected, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    sys.exit(main())
