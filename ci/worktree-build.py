#!/usr/bin/env python3
"""Keep Cargo targets private to a worktree and bound how many expensive local builds run at once.

Run from the worktree root. All cooperating worktrees share per-user build slots: `run`, a plain
`prepare` and `cleanup --target` take one of N slots (--slots or CUPERTINO_BUILD_SLOTS, default 4)
and give their child about 1/N of the processors; seed operations and `locked` take every slot, and
`locked --hold one|none` lets a package script take them per phase. One operation at a time uses a
worktree's target. The child inherits its locks, so killing this wrapper does not release a running build.
Seeds are explicitly promoted snapshots, never arbitrary target directories; `refresh-seed` replaces one in place.
"""
import argparse
import contextlib
import ctypes
import errno
import fcntl
import functools
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import signal
import stat
import subprocess
import sys
import time
import tomllib
from collections.abc import Callable, Iterator
from typing import TypedDict, cast

# Reserve room for the desktop and other work before starting a protected operation.
DISK_FLOOR = 30 * 1024**3
# Bound immutable snapshots independently of the optional compiler cache.
SEED_LIMIT = 1
SEED_AGE = 7 * 24 * 60 * 60
MARKER = ".cupertino-build.json"
# The marker and fingerprint layout. Schema 1 also hashed every manifest and Cargo.lock; its markers are still accepted
# while their inputs are exactly unchanged (legacy_fingerprint), and refused with "re-prepare" otherwise.
SCHEMA = 2
# A seed being written or replaced lives under this prefix, which no seed name can start with.
STAGING = "."
# How many shared operations run at once unless --slots or the variable below says otherwise. With 16 processors each
# slot's child gets 4 jobs, so at most 16 compilers or linkers run at once (CONTRIBUTING.md has the memory budget).
DEFAULT_SLOTS = 4
SLOTS_VARIABLE = "CUPERTINO_BUILD_SLOTS"
# An exclusive operation takes every slot up to this bound, whatever N each caller chose.
MAX_SLOTS = 16
# How often a waiter checks the queue and the slots again, in seconds.
POLL = 0.2
# A waiter's place in the queue: its arrival time in nanoseconds, its process and what it waits for.
TICKET = re.compile(r"[0-9]{20}-[0-9]+-(whole|shared)")
# Parallelism caps given to a shared operation's child: they change how fast it runs, never what it produces.
JOB_VARIABLES = ("CARGO_BUILD_JOBS", "NEXTEST_TEST_THREADS", "RUST_TEST_THREADS")
UNFINGERPRINTED = frozenset({"CARGO_TARGET_DIR", "CUPERTINO_BUILD_LOCK_FD", SLOTS_VARIABLE, *JOB_VARIABLES})
# The build environment in the fingerprint: variables the compiler, Cargo, a build script or a compile-time env!/option_env!
# reads. A variable only a test or a running program reads (CUPERTINO_UPDATE_GOLDENS, CUPERTINO_REPAINT_*,
# CUPERTINO_SPEC_DIR, RUST_LOG, RUST_BACKTRACE, ...) is a runtime input and stays out, so setting it never refuses a seed
# or a target. The project's own compile-time variables are listed by name; test-worktree-build.py fails when a build
# script or env!/option_env! reads a CUPERTINO_ variable missing here.
PROJECT_BUILD_VARIABLES = frozenset({
    "CUPERTINO_THEME_DIR", "CUPERTINO_INSTALL_PREFIX", "CUPERTINO_BUILD_COMMIT", "CUPERTINO_ADMIN_GROUPS",
})
BUILD_ENV_NAMES = frozenset({
    "PATH", "HOME", "CC", "CXX", "CPP", "AS", "AR", "LD", "NM", "RANLIB", "STRIP", "OBJCOPY", "OBJDUMP",
    "CFLAGS", "CXXFLAGS", "CPPFLAGS", "ASFLAGS", "ARFLAGS", "LDFLAGS", "LIBRARY_PATH", "LD_LIBRARY_PATH",
    "CPATH", "C_INCLUDE_PATH", "CPLUS_INCLUDE_PATH", "LIBCLANG_PATH", "CLANG_PATH", "LLVM_CONFIG_PATH",
    "MAKE", "MAKEFLAGS", "MFLAGS", "CMAKE", "HOST", "TARGET", "OPT_LEVEL", "DEBUG", "PROFILE", "NUM_JOBS",
    *PROJECT_BUILD_VARIABLES,
})
BUILD_ENV_PREFIXES = (
    "CARGO_", "RUSTC", "RUSTDOC", "RUSTFLAGS", "RUSTUP_", "SCCACHE_", "PKG_CONFIG", "BINDGEN_", "CMAKE_", "LLVM_",
    "CLANG_", "CC_", "CXX_", "CPP_", "AS_", "AR_", "LD_", "NM_", "RANLIB_", "STRIP_",
    "CFLAGS_", "CXXFLAGS_", "CPPFLAGS_", "ASFLAGS_", "ARFLAGS_", "LDFLAGS_", "HOST_", "TARGET_",
)
# Cargo variables that change only its terminal output.
RUNTIME_PREFIXES = ("CARGO_TERM_",)
# Schema 1's environment scope, kept only to recognise its markers (legacy_fingerprint).
LEGACY_ENV_PREFIXES = (
    "CARGO_", "RUST", "CUPERTINO_", "SCCACHE_", "PKG_CONFIG", "BINDGEN_", "CMAKE_", "LLVM_", "CLANG_",
    "CC_", "CXX_", "CPP_", "AS_", "AR_", "LD_", "NM_", "RANLIB_", "STRIP_",
    "CFLAGS_", "CXXFLAGS_", "CPPFLAGS_", "ASFLAGS_", "ARFLAGS_", "LDFLAGS_", "HOST_", "TARGET_",
)
# What a `locked` child holds: "every" slot, "one", or "none" (the worktree lease alone). Only a child holding none may
# call `locked` again, from the same worktree, to take slots for one phase.
HOLD_VARIABLE = "CUPERTINO_BUILD_LOCK_HOLD"
HOLDS = ("every", "one", "none")
# Swap two directory entries in one step (renameat2), so a seed's name always holds a whole seed.
AT_FDCWD = -100
RENAME_EXCHANGE = 2
# What renameat2 reports when a filesystem or kernel cannot swap; promotion then renames twice (replace_seed).
UNSWAPPABLE = frozenset({errno.EINVAL, errno.ENOSYS, errno.EOPNOTSUPP})
# A seed name: it never starts with STAGING and never leaves the seeds directory.
SEED_NAME = re.compile(r"[A-Za-z0-9][A-Za-z0-9_.-]*")


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


def slot_path(directory: Path, slot: int) -> Path:
    # Slot 0 keeps the single lock's name, so an older copy of this script holding it counts as one slot.
    return directory / ("heavy.lock" if slot == 0 else f"heavy-{slot}.lock")


@contextlib.contextmanager
def ticket(directory: Path, whole: bool) -> Iterator[Path]:
    """Hold a place in the per-user queue: a file named by arrival time, locked while its owner waits."""
    queue = directory / "queue"
    queue.mkdir(mode=0o700, parents=True, exist_ok=True)
    if queue.is_symlink():
        raise ValueError(f"{queue}: refusing a symlinked queue directory")
    name = f"{time.time_ns():020d}-{os.getpid()}-{'whole' if whole else 'shared'}"
    # Lock before the name appears, so a peer never mistakes a new ticket for one left by a killed waiter.
    staging = queue / f".{name}"
    descriptor = os.open(staging, os.O_CREAT | os.O_EXCL | os.O_WRONLY | os.O_CLOEXEC, 0o600)
    try:
        fcntl.flock(descriptor, fcntl.LOCK_EX)
        staging.rename(queue / name)
        yield queue / name
    finally:
        (queue / name).unlink(missing_ok=True)
        staging.unlink(missing_ok=True)
        os.close(descriptor)


def ahead(own: Path) -> tuple[int, bool]:
    """Count the live waiters that arrived before `own`, and whether one wants every slot.

    Tickets of waiters that were killed are removed on the way. Anything else in the queue, such as a link, a pipe
    or a directory, is neither opened through nor waited for.
    """
    count, whole = 0, False
    for entry in sorted(own.parent.iterdir()):
        if TICKET.fullmatch(entry.name) is None:
            continue
        if entry.name >= own.name:
            break
        try:
            descriptor = os.open(entry, os.O_RDONLY | os.O_CLOEXEC | os.O_NOFOLLOW | os.O_NONBLOCK)
        except OSError:
            # gone, or a link
            continue
        try:
            if not stat.S_ISREG(os.fstat(descriptor).st_mode):
                continue
            fcntl.flock(descriptor, fcntl.LOCK_SH | fcntl.LOCK_NB)
        except BlockingIOError:
            count += 1
            whole = whole or entry.name.endswith("-whole")
        else:
            entry.unlink(missing_ok=True)
        finally:
            os.close(descriptor)
    return count, whole


def try_lock(path: Path, held: contextlib.ExitStack, block: bool = False) -> int | None:
    lease = path.open("a+")
    try:
        fcntl.flock(lease, fcntl.LOCK_EX | (0 if block else fcntl.LOCK_NB))
    except BlockingIOError:
        lease.close()
        return None
    except BaseException:
        lease.close()
        raise
    return held.enter_context(lease).fileno()


def take_slot(directory: Path, slots: int, skip: int, held: contextlib.ExitStack) -> tuple[int, int] | None:
    """Take a free slot after leaving `skip` free ones to the waiters ahead; return its number and lease."""
    for slot in range(slots):
        with contextlib.ExitStack() as probe:
            lease = try_lock(slot_path(directory, slot), probe)
            if lease is None:
                continue
            if skip:
                skip -= 1
                continue
            held.enter_context(probe.pop_all())
            return slot, lease
    return None


@contextlib.contextmanager
def acquire(directory: Path, slots: int, whole: bool) -> Iterator[tuple[int, ...]]:
    """Take one of `slots` build slots, or every slot when `whole`, in arrival order.

    Waiters queue first come, first served, so nobody starves: a shared waiter takes a free slot only
    when enough remain for the shared waiters ahead of it, and never passes a waiting exclusive
    operation. That one keeps its place at the head while it takes slot 0, 1, ... in order, so no
    new shared operation starts meanwhile and two exclusive operations cannot deadlock.
    """
    directory.mkdir(parents=True, exist_ok=True)
    wanted = "every build slot" if whole else f"one of {slots} build slots"
    waiting = False

    def wait() -> None:
        nonlocal waiting
        if not waiting:
            print(f"worktree-build: waiting for {wanted} in {directory}", flush=True)
            waiting = True

    with contextlib.ExitStack() as held:
        leases: list[int] = []
        with ticket(directory, whole) as own:
            while True:
                count, exclusive_ahead = ahead(own)
                if whole and not count:
                    for slot in range(MAX_SLOTS):
                        lease = try_lock(slot_path(directory, slot), held)
                        if lease is None:
                            wait()
                            lease = try_lock(slot_path(directory, slot), held, block=True)
                        leases.append(cast(int, lease))
                    break
                if not whole and not exclusive_ahead:
                    taken = take_slot(directory, slots, count, held)
                    if taken is not None:
                        granted, lease = taken
                        leases.append(lease)
                        break
                wait()
                time.sleep(POLL)
        if whole:
            print(f"worktree-build: holding every build slot (0–{MAX_SLOTS - 1})", flush=True)
        else:
            print(f"worktree-build: holding build slot {granted} of {slots} ({slot_path(directory, granted).name})",
                  flush=True)
        yield tuple(leases)


@contextlib.contextmanager
def worktree_lease(directory: Path, root: Path) -> Iterator[int]:
    """Hold this worktree's own lock, so two operations never use its target at once.

    It is taken before the slot queue and released after the slots, in that order everywhere, so it adds no deadlock.
    """
    worktrees = directory / "worktrees"
    worktrees.mkdir(parents=True, exist_ok=True)
    path = worktrees / f"{hashlib.sha256(str(root).encode()).hexdigest()[:32]}.lock"
    with contextlib.ExitStack() as held:
        lease = try_lock(path, held)
        if lease is None:
            print(f"worktree-build: waiting for another operation in {root}", flush=True)
            lease = try_lock(path, held, block=True)
        yield cast(int, lease)


def job_limits(environ: dict[str, str], slots: int) -> dict[str, str]:
    """Give a shared operation's child about 1/N of the processors; a value the caller set wins."""
    jobs = str(max(1, len(os.sched_getaffinity(0)) // slots))
    return {**{name: jobs for name in JOB_VARIABLES}, **environ}


def slot_count(value: str | None) -> int:
    count = DEFAULT_SLOTS if value is None or value == "" else int(value) if value.isdecimal() else 0
    if not 1 <= count <= MAX_SLOTS:
        raise ValueError(f"build slots must be a whole number from 1 to {MAX_SLOTS}")
    return count


def disk_room(path: Path) -> None:
    free = shutil.disk_usage(path).free
    if free < DISK_FLOOR:
        raise ValueError(f"{path}: {free / 1024**3:.1f} GiB free; require {DISK_FLOOR / 1024**3:.0f} GiB")


def cargo_configs(root: Path, environ: dict[str, str], tracked: list[str]) -> dict[str, str]:
    """Every Cargo configuration file a build here reads: tracked ones, the worktree's and its parents', and CARGO_HOME's."""
    inputs = {name: (root / name).read_text() for name in tracked if name.startswith(".cargo/")}
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
    return inputs


def build_environment(environ: dict[str, str]) -> dict[str, str]:
    """The build inputs among the environment variables (see PROJECT_BUILD_VARIABLES for the rule)."""
    environment = {key: value for key, value in environ.items()
                   if (key in BUILD_ENV_NAMES or key.startswith(BUILD_ENV_PREFIXES))
                   and not key.startswith(RUNTIME_PREFIXES) and key not in UNFINGERPRINTED}
    environment["CARGO_INCREMENTAL"] = "0"
    return environment


def host_identity(root: Path, environ: dict[str, str]) -> dict[str, str]:
    return {"rustc": output([environ.get("RUSTC", "rustc"), "-vV"], root), "cargo": output(["cargo", "-vV"], root),
            "host": platform.platform(), "os": Path("/etc/os-release").read_text()}


def fingerprint(root: Path, profile: str, environ: dict[str, str]) -> str:
    """Match what can make a reused target wrong, or worthless, without Cargo noticing it crate by crate.

    Hashed: the rust-toolchain files, every Cargo configuration file, the root manifest's [profile] tables, the profile
    name, `rustc -vV` and `cargo -vV`, the host and OS, and the build environment (build_environment).
    Not hashed: the manifests' packages and dependencies, Cargo.lock, [workspace.lints] (it changes diagnostics, not
    code) and the sources. Cargo checks each crate's freshness against those after a clone and rebuilds only what
    changed, so a branch that adds a dependency keeps its target and can still start from the seed.
    Values are hashed, never recorded, as the environment may hold private paths.
    """
    tracked = output(["git", "ls-files", "-z"], root).split("\0")
    inputs = {name: (root / name).read_text() for name in tracked
              if name and Path(name).name in {"rust-toolchain", "rust-toolchain.toml"}}
    inputs.update(cargo_configs(root, environ, tracked))
    manifest = root / "Cargo.toml"
    profiles = tomllib.loads(manifest.read_text()).get("profile", {}) if manifest.exists() else {}
    identity = {"schema": SCHEMA, "profile": profile, "profiles": profiles, "environment": build_environment(environ),
                "inputs": inputs, **host_identity(root, environ)}
    return hashlib.sha256(json.dumps(identity, sort_keys=True).encode()).hexdigest()


def legacy_fingerprint(root: Path, profile: str, environ: dict[str, str]) -> str:
    """Schema 1's fingerprint of the same inputs: it also hashed every manifest and Cargo.lock and a wider environment.

    A schema 1 marker whose value still matches was made from exactly these inputs, which include everything schema 2
    hashes, so the target or seed is reused as it is. Remove this once no schema 1 seed or target is left.
    """
    tracked = output(["git", "ls-files", "-z"], root).split("\0")
    inputs = {name: (root / name).read_text() for name in tracked
              if name and Path(name).name in {"Cargo.toml", "Cargo.lock", "rust-toolchain", "rust-toolchain.toml"}}
    inputs.update(cargo_configs(root, environ, tracked))
    environment = {key: value for key, value in environ.items()
                   if (key in BUILD_ENV_NAMES - PROJECT_BUILD_VARIABLES or key.startswith(LEGACY_ENV_PREFIXES))
                   and key not in UNFINGERPRINTED
                   and not key.startswith("CUPERTINO_TEST_FIXTURE_")}
    environment["CARGO_INCREMENTAL"] = "0"
    identity = {"schema": 1, "profile": profile, "environment": environment, "inputs": inputs,
                **host_identity(root, environ)}
    return hashlib.sha256(json.dumps(identity, sort_keys=True).encode()).hexdigest()


def matches(metadata: Metadata, identity: str, legacy: Callable[[], str] | None) -> bool:
    """Whether a marker was made from these inputs; a schema 1 one is compared with schema 1's fingerprint."""
    if metadata["schema"] == SCHEMA:
        return metadata["fingerprint"] == identity
    return legacy is not None and metadata["fingerprint"] == legacy()


def read_marker(directory: Path, owner: str) -> Metadata:
    if directory.is_symlink() or not directory.is_dir() or (directory / MARKER).is_symlink():
        raise ValueError(f"{directory}: refusing an unmanaged directory or symlink")
    if not (directory / MARKER).exists():
        raise ValueError(f"{directory}: no {MARKER}, so this wrapper did not make it (a cargo or "
                         "ci/build-test-fixtures.py command run without `run` makes one); it is left alone: move it "
                         "away or delete it, then run again through `run`")
    metadata = json.loads((directory / MARKER).read_text())
    if not isinstance(metadata, dict) or metadata.get("owner") != owner or metadata.get("schema") not in (1, SCHEMA):
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


def child(command: list[str], root: Path, environ: dict[str, str], leases: tuple[int, ...]) -> int:
    """Keep the slot leases until the whole child process group has stopped."""
    child_environment = {**environ, "CUPERTINO_BUILD_LOCK_FD": ",".join(map(str, leases))}
    process = subprocess.Popen(command, cwd=root, env=child_environment, pass_fds=leases, start_new_session=True)
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


def clone(source: Path, destination: Path, root: Path, leases: tuple[int, ...]) -> None:
    # Container/package targets have their own toolchain and never enter a host seed.
    entries = [str(path) for path in source.iterdir() if path.name != "package-build"]
    if not entries:
        raise ValueError(f"{source}: seed source is empty")
    destination.mkdir()
    # No copy fallback: an unsupported filesystem must fail visibly before a build starts.
    try:
        status = child(["cp", "--archive", "--reflink=always", *entries, str(destination)],
                       root, dict(os.environ), leases)
        if status:
            raise ValueError(f"reflink clone failed with exit status {status}")
    except BaseException:
        shutil.rmtree(destination)
        raise


def seed_path(state: Path, name: str) -> Path:
    if SEED_NAME.fullmatch(name) is None:
        raise ValueError(f"seed name {name!r}: use letters, digits, '.', '_' and '-', starting with a letter or digit")
    return seed_directory(state) / name


def seed_directory(state: Path) -> Path:
    seeds = state / "seeds"
    if state.is_symlink() or seeds.is_symlink():
        raise ValueError(f"{state}: refusing a symlinked cache directory")
    return seeds


def outdate_older_sources(root: Path, seed_created: float) -> int:
    """Give each worktree file (tracked, or untracked and not ignored) that is not newer than the seed the current time.

    Cargo judges a workspace crate fresh when no file it read is newer than its last build; it never compares
    contents. A seed's outputs carry the time they were built, so a worktree file older than the seed but with other
    contents (a commit made before the seed was refreshed, or a checkout older than it) would be taken as already
    built, and the seed's outputs of the other contents used. A file of the seed's own time or later is already
    rebuilt. Returns how many files were changed.
    """
    now = time.time_ns()
    names = output(["git", "ls-files", "-z", "--cached", "--others", "--exclude-standard"], root).split("\0")
    changed = 0
    for name in dict.fromkeys(filter(None, names)):
        path = root / name
        try:
            info = path.lstat()
        except FileNotFoundError:
            # a tracked file deleted in this worktree
            continue
        if stat.S_ISREG(info.st_mode) and info.st_mtime <= seed_created:
            os.utime(path, ns=(info.st_atime_ns, now))
            changed += 1
    return changed


def prepare(root: Path, state: Path, identity: str, seed: str | None, leases: tuple[int, ...],
            legacy: Callable[[], str] | None = None) -> Metadata:
    target = root / "target"
    if target.exists() or target.is_symlink():
        metadata = read_marker(target, str(root))
        if not matches(metadata, identity, legacy):
            if metadata["schema"] != SCHEMA:
                raise ValueError("target was prepared by an older worktree-build.py from other inputs; "
                                 "cleanup --target, then prepare again")
            raise ValueError("target toolchain, profile, Cargo configuration or build environment changed; "
                             "cleanup --target, then prepare again")
        if seed is not None:
            raise ValueError("a seed can only be applied to an absent target")
        if metadata["schema"] != SCHEMA:
            metadata = Metadata(schema=SCHEMA, owner=metadata["owner"], fingerprint=identity,
                                successful=metadata["successful"], created=metadata["created"])
            write_marker(target, metadata)
        return metadata
    disk_room(root)
    if seed is None:
        target.mkdir()
    else:
        source = seed_path(state, seed)
        metadata = read_marker(source, str(state))
        if not matches(metadata, identity, legacy):
            if metadata["schema"] != SCHEMA:
                raise ValueError(f"seed {seed}: made by an older worktree-build.py from other inputs; "
                                 "refresh-seed it, or prepare without --seed")
            raise ValueError(f"seed {seed}: incompatible toolchain, profile, configuration or environment")
        if time.time() - metadata["created"] >= SEED_AGE:
            raise ValueError(f"seed {seed}: expired; cleanup --expired-seeds")
        # Before the clone, so the seed's own outputs keep their times whatever the worktree ignores.
        changed = outdate_older_sources(root, metadata["created"])
        if changed:
            print(f"worktree-build: {changed} files not newer than seed {seed} given the current time, "
                  "so Cargo rebuilds what reads them", flush=True)
        clone(source, target, root, leases)
    metadata = Metadata(schema=SCHEMA, owner=str(root), fingerprint=identity, successful=False, created=time.time())
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


def exchange(first: Path, second: Path) -> None:
    """Swap two existing directory entries atomically: a reader of either name sees one whole directory or the other."""
    renameat2 = getattr(ctypes.CDLL(None, use_errno=True), "renameat2", None)
    if renameat2 is None:
        raise OSError(errno.ENOSYS, f"cannot swap {first} and {second}: the C library has no renameat2")
    if renameat2(AT_FDCWD, os.fsencode(first), AT_FDCWD, os.fsencode(second), RENAME_EXCHANGE) != 0:
        error = ctypes.get_errno()
        raise OSError(error, f"cannot swap {first} and {second}: {os.strerror(error)}")


def replace_seed(staging: Path, destination: Path, retired: Path) -> None:
    """Put the seed at `staging` in the place of the one at `destination`, leaving the previous one at `staging`.

    Where the filesystem cannot swap two names, the previous seed is renamed to `retired` and removed, and the new one
    takes its name. Every reader and writer of seeds holds every build slot, so nobody looks between the two renames;
    a crash there leaves no seed of that name (the next refresh-seed makes it) and never a partial one.
    """
    try:
        exchange(staging, destination)
    except OSError as error:
        if error.errno not in UNSWAPPABLE:
            raise
        destination.rename(retired)
        try:
            staging.rename(destination)
        except BaseException:
            retired.rename(destination)
            raise
        shutil.rmtree(retired)


def remove_staging(seeds: Path) -> None:
    """Remove what a killed promotion left under a staging name. Only called while holding every slot."""
    for entry in seeds.iterdir():
        if entry.name.startswith(STAGING):
            if entry.is_symlink() or not entry.is_dir():
                entry.unlink()
            else:
                shutil.rmtree(entry)
            print(f"worktree-build: removed unfinished seed {entry.name}", flush=True)


def promote(root: Path, state: Path, identity: str, name: str, max_seed_gib: int, leases: tuple[int, ...],
            replace: bool = False, legacy: Callable[[], str] | None = None) -> None:
    """Snapshot this worktree's successful target as seed `name`; with `replace`, swap it for the seed of that name.

    The copy is made and marked under a staging name, then takes the seed's name in one rename (or one exchange with
    the previous seed, which is removed afterwards), so no reader ever finds a half-written seed under the name.
    """
    target = root / "target"
    metadata = read_marker(target, str(root))
    if not matches(metadata, identity, legacy) or metadata.get("successful") is not True:
        raise ValueError("only a target from a successful run with matching inputs can be promoted")
    destination = seed_path(state, name)
    seeds = destination.parent
    seeds.mkdir(parents=True, exist_ok=True)
    remove_staging(seeds)
    if destination.exists() or destination.is_symlink():
        if not replace:
            raise ValueError(f"seed {name} exists; refresh-seed replaces it")
        read_marker(destination, str(state))
    if len([entry for entry in seeds.iterdir() if entry.name != name]) >= SEED_LIMIT:
        raise ValueError(f"at most {SEED_LIMIT} seeds; cleanup --expired-seeds before promoting")
    size = sum(path.lstat().st_size for path in target.rglob("*")
               if path.is_file() and path.relative_to(target).parts[0] != "package-build")
    if max_seed_gib <= 0 or size > max_seed_gib * 1024**3:
        raise ValueError(f"target exceeds the positive --max-seed-gib budget ({max_seed_gib})")
    disk_room(seeds)
    staging, retired = seeds / f"{STAGING}{name}.new", seeds / f"{STAGING}{name}.old"
    clone(target, staging, root, leases)
    try:
        write_marker(staging, Metadata(schema=SCHEMA, owner=str(state), fingerprint=identity, successful=True,
                                       created=time.time()))
        if destination.exists():
            replace_seed(staging, destination, retired)
            print(f"worktree-build: replaced seed {name}", flush=True)
        else:
            staging.rename(destination)
    finally:
        for leftover in (staging, retired):
            if leftover.exists():
                shutil.rmtree(leftover)
    print(f"worktree-build: promoted {destination}", flush=True)


def cleanup(root: Path, state: Path, target: bool, expired: bool) -> None:
    if target:
        directory = root / "target"
        read_marker(directory, str(root))
        shutil.rmtree(directory)
        print(f"worktree-build: removed {directory}", flush=True)
    seeds = seed_directory(state) if expired else None
    if seeds is not None and seeds.exists():
        remove_staging(seeds)
        for directory in seeds.iterdir():
            metadata = read_marker(directory, str(state))
            if time.time() - metadata["created"] >= SEED_AGE:
                shutil.rmtree(directory)
                print(f"worktree-build: removed expired seed {directory.name}", flush=True)


def interrupted(signum: int, frame: object) -> None:
    raise InterruptedError(f"received signal {signum}")


def run_locked(command: list[str], root: Path, environ: dict[str, str], locks: Path, slots: int, hold: str) -> int:
    """Run a package or container script holding this worktree's lease and `hold` slots.

    Called from inside a `locked --hold none` child of the same worktree, it takes only the slots: the parent keeps
    the worktree lease for its whole run, so a script can hold every slot for its compile and none while it waits on
    the network or inspects packages.
    """
    nested = "CUPERTINO_BUILD_LOCK_FD" in environ
    if nested and (environ.get(HOLD_VARIABLE) != "none" or environ.get("CUPERTINO_BUILD_LOCK_ROOT") != str(root)):
        raise ValueError("nested worktree-build invocations would deadlock; wrap the outermost command only")
    if nested and hold == "none":
        raise ValueError("a locked --hold none command already holds this worktree")
    with contextlib.ExitStack() as held:
        leases: tuple[int, ...] = () if nested else (held.enter_context(worktree_lease(locks, root)),)
        if hold != "none":
            leases += held.enter_context(acquire(locks, slots, hold == "every"))
        disk_room(root)
        if "CARGO_TARGET_DIR" in environ:
            destination = Path(environ["CARGO_TARGET_DIR"]).resolve()
            while not destination.exists():
                destination = destination.parent
            disk_room(destination)
        environment = {**environ, "CARGO_INCREMENTAL": "0", HOLD_VARIABLE: hold, "CUPERTINO_BUILD_LOCK_ROOT": str(root)}
        status = child(command, root, environment, leases)
        return status if status >= 0 else 128 - status


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="action", required=True)
    shared = argparse.ArgumentParser(add_help=False)
    shared.add_argument("--slots", help=f"build slots shared by all worktrees (default ${SLOTS_VARIABLE} or {DEFAULT_SLOTS})")
    for action in ("prepare", "run", "promote", "refresh-seed"):
        sub = commands.add_parser(action, parents=[shared], help={
            "refresh-seed": "replace seed NAME with this worktree's successful target, atomically"}.get(action))
        sub.add_argument("--profile", required=True)
        if action == "prepare":
            sub.add_argument("--seed")
        elif action == "run":
            sub.add_argument("command", nargs=argparse.REMAINDER)
        else:
            sub.add_argument("--name", required=True)
            sub.add_argument("--max-seed-gib", type=int, required=True)
    locked = commands.add_parser("locked", parents=[shared],
                                 help="hold the worktree and every build slot (or --hold one|none) around any command; "
                                      "preserve its target layout")
    locked.add_argument("--hold", choices=HOLDS, default="every")
    locked.add_argument("command", nargs=argparse.REMAINDER)
    clean = commands.add_parser("cleanup", parents=[shared])
    clean.add_argument("--target", action="store_true")
    clean.add_argument("--expired-seeds", action="store_true")
    args = parser.parse_args()
    for name in (getattr(args, "seed", None), getattr(args, "name", None)):
        if name is not None and re.fullmatch(r"[a-z0-9][a-z0-9-]{0,63}", name) is None:
            parser.error("seed names must be 1–64 lowercase letters, digits or hyphens")
    if args.action == "cleanup" and not args.target and not args.expired_seeds:
        parser.error("cleanup requires --target or --expired-seeds")
    root = Path.cwd().resolve()
    state = repository(root, args.action == "locked") / "cupertino-build"
    environ = dict(os.environ)
    slots = slot_count(args.slots if args.slots is not None else environ.get(SLOTS_VARIABLE))
    locks = Path.home() / ".cache" / "cupertino-build"
    if args.action == "locked":
        command = args.command[1:] if args.command[:1] == ["--"] else args.command
        if not command:
            parser.error("locked requires a command")
        return run_locked(command, root, environ, locks, slots, args.hold)
    if "CUPERTINO_BUILD_LOCK_FD" in environ:
        raise ValueError("nested worktree-build invocations would deadlock; wrap the outermost command only")
    # Shared operations touch only this worktree's target; seed operations need every slot.
    whole = (args.action in {"promote", "refresh-seed"} or (args.action == "prepare" and args.seed is not None)
             or (args.action == "cleanup" and args.expired_seeds))
    if any(name in environ for name in ("CARGO_BUILD_BUILD_DIR", "CARGO_BUILD_TARGET_DIR")):
        raise ValueError("remove Cargo build/target directory overrides")
    if "CARGO_TARGET_DIR" in environ and Path(environ["CARGO_TARGET_DIR"]).resolve() != root / "target":
        raise ValueError("CARGO_TARGET_DIR must be this worktree's target directory")
    environ.update(CARGO_TARGET_DIR=str(root / "target"), CARGO_INCREMENTAL="0")
    with worktree_lease(locks, root) as own, acquire(locks, slots, whole) as slot_leases:
        leases = (own, *slot_leases)
        if args.action == "cleanup":
            cleanup(root, state, args.target, args.expired_seeds)
            return 0
        identity = fingerprint(root, args.profile, environ)
        legacy = functools.cache(functools.partial(legacy_fingerprint, root, args.profile, environ))
        if args.action == "prepare":
            prepare(root, state, identity, args.seed, leases, legacy)
        elif args.action in {"promote", "refresh-seed"}:
            promote(root, state, identity, args.name, args.max_seed_gib, leases, args.action == "refresh-seed", legacy)
        else:
            command = args.command[1:] if args.command[:1] == ["--"] else args.command
            command = cargo_command(command, args.profile)
            metadata = prepare(root, state, identity, None, leases, legacy)
            disk_room(root)
            metadata["successful"] = False
            write_marker(root / "target", metadata)
            status = child(command, root, job_limits(environ, slots), leases)
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
