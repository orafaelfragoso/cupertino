#!/usr/bin/env python3
"""Keep Cargo targets private to a worktree and serialize expensive local builds.

Run from the worktree root. All cooperating worktrees use one per-user lock;
the child inherits it, so killing this wrapper does not release a running build.
Seeds are explicitly promoted snapshots, never arbitrary target directories.
"""
import argparse
import contextlib
import fcntl
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import signal
import subprocess
import sys
import time
import tomllib
from collections.abc import Iterator
from typing import TypedDict, cast

# Reserve room for the desktop and other work before starting a protected operation.
DISK_FLOOR = 30 * 1024**3
# Bound immutable snapshots independently of the optional compiler cache.
SEED_LIMIT = 1
SEED_AGE = 7 * 24 * 60 * 60
MARKER = ".cupertino-build.json"
# Match compiler/build-tool inputs without tying snapshots to a terminal, credential or agent session.
BUILD_ENV_NAMES = frozenset({
    "PATH", "HOME", "CC", "CXX", "CPP", "AS", "AR", "LD", "NM", "RANLIB", "STRIP", "OBJCOPY", "OBJDUMP",
    "CFLAGS", "CXXFLAGS", "CPPFLAGS", "ASFLAGS", "ARFLAGS", "LDFLAGS", "LIBRARY_PATH", "LD_LIBRARY_PATH",
    "CPATH", "C_INCLUDE_PATH", "CPLUS_INCLUDE_PATH", "LIBCLANG_PATH", "CLANG_PATH", "LLVM_CONFIG_PATH",
    "MAKE", "MAKEFLAGS", "MFLAGS", "CMAKE", "HOST", "TARGET", "OPT_LEVEL", "DEBUG", "PROFILE", "NUM_JOBS",
})
BUILD_ENV_PREFIXES = (
    "CARGO_", "RUST", "CUPERTINO_", "SCCACHE_", "PKG_CONFIG", "BINDGEN_", "CMAKE_", "LLVM_", "CLANG_",
    "CC_", "CXX_", "CPP_", "AS_", "AR_", "LD_", "NM_", "RANLIB_", "STRIP_",
    "CFLAGS_", "CXXFLAGS_", "CPPFLAGS_", "ASFLAGS_", "ARFLAGS_", "LDFLAGS_", "HOST_", "TARGET_",
)


class Metadata(TypedDict):
    schema: int
    owner: str
    fingerprint: str
    successful: bool
    created: float


def output(command: list[str], root: Path) -> str:
    return subprocess.check_output(command, cwd=root, text=True).strip()


def repository(root: Path, allow_primary: bool) -> Path:
    if output(["git", "rev-parse", "--show-toplevel"], root) != str(root):
        raise ValueError("run from the worktree root")
    common = Path(output(["git", "rev-parse", "--path-format=absolute", "--git-common-dir"], root))
    if root == common.parent and not allow_primary:
        raise ValueError("use a linked worktree, not the main checkout")
    return common


@contextlib.contextmanager
def exclusive(path: Path) -> Iterator[int]:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("a+") as lease:
        print(f"worktree-build: waiting for {path}", flush=True)
        fcntl.flock(lease, fcntl.LOCK_EX)
        yield lease.fileno()


def disk_room(path: Path) -> None:
    free = shutil.disk_usage(path).free
    if free < DISK_FLOOR:
        raise ValueError(f"{path}: {free / 1024**3:.1f} GiB free; require {DISK_FLOOR / 1024**3:.0f} GiB")


def fingerprint(root: Path, profile: str, environ: dict[str, str]) -> str:
    """Conservatively match host, toolchain, profiles, config and known build environment inputs.

    Cargo still validates source/dependency freshness after a snapshot is cloned.
    Hash values rather than recording potentially private environment contents.
    """
    tracked = output(["git", "ls-files", "-z"], root).split("\0")
    inputs = {name: (root / name).read_text() for name in tracked
              if name and (Path(name).name in {"Cargo.toml", "Cargo.lock", "rust-toolchain", "rust-toolchain.toml"}
                           or name.startswith(".cargo/"))}
    cargo_home = Path(environ.get("CARGO_HOME", str(Path.home() / ".cargo")))
    for directory in [cargo_home, root / ".cargo", *[parent / ".cargo" for parent in root.parents]]:
        for name in ("config", "config.toml"):
            config = directory / name
            if config.exists():
                name = config.relative_to(root).as_posix() if config.is_relative_to(root) else str(config)
                inputs[name] = config.read_text()
    for name, contents in inputs.items():
        if Path(name).name in {"config", "config.toml"}:
            configuration = tomllib.loads(contents)
            if "build-dir" in configuration.get("build", {}) or "include" in configuration:
                raise ValueError(f"{name}: external build directories and config includes are unsupported")
    environment = {key: value for key, value in environ.items()
                   if (key in BUILD_ENV_NAMES or key.startswith(BUILD_ENV_PREFIXES))
                   and key not in {"CARGO_TARGET_DIR", "CUPERTINO_BUILD_LOCK_FD"}
                   and not key.startswith("CUPERTINO_TEST_FIXTURE_")}
    environment["CARGO_INCREMENTAL"] = "0"
    identity = {"schema": 1, "profile": profile, "environment": environment, "inputs": inputs,
                "rustc": output([environ.get("RUSTC", "rustc"), "-vV"], root), "cargo": output(["cargo", "-vV"], root),
                "host": platform.platform(), "os": Path("/etc/os-release").read_text()}
    return hashlib.sha256(json.dumps(identity, sort_keys=True).encode()).hexdigest()


def read_marker(directory: Path, owner: str) -> Metadata:
    if directory.is_symlink() or not directory.is_dir() or (directory / MARKER).is_symlink():
        raise ValueError(f"{directory}: refusing an unmanaged directory or symlink")
    metadata = json.loads((directory / MARKER).read_text())
    if not isinstance(metadata, dict) or metadata.get("owner") != owner or metadata.get("schema") != 1:
        raise ValueError(f"{directory}: ownership marker does not match")
    if not isinstance(metadata.get("fingerprint"), str) or type(metadata.get("successful")) is not bool \
            or not isinstance(metadata.get("created"), (int, float)):
        raise ValueError(f"{directory}: incomplete ownership marker")
    return cast(Metadata, metadata)


def write_marker(directory: Path, metadata: Metadata) -> None:
    staging = directory / (MARKER + ".new")
    with staging.open("x") as stream:
        json.dump(metadata, stream, sort_keys=True)
    staging.replace(directory / MARKER)


def group_running(group: int) -> bool:
    """Ignore exited zombies when checking our Linux child process group."""
    for entry in Path("/proc").iterdir():
        if not entry.name.isdecimal():
            continue
        try:
            fields = (entry / "stat").read_text().rsplit(")", 1)[1].split()
        except (FileNotFoundError, ProcessLookupError):
            # Processes can exit between the directory listing and this read.
            continue
        if int(fields[2]) == group and fields[0] != "Z":
            return True
    return False


def child(command: list[str], root: Path, environ: dict[str, str], lease: int) -> int:
    """Keep the resource lease until the whole child process group has stopped."""
    child_environment = {**environ, "CUPERTINO_BUILD_LOCK_FD": str(lease)}
    process = subprocess.Popen(command, cwd=root, env=child_environment, pass_fds=(lease,), start_new_session=True)
    try:
        return process.wait()
    finally:
        # An intermediary may exit before a compiler/test descendant and close its inherited lease.
        # Stop every remaining group member before the caller can release the resource guard.
        with contextlib.suppress(ProcessLookupError):
            os.killpg(process.pid, signal.SIGTERM)
        deadline = time.monotonic() + 10
        while group_running(process.pid):
            process.poll()
            if time.monotonic() >= deadline:
                with contextlib.suppress(ProcessLookupError):
                    os.killpg(process.pid, signal.SIGKILL)
            time.sleep(0.05)
        process.wait()


def clone(source: Path, destination: Path, root: Path, lease: int) -> None:
    # Container/package targets have their own toolchain and never enter a host seed.
    entries = [str(path) for path in source.iterdir() if path.name != "package-build"]
    if not entries:
        raise ValueError(f"{source}: seed source is empty")
    destination.mkdir()
    # No copy fallback: an unsupported filesystem must fail visibly before a build starts.
    try:
        status = child(["cp", "--archive", "--reflink=always", *entries, str(destination)],
                       root, dict(os.environ), lease)
        if status:
            raise ValueError(f"reflink clone failed with exit status {status}")
    except BaseException:
        shutil.rmtree(destination)
        raise


def seed_directory(state: Path) -> Path:
    seeds = state / "seeds"
    if state.is_symlink() or seeds.is_symlink():
        raise ValueError(f"{state}: refusing a symlinked cache directory")
    return seeds


def prepare(root: Path, state: Path, identity: str, seed: str | None, lease: int) -> Metadata:
    target = root / "target"
    if target.exists() or target.is_symlink():
        metadata = read_marker(target, str(root))
        if metadata.get("fingerprint") != identity:
            raise ValueError("target build inputs changed; cleanup --target, then prepare again")
        if seed is not None:
            raise ValueError("a seed can only be applied to an absent target")
        return metadata
    disk_room(root)
    if seed is None:
        target.mkdir()
    else:
        source = seed_directory(state) / seed
        metadata = read_marker(source, str(state))
        if metadata.get("fingerprint") != identity:
            raise ValueError(f"seed {seed}: incompatible toolchain, profile, configuration or environment")
        if time.time() - metadata["created"] >= SEED_AGE:
            raise ValueError(f"seed {seed}: expired; cleanup --expired-seeds")
        clone(source, target, root, lease)
    metadata = Metadata(schema=1, owner=str(root), fingerprint=identity, successful=False, created=time.time())
    write_marker(target, metadata)
    print(f"worktree-build: prepared {target}", flush=True)
    return metadata


def cargo_command(command: list[str], profile: str) -> list[str]:
    if len(command) >= 3 and Path(command[0]).name.startswith("python3") and command[1] == "ci/build-test-fixtures.py":
        if "--" not in command[2:]:
            raise ValueError("fixture wrapper requires -- followed by a Cargo command")
        split = command.index("--")
        if any(arg == "--target-dir" or arg.startswith("--target-dir=") for arg in command[2:split]):
            raise ValueError("fixture target directory is managed by this wrapper")
        if any(arg.startswith(("--profile", "--release")) for arg in command[2:split]):
            raise ValueError("fixture profile is set by --profile on this wrapper")
        return command[:2] + ["--profile", profile] + command[2:split + 1] + cargo_command(command[split + 1:], profile)
    if len(command) < 2 or command[0] != "cargo" or command[1] not in {"build", "check", "clippy", "test", "run", "nextest"}:
        raise ValueError("run requires cargo build/check/clippy/test/run or cargo nextest run")
    end = command.index("--") if "--" in command else len(command)
    flags = command[:end]
    if any(arg in {"--release", "-r", "--profile", "--cargo-profile", "--target-dir", "--config"}
           or arg.startswith(("--profile=", "--cargo-profile=", "--target-dir=", "--config=")) for arg in flags):
        raise ValueError("profile is set by --profile on this wrapper; target/config overrides are not allowed")
    if command[1] == "nextest":
        if len(command) < 3 or command[2] != "run":
            raise ValueError("only cargo nextest run is supported")
        insertion, option = 3, "--cargo-profile"
    else:
        insertion, option = 2, "--profile"
    return command[:insertion] + [option, profile] + command[insertion:]


def promote(root: Path, state: Path, identity: str, name: str, max_seed_gib: int, lease: int) -> None:
    target = root / "target"
    metadata = read_marker(target, str(root))
    if metadata.get("fingerprint") != identity or metadata.get("successful") is not True:
        raise ValueError("only a target from a successful run with matching inputs can be promoted")
    seeds = seed_directory(state)
    seeds.mkdir(parents=True, exist_ok=True)
    if len(list(seeds.iterdir())) >= SEED_LIMIT:
        raise ValueError(f"at most {SEED_LIMIT} seeds; cleanup --expired-seeds before promoting")
    size = sum(path.lstat().st_size for path in target.rglob("*")
               if path.is_file() and path.relative_to(target).parts[0] != "package-build")
    if max_seed_gib <= 0 or size > max_seed_gib * 1024**3:
        raise ValueError(f"target exceeds the positive --max-seed-gib budget ({max_seed_gib})")
    disk_room(seeds)
    destination = seeds / name
    clone(target, destination, root, lease)
    write_marker(destination, Metadata(schema=1, owner=str(state), fingerprint=identity, successful=True, created=time.time()))
    print(f"worktree-build: promoted {destination}", flush=True)


def cleanup(root: Path, state: Path, target: bool, expired: bool) -> None:
    if target:
        directory = root / "target"
        read_marker(directory, str(root))
        shutil.rmtree(directory)
        print(f"worktree-build: removed {directory}", flush=True)
    seeds = seed_directory(state) if expired else None
    if seeds is not None and seeds.exists():
        for directory in seeds.iterdir():
            metadata = read_marker(directory, str(state))
            if time.time() - metadata["created"] >= SEED_AGE:
                shutil.rmtree(directory)
                print(f"worktree-build: removed expired seed {directory.name}", flush=True)


def interrupted(signum: int, frame: object) -> None:
    raise InterruptedError(f"received signal {signum}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="action", required=True)
    for action in ("prepare", "run", "promote"):
        sub = commands.add_parser(action)
        sub.add_argument("--profile", required=True)
        if action == "prepare":
            sub.add_argument("--seed")
        elif action == "run":
            sub.add_argument("command", nargs=argparse.REMAINDER)
        else:
            sub.add_argument("--name", required=True)
            sub.add_argument("--max-seed-gib", type=int, required=True)
    locked = commands.add_parser("locked", help="hold the disk/resource guard around any command; preserve its target layout")
    locked.add_argument("command", nargs=argparse.REMAINDER)
    clean = commands.add_parser("cleanup")
    clean.add_argument("--target", action="store_true")
    clean.add_argument("--expired-seeds", action="store_true")
    args = parser.parse_args()
    for name in (getattr(args, "seed", None), getattr(args, "name", None)):
        if name is not None and re.fullmatch(r"[a-z0-9][a-z0-9-]{0,63}", name) is None:
            parser.error("seed names must be 1–64 lowercase letters, digits or hyphens")
    root = Path.cwd().resolve()
    state = repository(root, args.action == "locked") / "cupertino-build"
    environ = dict(os.environ)
    if "CUPERTINO_BUILD_LOCK_FD" in environ:
        raise ValueError("nested worktree-build invocations would deadlock; wrap the outermost command only")
    lock = Path.home() / ".cache" / "cupertino-build" / "heavy.lock"
    if args.action == "locked":
        command = args.command[1:] if args.command[:1] == ["--"] else args.command
        if not command:
            parser.error("locked requires a command")
        with exclusive(lock) as lease:
            disk_room(root)
            if "CARGO_TARGET_DIR" in environ:
                destination = Path(environ["CARGO_TARGET_DIR"]).resolve()
                while not destination.exists():
                    destination = destination.parent
                disk_room(destination)
            status = child(command, root, {**environ, "CARGO_INCREMENTAL": "0"}, lease)
            return status if status >= 0 else 128 - status
    if any(name in environ for name in ("CARGO_BUILD_BUILD_DIR", "CARGO_BUILD_TARGET_DIR")):
        raise ValueError("remove Cargo build/target directory overrides")
    if "CARGO_TARGET_DIR" in environ and Path(environ["CARGO_TARGET_DIR"]).resolve() != root / "target":
        raise ValueError("CARGO_TARGET_DIR must be this worktree's target directory")
    environ.update(CARGO_TARGET_DIR=str(root / "target"), CARGO_INCREMENTAL="0")
    with exclusive(lock) as lease:
        if args.action == "cleanup":
            if not args.target and not args.expired_seeds:
                parser.error("cleanup requires --target or --expired-seeds")
            cleanup(root, state, args.target, args.expired_seeds)
            return 0
        identity = fingerprint(root, args.profile, environ)
        if args.action == "prepare":
            prepare(root, state, identity, args.seed, lease)
        elif args.action == "promote":
            promote(root, state, identity, args.name, args.max_seed_gib, lease)
        else:
            command = args.command[1:] if args.command[:1] == ["--"] else args.command
            command = cargo_command(command, args.profile)
            metadata = prepare(root, state, identity, None, lease)
            disk_room(root)
            metadata["successful"] = False
            write_marker(root / "target", metadata)
            status = child(command, root, environ, lease)
            metadata["successful"] = status == 0
            write_marker(root / "target", metadata)
            return status if status >= 0 else 128 - status
    return 0


if __name__ == "__main__":
    signal.signal(signal.SIGTERM, interrupted)
    try:
        sys.exit(main())
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        print(f"worktree-build: {error}", file=sys.stderr)
        sys.exit(1)
