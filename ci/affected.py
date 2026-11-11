#!/usr/bin/env python3
"""Select changed workspace crates and their dependents, or run a check on that selection."""
import argparse
from dataclasses import dataclass
import json
import os
from pathlib import Path
import subprocess
import sys


@dataclass(frozen=True)
class Package:
    name: str
    directory: str
    dependencies: frozenset[str]
    executable: bool


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


def fixture_packages(packages: list[Package], selected: list[str]) -> list[str]:
    """Match the test package graph to the executables its integration fixtures launch."""
    expanded = set(selected)
    if "cupertino-server" in expanded:
        expanded.update(package.name for package in packages if package.executable)
    if "cupertino-wayland-client" in expanded:
        expanded.add("cupertino-kit")
    return sorted(expanded)


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
        prepare_fixtures = args.check == "tests" and bool(
            set(packages) & {"cupertino-server", "cupertino-wayland-client", "cupertino-ui-abi"})
        if prepare_fixtures:
            packages = fixture_packages(workspace(), packages)
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
        elif prepare_fixtures:
            fixture_options = [option for package in packages for option in ("--package", package)]
            command = [sys.executable, str(Path(__file__).resolve().with_name("build-test-fixtures.py")),
                       "--locked", *fixture_options, "--", *command]
        lease = os.environ.get("CUPERTINO_BUILD_LOCK_FD")
        inherited = () if lease is None else tuple(map(int, lease.split(",")))
        for descriptor in inherited:
            os.fstat(descriptor)
        return subprocess.run(command, check=False, pass_fds=inherited).returncode
    packages = workspace()
    if args.command == "full":
        selected = sorted(p.name for p in packages)
    else:
        selected = affected(packages, changed_paths(args.base, args.head))
    print(json.dumps(selected, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    sys.exit(main())
