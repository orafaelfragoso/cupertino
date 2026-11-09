"""Run without compiling Rust: python3 -I ci/test-worktree-build.py."""
import contextlib
import fcntl
import importlib.util
import os
import re
import shutil
from pathlib import Path
import select
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

SCRIPT = Path(__file__).with_name("worktree-build.py")
SPEC = importlib.util.spec_from_file_location("worktree_build", SCRIPT)
BUILD = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(BUILD)


class WorktreeBuildTest(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="cupertino-build-test-", dir=Path.home())
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.state = self.root / "state"
        self.state.mkdir()

    def metadata(self, directory, owner, **fields):
        directory.mkdir(parents=True)
        BUILD.write_marker(directory, {"schema": BUILD.SCHEMA, "owner": str(owner), "fingerprint": "host", "successful": False,
                                       "created": time.time(), **fields})

    def test_guard_only_allows_primary_checkout_but_managed_targets_do_not(self):
        common = self.root / ".git"
        for allow_primary in (False, True):
            with patch.object(BUILD, "output", side_effect=[str(self.root), str(common)]):
                if allow_primary:
                    self.assertEqual(BUILD.repository(self.root, True), common)
                else:
                    with self.assertRaisesRegex(ValueError, "linked worktree"):
                        BUILD.repository(self.root, False)
        command = [str(SCRIPT), "locked", "--", "python3", "-c", "print(1)"]
        with patch.object(BUILD, "repository", return_value=common) as repository, \
                patch.object(BUILD.Path, "cwd", return_value=self.root), \
                patch.object(BUILD.Path, "home", return_value=self.root), patch.object(BUILD, "disk_room"), \
                patch.object(BUILD, "child", return_value=0), patch.dict(os.environ, {}, clear=True), \
                patch.object(sys, "argv", command):
            self.assertEqual(BUILD.main(), 0)
            repository.assert_called_once_with(self.root, True)
            self.assertFalse((self.root / "target").exists())
            self.assertFalse((common / "cupertino-build").exists())

    def test_low_disk_refuses_prepare_before_creating_target(self):
        with patch.object(BUILD.shutil, "disk_usage", return_value=BUILD.shutil._ntuple_diskusage(100, 90, 10)):
            with self.assertRaisesRegex(ValueError, "require 30 GiB"):
                BUILD.prepare(self.root, self.state, "host", None, ())
        self.assertFalse((self.root / "target").exists())

    def test_input_matching_covers_toolchain_profile_and_package_environment(self):
        (self.root / "Cargo.toml").write_text("[workspace]\n")
        def version(command, root):
            return "Cargo.toml" if command[0] == "git" else "toolchain-one"
        with patch.object(BUILD, "output", side_effect=version):
            identity = BUILD.fingerprint(self.root, "dev", {})
            for name in ("CUPERTINO_THEME_DIR", "CUPERTINO_ADMIN_GROUPS", "CUPERTINO_INSTALL_PREFIX",
                         "CUPERTINO_BUILD_COMMIT", "RUSTFLAGS", "RUSTDOCFLAGS", "RUSTC_WRAPPER", "RUSTC", "LIBCLANG_PATH", "PATH", "HOME",
                         "CARGO_PROFILE_DEV_OPT_LEVEL", "CARGO_ENCODED_RUSTFLAGS", "RUSTUP_TOOLCHAIN",
                         "CC", "CC_x86_64_unknown_linux_gnu", "TARGET_CFLAGS", "PKG_CONFIG_PATH",
                         "BINDGEN_EXTRA_CLANG_ARGS_x86_64_unknown_linux_gnu", "CMAKE_TOOLCHAIN_FILE",
                         "SCCACHE_DIR", "LD_LIBRARY_PATH"):
                self.assertNotEqual(identity, BUILD.fingerprint(self.root, "dev", {name: "changed"}), name)
            self.assertNotEqual(identity, BUILD.fingerprint(self.root, "release", {}))
            self.assertEqual(identity, BUILD.fingerprint(self.root, "dev", {"PWD": "other", "CARGO_TARGET_DIR": "other"}))
        with patch.object(BUILD, "output", return_value="toolchain-two"):
            self.assertNotEqual(identity, BUILD.fingerprint(self.root, "dev", {}))

    def test_session_terminal_credentials_and_fixture_lease_do_not_change_compatibility(self):
        with patch.object(BUILD, "output", return_value="stable-toolchain"):
            identity = BUILD.fingerprint(self.root, "dev", {})
            for name in ("CODEX_TOOL_CALL_ID", "CODEX_THREAD_ID", "CODEX_SESSION_ID", "CODEX_APP_TOOLS_PIPE_PATH",
                         "TERM", "COLORTERM", "GH_TOKEN", "GITHUB_TOKEN", "SSH_AUTH_SOCK",
                         "DBUS_SESSION_BUS_ADDRESS", "XDG_SESSION_ID", "CUPERTINO_BUILD_LOCK_FD",
                         "CUPERTINO_TEST_FIXTURE_SESSION", "CUPERTINO_TEST_FIXTURE_EXAMPLE_STAMP",
                         "CUPERTINO_BUILD_SLOTS", "CARGO_BUILD_JOBS", "NEXTEST_TEST_THREADS", "RUST_TEST_THREADS"):
                with self.subTest(name=name):
                    self.assertEqual(identity, BUILD.fingerprint(self.root, "dev", {name: "another-session"}))

    def require_reflinks(self):
        source, destination = self.root / "probe-source", self.root / "probe-copy"
        source.write_text("probe")
        probe = subprocess.run(["cp", "--reflink=always", str(source), str(destination)], capture_output=True, text=True)
        if probe.returncode and any(reason in probe.stderr for reason in ("Operation not supported", "Invalid cross-device link")):
            self.skipTest("filesystem does not support reflink clones")
        self.assertEqual(probe.returncode, 0, probe.stderr)

    def test_matching_seed_is_reflinked_and_writes_do_not_change_seed(self):
        self.require_reflinks()
        subprocess.run(["git", "init", "-q", str(self.root)], check=True)
        source = self.state / "seeds" / "host"
        self.metadata(source, self.state, fingerprint="matching", created=time.time())
        (source / "artifact").write_text("before")
        with patch.object(BUILD, "disk_room"), BUILD.acquire(self.root / "locks", 1, True) as lease:
            BUILD.prepare(self.root, self.state, "matching", "host", lease)
        (self.root / "target" / "artifact").write_text("after")
        self.assertEqual((source / "artifact").read_text(), "before")
        self.assertFalse(BUILD.read_marker(self.root / "target", str(self.root))["successful"])

    def test_untracked_root_cargo_config_changes_compatibility(self):
        (self.root / ".cargo").mkdir()
        def version(command, root):
            return "" if command[0] == "git" else "toolchain"
        with patch.object(BUILD, "output", side_effect=version):
            before = BUILD.fingerprint(self.root, "dev", {})
            config = self.root / ".cargo" / "config"
            config.write_text('[build]\nrustflags = ["--cfg=local"]\n')
            self.assertNotEqual(before, BUILD.fingerprint(self.root, "dev", {}))
            config.write_text('[build]\nbuild-dir = "/elsewhere"\n')
            with self.assertRaisesRegex(ValueError, "external build directories"):
                BUILD.fingerprint(self.root, "dev", {})

    def workspace(self):
        """A small workspace: root manifest with a profile, a member, a lock file, a toolchain file and a Cargo config."""
        files = {
            "Cargo.toml": '[workspace]\nmembers = ["crates/a"]\n\n[profile.dev]\ndebug = "line-tables-only"\n',
            "Cargo.lock": "version = 4\n",
            "crates/a/Cargo.toml": '[package]\nname = "a"\nversion = "0.1.0"\n\n[dependencies]\n',
            "crates/a/src/lib.rs": "",
            "rust-toolchain.toml": '[toolchain]\nchannel = "1.0.0"\n',
            ".cargo/config.toml": "[build]\nincremental = false\n",
        }
        for name, contents in files.items():
            (self.root / name).parent.mkdir(parents=True, exist_ok=True)
            (self.root / name).write_text(contents)
        tracked = "\0".join(files)
        return lambda command, root: tracked if command[0] == "git" else "toolchain"

    def edit(self, name, old, new):
        path = self.root / name
        self.assertIn(old, path.read_text())
        path.write_text(path.read_text().replace(old, new))

    def test_dependencies_and_lock_file_never_change_the_fingerprint(self):
        with patch.object(BUILD, "output", side_effect=self.workspace()):
            identity = BUILD.fingerprint(self.root, "dev", {})
            self.edit("crates/a/Cargo.toml", "[dependencies]\n", '[dependencies]\nserde = "1"\n')
            self.edit("Cargo.lock", "version = 4\n", 'version = 4\n\n[[package]]\nname = "serde"\nversion = "1.0.0"\n')
            self.edit("Cargo.toml", 'members = ["crates/a"]', 'members = ["crates/a"]\n[workspace.dependencies]\nlog = "0.4"')
            self.edit("Cargo.toml", "[profile.dev]", "[workspace.lints.rust]\nunsafe_code = \"deny\"\n\n[profile.dev]")
            self.assertEqual(identity, BUILD.fingerprint(self.root, "dev", {}))

    def test_toolchain_profile_config_and_compiler_flags_change_the_fingerprint(self):
        changes = (("rust-toolchain.toml", '"1.0.0"', '"1.1.0"'),
                   ("Cargo.toml", 'debug = "line-tables-only"', "debug = 0"),
                   ("Cargo.toml", "[profile.dev]", "[profile.release]\nlto = true\n\n[profile.dev]"),
                   (".cargo/config.toml", "incremental = false", 'incremental = false\nrustflags = ["-Ctarget-cpu=native"]'))
        for name, old, new in changes:
            with self.subTest(name=name, new=new):
                with patch.object(BUILD, "output", side_effect=self.workspace()):
                    identity = BUILD.fingerprint(self.root, "dev", {})
                    self.edit(name, old, new)
                    self.assertNotEqual(identity, BUILD.fingerprint(self.root, "dev", {}))
        with patch.object(BUILD, "output", side_effect=self.workspace()):
            identity = BUILD.fingerprint(self.root, "dev", {})
            for name in ("RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "RUSTC_WRAPPER", "CARGO_PROFILE_DEV_DEBUG"):
                with self.subTest(name=name):
                    self.assertNotEqual(identity, BUILD.fingerprint(self.root, "dev", {name: "-Copt-level=1"}))

    def test_runtime_and_test_only_variables_never_change_the_fingerprint(self):
        with patch.object(BUILD, "output", side_effect=self.workspace()):
            identity = BUILD.fingerprint(self.root, "dev", {})
            for name in ("CUPERTINO_UPDATE_GOLDENS", "CUPERTINO_REPAINT_TRACE", "CUPERTINO_REPAINT_LOG", "CUPERTINO_SPEC_DIR",
                         "CUPERTINO_GOLDENS_REQUIRED", "CUPERTINO_ASSET_SEARCH_PATH", "CUPERTINO_TEST_CLOCK",
                         "CUPERTINO_BUILD_LOCK_HOLD", "CUPERTINO_BUILD_LOCK_ROOT", "RUST_LOG", "RUST_BACKTRACE",
                         "RUST_LIB_BACKTRACE", "RUST_MIN_STACK", "CARGO_TERM_COLOR", "CARGO_TERM_PROGRESS_WHEN"):
                with self.subTest(name=name):
                    self.assertEqual(identity, BUILD.fingerprint(self.root, "dev", {name: "1"}))

    def test_every_compile_time_project_variable_is_fingerprinted(self):
        """A CUPERTINO_ variable a build script or env!/option_env! reads changes what is compiled, so it is a build
        input; one a build script sets for the compiler (cargo:rustc-env) is derived from the others."""
        repository = SCRIPT.parent.parent
        tracked = subprocess.run(["git", "ls-files", "-z", "--", "*.rs"], cwd=repository, capture_output=True,
                                 check=True, text=True).stdout.split("\0")
        read, derived = set(), set()
        for name in filter(None, tracked):
            text = (repository / name).read_text()
            read |= set(re.findall(r'\b(?:option_)?env!\(\s*"(CUPERTINO_[A-Z0-9_]+)"', text))
            derived |= set(re.findall(r"rustc-env=(CUPERTINO_[A-Z0-9_]+)=", text))
            path = Path(name)
            if path.name == "build.rs" or path.parent.name == "build":
                read |= set(re.findall(r'"(CUPERTINO_[A-Z0-9_]+)"', text))
        self.assertEqual(read - derived, set(BUILD.PROJECT_BUILD_VARIABLES))

    def test_older_schema_markers_are_kept_only_while_every_input_is_unchanged(self):
        target, seed = self.root / "target", self.state / "seeds" / "old"
        self.metadata(target, self.root, schema=1, fingerprint="legacy-one", successful=True, created=5)
        with self.assertRaisesRegex(ValueError, "older worktree-build.py from other inputs; cleanup --target"):
            BUILD.prepare(self.root, self.state, "new", None, (), lambda: "legacy-two")
        with self.assertRaisesRegex(ValueError, "older worktree-build.py"):
            BUILD.prepare(self.root, self.state, "new", None, ())
        self.assertEqual(BUILD.read_marker(target, str(self.root))["schema"], 1)
        metadata = BUILD.prepare(self.root, self.state, "new", None, (), lambda: "legacy-one")
        self.assertEqual(metadata, {"schema": BUILD.SCHEMA, "owner": str(self.root), "fingerprint": "new",
                                    "successful": True, "created": 5})
        self.assertEqual(BUILD.read_marker(target, str(self.root)), metadata)
        shutil.rmtree(target)
        subprocess.run(["git", "init", "-q", str(self.root)], check=True)
        self.metadata(seed, self.state, schema=1, fingerprint="legacy-one")
        with patch.object(BUILD, "disk_room"), patch.object(BUILD, "clone") as clone:
            with self.assertRaisesRegex(ValueError, "seed old: made by an older worktree-build.py.*refresh-seed"):
                BUILD.prepare(self.root, self.state, "new", "old", (), lambda: "legacy-two")
            clone.assert_not_called()
            clone.side_effect = lambda source, destination, root, leases: destination.mkdir()
            BUILD.prepare(self.root, self.state, "new", "old", (), lambda: "legacy-one")
        self.assertEqual(BUILD.read_marker(target, str(self.root))["schema"], BUILD.SCHEMA)
        self.assertEqual(BUILD.read_marker(target, str(self.root))["fingerprint"], "new")

    def test_legacy_fingerprint_still_hashes_manifests_so_only_identical_inputs_match(self):
        with patch.object(BUILD, "output", side_effect=self.workspace()):
            legacy = BUILD.legacy_fingerprint(self.root, "dev", {"CUPERTINO_SPEC_DIR": "a"})
            self.assertNotEqual(legacy, BUILD.fingerprint(self.root, "dev", {"CUPERTINO_SPEC_DIR": "a"}))
            self.assertNotEqual(legacy, BUILD.legacy_fingerprint(self.root, "dev", {"CUPERTINO_SPEC_DIR": "b"}))
            self.edit("Cargo.lock", "version = 4", "version = 3")
            self.assertNotEqual(legacy, BUILD.legacy_fingerprint(self.root, "dev", {"CUPERTINO_SPEC_DIR": "a"}))

    def successful_target(self, contents="new build"):
        target = self.root / "target"
        self.metadata(target, self.root, fingerprint="host", successful=True)
        (target / "artifact").write_text(contents)
        return target

    def test_refresh_replaces_the_seed_of_the_same_name_in_one_step(self):
        self.require_reflinks()
        seed = self.state / "seeds" / "parity-dev"
        self.metadata(seed, self.state, fingerprint="host", successful=True, created=time.time())
        (seed / "artifact").write_text("old build")
        self.successful_target()
        with self.assertRaisesRegex(ValueError, "seed parity-dev exists; refresh-seed replaces it"):
            BUILD.promote(self.root, self.state, "host", "parity-dev", 12, ())
        observed = []
        real_clone, real_exchange = BUILD.clone, BUILD.exchange
        def clone(source, destination, root, leases):
            # While the copy is written, the seed's name still holds the whole previous seed.
            observed.append((destination.name, (seed / "artifact").read_text(), BUILD.read_marker(seed, str(self.state))))
            real_clone(source, destination, root, leases)
        def exchange(first, second):
            observed.append(("exchange", first.name, second.name))
            real_exchange(first, second)
        with patch.object(BUILD, "disk_room"), patch.object(BUILD, "clone", clone), \
                patch.object(BUILD, "exchange", exchange), BUILD.acquire(self.root / "locks", 1, True) as lease:
            BUILD.promote(self.root, self.state, "host", "parity-dev", 12, lease, replace=True)
        self.assertEqual([entry[0] for entry in observed], [".parity-dev.new", "exchange"])
        self.assertEqual(observed[0][1], "old build")
        self.assertEqual(observed[1][1:], (".parity-dev.new", "parity-dev"))
        self.assertEqual((seed / "artifact").read_text(), "new build")
        self.assertEqual(sorted(path.name for path in seed.parent.iterdir()), ["parity-dev"])
        metadata = BUILD.read_marker(seed, str(self.state))
        self.assertEqual((metadata["schema"], metadata["successful"]), (BUILD.SCHEMA, True))

    def test_refresh_creates_a_missing_seed_and_still_bounds_other_names_and_size(self):
        self.require_reflinks()
        self.successful_target()
        with patch.object(BUILD, "disk_room"), BUILD.acquire(self.root / "locks", 1, True) as lease:
            BUILD.promote(self.root, self.state, "host", "parity-dev", 12, lease, replace=True)
            self.assertEqual((self.state / "seeds" / "parity-dev" / "artifact").read_text(), "new build")
            with self.assertRaisesRegex(ValueError, "at most 1 seed"):
                BUILD.promote(self.root, self.state, "host", "another", 12, lease, replace=True)
            with self.assertRaisesRegex(ValueError, "budget"):
                BUILD.promote(self.root, self.state, "host", "parity-dev", 0, lease, replace=True)
        with self.assertRaisesRegex(ValueError, "successful run with matching inputs"):
            BUILD.promote(self.root, self.state, "other-inputs", "parity-dev", 12, (), replace=True)

    def test_a_failed_refresh_keeps_the_previous_seed_and_leaves_no_staging_copy(self):
        self.require_reflinks()
        seed = self.state / "seeds" / "parity-dev"
        self.metadata(seed, self.state, fingerprint="host", successful=True)
        (seed / "artifact").write_text("old build")
        self.successful_target()
        for failure in ("clone", "exchange"):
            with self.subTest(failure=failure), patch.object(BUILD, "disk_room"), \
                    BUILD.acquire(self.root / "locks", 1, True) as lease:
                if failure == "clone":
                    patcher = patch.object(BUILD, "child", return_value=1)
                else:
                    patcher = patch.object(BUILD, "exchange", side_effect=OSError(22, "unsupported"))
                with patcher, self.assertRaises((ValueError, OSError)):
                    BUILD.promote(self.root, self.state, "host", "parity-dev", 12, lease, replace=True)
                self.assertEqual((seed / "artifact").read_text(), "old build")
                self.assertEqual([path.name for path in seed.parent.iterdir()], ["parity-dev"])

    def git_worktree(self, root, files):
        root.mkdir(parents=True, exist_ok=True)
        subprocess.run(["git", "init", "-q", str(root)], check=True)
        for name, contents in files.items():
            (root / name).parent.mkdir(parents=True, exist_ok=True)
            (root / name).write_text(contents)
        subprocess.run(["git", "-C", str(root), "add", "--",
                        *[name for name in files if name not in {"untracked.rs", "ignored.rs"}]], check=True)

    def test_files_older_than_the_seed_get_the_current_time_and_nothing_else_is_touched(self):
        worktree = self.root / "worktree"
        self.git_worktree(worktree, {".gitignore": "/ignored.rs\n", "src/lib.rs": "", "src/new.rs": "",
                                     "src/gone.rs": "", "untracked.rs": "", "ignored.rs": ""})
        (worktree / "src/gone.rs").unlink()
        (worktree / "link.rs").symlink_to("src/lib.rs")
        subprocess.run(["git", "-C", str(worktree), "add", "link.rs"], check=True)
        seed_time = time.time() - 600
        old, new = seed_time - 3600, seed_time + 60
        for name in (".gitignore", "src/lib.rs", "untracked.rs", "ignored.rs"):
            os.utime(worktree / name, (old, old))
        os.utime(worktree / "src/new.rs", (new, new))
        os.utime(worktree / "link.rs", (old, old), follow_symlinks=False)
        before = time.time()
        self.assertEqual(BUILD.outdate_older_sources(worktree, seed_time), 3)
        for name in (".gitignore", "src/lib.rs", "untracked.rs"):
            with self.subTest(name=name):
                self.assertGreaterEqual((worktree / name).stat().st_mtime, before)
        self.assertEqual((worktree / "ignored.rs").stat().st_mtime, old)
        self.assertEqual((worktree / "src/new.rs").stat().st_mtime, new)
        self.assertEqual((worktree / "link.rs").lstat().st_mtime, old)

    def test_a_worktree_older_than_the_seed_builds_its_own_sources_not_the_seeds(self):
        """Cargo compares file times only: without new times, the seed's build of other contents is taken as fresh."""
        self.require_reflinks()
        cargo = shutil.which("cargo")
        if cargo is None:
            self.skipTest("cargo is not installed")
        def crate(root, which):
            self.git_worktree(root, {"Cargo.toml": '[package]\nname = "probe"\nversion = "0.1.0"\nedition = "2021"\n',
                                     "src/main.rs": f'fn main() {{ print!("{which}"); }}\n'})
            for name in ("Cargo.toml", "src/main.rs"):
                os.utime(root / name, (time.time() - 3600, time.time() - 3600))
        environment = {"PATH": os.environ["PATH"], "HOME": os.environ["HOME"], "CARGO_HOME": str(self.root / "cargo-home"),
                       "CARGO_INCREMENTAL": "0", "CARGO_TERM_COLOR": "never",
                       **{name: os.environ[name] for name in ("RUSTUP_HOME", "RUSTUP_TOOLCHAIN") if name in os.environ}}
        def run(root):
            return subprocess.run([cargo, "run", "--quiet", "--offline"], cwd=root, check=True, capture_output=True,
                                  text=True, env=dict(environment, CARGO_TARGET_DIR=str(root / "target"))).stdout
        origin, worktree = self.root / "origin", self.root / "worktree"
        crate(origin, "seed")
        crate(worktree, "worktree")
        self.assertEqual(run(origin), "seed")
        seed = self.state / "seeds" / "dev"
        seed.parent.mkdir(parents=True)
        subprocess.run(["cp", "--archive", "--reflink=always", str(origin / "target"), str(seed)], check=True)
        BUILD.write_marker(seed, {"schema": BUILD.SCHEMA, "owner": str(self.state), "fingerprint": "host",
                                  "successful": True, "created": time.time()})
        with patch.object(BUILD, "disk_room"), BUILD.acquire(self.root / "locks", 1, True) as lease:
            BUILD.prepare(worktree, self.state, "host", "dev", lease)
        self.assertEqual(run(worktree), "worktree")

    def test_a_staging_copy_left_by_a_killed_refresh_is_removed_and_never_counts_as_a_seed(self):
        self.require_reflinks()
        seeds = self.state / "seeds"
        (seeds / ".parity-dev.new" / "partial").mkdir(parents=True)
        self.successful_target()
        with patch.object(BUILD, "disk_room"), BUILD.acquire(self.root / "locks", 1, True) as lease:
            BUILD.promote(self.root, self.state, "host", "parity-dev", 12, lease)
        self.assertEqual([path.name for path in seeds.iterdir()], ["parity-dev"])
        (seeds / ".another.new").mkdir()
        (seeds / ".link.new").symlink_to(self.root / "target", target_is_directory=True)
        BUILD.cleanup(self.root, self.state, False, True)
        self.assertEqual([path.name for path in seeds.iterdir()], ["parity-dev"])
        self.assertTrue((self.root / "target" / "artifact").exists())

    def test_exchange_swaps_two_directories(self):
        first, second = self.root / "first", self.root / "second"
        first.mkdir()
        second.mkdir()
        (first / "one").touch()
        (second / "two").touch()
        BUILD.exchange(first, second)
        self.assertEqual([path.name for path in first.iterdir()], ["two"])
        self.assertEqual([path.name for path in second.iterdir()], ["one"])
        with self.assertRaisesRegex(OSError, "cannot swap"):
            BUILD.exchange(first, self.root / "missing")

    def test_incompatible_or_expired_seed_refuses_before_clone(self):
        source = self.state / "seeds" / "host"
        self.metadata(source, self.state, fingerprint="container", created=0)
        with patch.object(BUILD, "disk_room"), patch.object(BUILD, "clone") as clone:
            with self.assertRaisesRegex(ValueError, "incompatible"):
                BUILD.prepare(self.root, self.state, "host", "host", ())
            with self.assertRaisesRegex(ValueError, "expired"):
                BUILD.prepare(self.root, self.state, "container", "host", ())
        clone.assert_not_called()

    def test_copy_failure_is_visible_and_partial_destination_removed(self):
        source = self.root / "source"
        source.mkdir()
        (source / "artifact").write_text("partial")
        target = self.root / "target"
        with patch.object(BUILD, "child", return_value=1):
            with self.assertRaisesRegex(ValueError, "reflink clone failed"):
                BUILD.clone(source, target, self.root, ())
        self.assertFalse(target.exists())

    def test_promote_requires_success_and_bounds_seed_count_and_size(self):
        target = self.root / "target"
        self.metadata(target, self.root, fingerprint="host", successful=False)
        with self.assertRaisesRegex(ValueError, "successful run"):
            BUILD.promote(self.root, self.state, "host", "host", 12, ())
        BUILD.write_marker(target, {"schema": BUILD.SCHEMA, "owner": str(self.root), "fingerprint": "host", "successful": True,
                                    "created": time.time()})
        with self.assertRaisesRegex(ValueError, "budget"):
            BUILD.promote(self.root, self.state, "host", "host", 0, ())
        self.metadata(self.state / "seeds" / "host", self.state)
        with self.assertRaisesRegex(ValueError, "at most 1 seed"):
            BUILD.promote(self.root, self.state, "host", "another", 12, ())

    def test_promotion_excludes_separate_container_targets(self):
        self.require_reflinks()
        target = self.root / "target"
        self.metadata(target, self.root, fingerprint="host", successful=True)
        (target / "package-build").mkdir()
        (target / "package-build" / "container-artifact").write_text("do not clone")
        with patch.object(BUILD, "disk_room"), BUILD.acquire(self.root / "locks", 1, True) as lease:
            BUILD.promote(self.root, self.state, "host", "host", 12, lease)
        self.assertFalse((self.state / "seeds" / "host" / "package-build").exists())

    def test_cleanup_refuses_unmanaged_or_other_worktree_targets_and_symlinks(self):
        target = self.root / "target"
        target.mkdir()
        with self.assertRaises(FileNotFoundError):
            BUILD.cleanup(self.root, self.state, True, False)
        BUILD.write_marker(target, {"schema": BUILD.SCHEMA, "owner": "other-worktree"})
        with self.assertRaisesRegex(ValueError, "ownership"):
            BUILD.cleanup(self.root, self.state, True, False)
        target.rename(self.root / "outside")
        target.symlink_to(self.root / "outside", target_is_directory=True)
        with self.assertRaisesRegex(ValueError, "symlink"):
            BUILD.cleanup(self.root, self.state, True, False)
        self.assertTrue((self.root / "outside" / BUILD.MARKER).exists())

    def test_cleanup_refuses_symlinked_seed_parent(self):
        external = self.root / "external"
        self.metadata(external / "old", self.state, created=0)
        (self.state / "seeds").symlink_to(external, target_is_directory=True)
        with self.assertRaisesRegex(ValueError, "symlinked cache"):
            BUILD.cleanup(self.root, self.state, False, True)
        self.assertTrue((external / "old").exists())

    def test_cleanup_removes_only_owned_target_and_expired_seed(self):
        self.metadata(self.root / "target", self.root)
        self.metadata(self.state / "seeds" / "old", self.state, created=0)
        self.metadata(self.state / "seeds" / "fresh", self.state, created=time.time())
        (self.root / "unrelated").write_text("keep")
        BUILD.cleanup(self.root, self.state, True, True)
        self.assertFalse((self.root / "target").exists())
        self.assertFalse((self.state / "seeds" / "old").exists())
        self.assertTrue((self.state / "seeds" / "fresh").exists())
        self.assertEqual((self.root / "unrelated").read_text(), "keep")

    def test_cargo_profile_is_injected_before_application_arguments(self):
        self.assertEqual(BUILD.cargo_command(["cargo", "run", "--", "--release"], "dev"),
                         ["cargo", "run", "--profile", "dev", "--", "--release"])
        self.assertEqual(BUILD.cargo_command(["cargo", "nextest", "run", "--workspace"], "test"),
                         ["cargo", "nextest", "run", "--cargo-profile", "test", "--workspace"])
        fixture = ["python3", "ci/build-test-fixtures.py", "--locked", "--", "cargo", "nextest", "run"]
        self.assertEqual(BUILD.cargo_command(fixture, "dev"),
                         ["python3", "ci/build-test-fixtures.py", "--profile", "dev", "--locked", "--",
                          "cargo", "nextest", "run", "--cargo-profile", "dev"])
        for flag in ("--target-dir=elsewhere", "--release", "--profile=release", "--config=build.incremental=true"):
            with self.assertRaises(ValueError):
                BUILD.cargo_command(["cargo", "build", flag], "dev")

    def test_fixture_wrapper_cannot_override_the_managed_target_directory(self):
        for override in (["--target-dir", "/another-worktree/target"], ["--target-dir=/another-worktree/target"]):
            with self.subTest(override=override), self.assertRaisesRegex(ValueError, "target directory"):
                BUILD.cargo_command(["python3", "ci/build-test-fixtures.py", *override,
                                     "--", "cargo", "nextest", "run"], "dev")

    def test_failed_run_cannot_promote_and_success_records_actual_status(self):
        command = [str(SCRIPT), "run", "--profile", "dev", "--", "cargo", "build", "--workspace"]
        with patch.object(BUILD, "repository", return_value=self.state), patch.object(BUILD.Path, "cwd", return_value=self.root), \
                patch.object(BUILD.Path, "home", return_value=self.root), patch.object(BUILD, "fingerprint", return_value="host"), \
                patch.object(BUILD, "disk_room"), patch.object(BUILD, "child", return_value=7) as child, \
                patch.dict(os.environ, {}, clear=True), patch.object(sys, "argv", command):
            self.assertEqual(BUILD.main(), 7)
            self.assertFalse(BUILD.read_marker(self.root / "target", str(self.root))["successful"])
            child.assert_called_once()
            self.assertEqual(child.call_args.args[0], ["cargo", "build", "--profile", "dev", "--workspace"])
            # the worktree's lock and one slot
            self.assertEqual(len(child.call_args.args[3]), 2)
            self.assertEqual(child.call_args.args[2]["CARGO_INCREMENTAL"], "0")
            self.assertEqual(child.call_args.args[2]["CARGO_TARGET_DIR"], str(self.root / "target"))
            child.return_value = 0
            self.assertEqual(BUILD.main(), 0)
            self.assertTrue(BUILD.read_marker(self.root / "target", str(self.root))["successful"])

    def worker(self, slots, mode="shared", worktree=None):
        """A process taking the slots like the wrapper, after the lock of `worktree` when one is named."""
        worker = subprocess.Popen([sys.executable, "-I", str(Path(__file__)), "--lock-worker", str(self.root / "locks"),
                                   str(slots), mode, *([str(self.root / worktree)] if worktree else [])],
                                  stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        def stop():
            if worker.poll() is None:
                worker.kill()
            worker.communicate(timeout=10)
        self.addCleanup(stop)
        return worker

    def line(self, worker, timeout=10.0):
        """Read one line without buffering more, so that select() still sees the next one."""
        text = b""
        while not text.endswith(b"\n"):
            self.assertTrue(select.select([worker.stdout], [], [], timeout)[0], f"no output after {text!r}")
            byte = os.read(worker.stdout.fileno(), 1)
            if not byte:
                self.fail(f"worker exited after {text!r}: {worker.stderr.read()!r}")
            text += byte
        return text.decode()

    def granted(self, worker):
        holding = self.line(worker)
        self.assertIn("holding", holding)
        self.assertEqual(self.line(worker), "acquired\n")
        return holding

    def waits(self, worker):
        self.assertIn("waiting", self.line(worker))
        self.assertEqual(select.select([worker.stdout], [], [], 4 * BUILD.POLL)[0], [])

    def release(self, worker):
        worker.stdin.write(b"release\n")
        worker.stdin.flush()
        worker.wait(timeout=10)

    def test_second_operation_waits_for_entire_protected_operation(self):
        first = self.worker(1)
        self.assertIn("build slot 0 of 1 (heavy.lock)", self.granted(first))
        second = self.worker(1)
        self.waits(second)
        self.release(first)
        self.granted(second)
        self.release(second)

    def test_slots_are_granted_up_to_n_and_the_next_waits_for_a_free_one(self):
        holders, slots = [], []
        for _ in range(3):
            holders.append(self.worker(3))
            slots.append(self.granted(holders[-1]).split("holding build slot ")[1][0])
        self.assertEqual(slots, ["0", "1", "2"])
        fourth = self.worker(3)
        self.waits(fourth)
        self.release(holders[1])
        self.assertIn("of 3", self.granted(fourth))
        self.release(fourth)
        for worker in holders[::2]:
            self.release(worker)

    def test_older_wrapper_holding_the_single_lock_counts_as_one_slot(self):
        old = self.worker(2, "old")
        self.assertEqual(self.line(old), "acquired\n")
        first = self.worker(2)
        self.assertIn("build slot 1 of 2 (heavy-1.lock)", self.granted(first))
        second = self.worker(2)
        self.waits(second)
        self.release(old)
        self.assertIn("build slot 0 of 2", self.granted(second))
        self.release(first)
        self.release(second)

    def test_exclusive_operation_waits_for_every_slot_and_holds_back_later_arrivals(self):
        shared = [self.worker(3) for _ in range(2)]
        for worker in shared:
            self.granted(worker)
        whole = self.worker(3, "whole")
        self.waits(whole)
        # A free slot remains, but the exclusive operation arrived first.
        later = self.worker(3)
        self.waits(later)
        self.release(shared[0])
        self.assertEqual(select.select([whole.stdout], [], [], 4 * BUILD.POLL)[0], [])
        self.release(shared[1])
        self.assertIn("every build slot", self.granted(whole))
        self.assertEqual(select.select([later.stdout], [], [], 4 * BUILD.POLL)[0], [])
        old = self.worker(3, "old")
        self.assertEqual(select.select([old.stdout], [], [], 4 * BUILD.POLL)[0], [])
        self.release(whole)
        self.granted(later)
        self.assertEqual(self.line(old), "acquired\n")
        self.release(later)
        self.release(old)

    def test_waiters_are_served_in_arrival_order_and_a_killed_waiter_leaves_no_gap(self):
        holder = self.worker(1)
        self.granted(holder)
        waiters = []
        for _ in range(3):
            waiters.append(self.worker(1))
            self.waits(waiters[-1])
        dead, first, second = waiters
        dead.kill()
        dead.wait(timeout=10)
        self.release(holder)
        self.granted(first)
        self.assertEqual(select.select([second.stdout], [], [], 4 * BUILD.POLL)[0], [])
        self.release(first)
        self.granted(second)
        self.release(second)
        self.assertEqual(list((self.root / "locks" / "queue").iterdir()), [])

    def test_a_second_operation_in_the_same_worktree_waits_without_holding_a_slot(self):
        first = self.worker(2, worktree="one")
        self.granted(first)
        second = self.worker(2, worktree="one")
        self.waits(second)
        # It is not queued for a slot either, so another worktree takes the free one.
        other = self.worker(2, worktree="two")
        self.assertIn("build slot 1 of 2", self.granted(other))
        self.release(first)
        self.assertIn("build slot 0 of 2", self.granted(second))
        self.release(second)
        self.release(other)

    def test_running_child_keeps_the_worktree_lock_when_wrapper_is_killed(self):
        first = self.worker(2, "child", worktree="one")
        self.assertIn("holding", self.line(first))
        self.assertEqual(self.line(first), "child-ready\n")
        first.kill()
        first.wait(timeout=10)
        second = self.worker(2, worktree="one")
        self.waits(second)
        first.stdin.write(b"release\n")
        first.stdin.flush()
        self.granted(second)
        self.release(second)

    def test_a_shared_waiter_leaves_a_free_slot_to_each_live_waiter_ahead(self):
        # A waiter ahead that has not polled yet: its ticket is live, so the next arrival must not take its slot.
        with BUILD.ticket(self.root / "locks", False):
            holder = self.worker(2)
            self.granted(holder)
            behind = self.worker(2)
            self.waits(behind)
            self.release(holder)
            # Both slots free: one stays for the waiter ahead.
            self.assertIn("build slot 1 of 2", self.granted(behind))
        self.release(behind)

    def test_a_shared_waiter_never_passes_a_waiting_exclusive_operation(self):
        with BUILD.ticket(self.root / "locks", True):
            behind = self.worker(2)
            self.waits(behind)
        self.granted(behind)
        self.release(behind)

    def test_an_exclusive_operation_waits_for_every_earlier_arrival(self):
        with BUILD.ticket(self.root / "locks", False):
            whole = self.worker(2, "whole")
            self.waits(whole)
        self.assertIn("every build slot", self.granted(whole))
        self.release(whole)

    def test_a_ticket_left_by_a_killed_waiter_is_removed_and_not_waited_for(self):
        queue = self.root / "locks" / "queue"
        queue.mkdir(parents=True)
        (queue / f"{1:020d}-1-whole").write_text("")
        worker = self.worker(2)
        self.granted(worker)
        self.assertFalse((queue / f"{1:020d}-1-whole").exists())
        self.release(worker)

    def test_queue_entries_that_are_not_tickets_neither_block_nor_count(self):
        holder = self.worker(2)
        self.granted(holder)
        queue = self.root / "locks" / "queue"
        # Ticket-like names that sort first: a link to the held slot lock, a pipe, a directory, and a stray file.
        (queue / f"{1:020d}-1-whole").symlink_to(self.root / "locks" / "heavy.lock")
        os.mkfifo(queue / f"{2:020d}-1-shared")
        (queue / f"{3:020d}-1-shared").mkdir()
        (queue / "0-stray").write_text("")
        self.assertIn("build slot 1 of 2", self.granted(self.worker(2)))
        self.assertTrue((self.root / "locks" / "heavy.lock").exists())
        self.release(holder)

    def test_exclusive_operations_queued_together_run_one_after_the_other(self):
        shared = self.worker(2)
        self.granted(shared)
        first, second = self.worker(2, "whole"), None
        self.waits(first)
        second = self.worker(2, "whole")
        self.waits(second)
        self.release(shared)
        self.assertIn("every build slot", self.granted(first))
        self.assertEqual(select.select([second.stdout], [], [], 4 * BUILD.POLL)[0], [])
        self.release(first)
        self.assertIn("every build slot", self.granted(second))
        self.release(second)

    def test_operations_touching_seeds_or_packages_take_every_slot(self):
        expected = {("run", "--profile", "dev", "--", "cargo", "build"): False,
                    ("prepare", "--profile", "dev"): False,
                    ("cleanup", "--target"): False,
                    ("prepare", "--profile", "dev", "--seed", "host"): True,
                    ("promote", "--profile", "dev", "--name", "host", "--max-seed-gib", "12"): True,
                    ("refresh-seed", "--profile", "dev", "--name", "host", "--max-seed-gib", "12"): True,
                    ("cleanup", "--target", "--expired-seeds"): True,
                    ("cleanup", "--expired-seeds"): True,
                    ("locked", "--", "true"): True}
        for arguments, whole in expected.items():
            seen = []
            @contextlib.contextmanager
            def acquire(directory, slots, exclusive):
                seen.append((directory, slots, exclusive))
                yield ()
            with self.subTest(arguments=arguments), patch.object(BUILD, "acquire", acquire), \
                    patch.object(BUILD, "repository", return_value=self.state), \
                    patch.object(BUILD.Path, "cwd", return_value=self.root), \
                    patch.object(BUILD.Path, "home", return_value=self.root), \
                    patch.object(BUILD, "fingerprint", return_value="host"), patch.object(BUILD, "disk_room"), \
                    patch.object(BUILD, "child", return_value=0), patch.object(BUILD, "prepare"), \
                    patch.object(BUILD, "promote"), patch.object(BUILD, "cleanup"), patch.object(BUILD, "write_marker"), \
                    patch.dict(os.environ, {}, clear=True), patch.object(sys, "argv", [str(SCRIPT), *arguments]):
                BUILD.main()
                self.assertEqual(seen, [(self.root / ".cache" / "cupertino-build", 4, whole)])

    def run_locked(self, arguments, environment):
        """Run `locked` with the slot queue and worktree lease recorded; return (slot requests, leases, child env)."""
        requests, leases = [], []
        @contextlib.contextmanager
        def acquire(directory, slots, exclusive):
            requests.append(exclusive)
            yield (101,)
        @contextlib.contextmanager
        def worktree_lease(directory, root):
            leases.append(root)
            yield 100
        with patch.object(BUILD, "acquire", acquire), patch.object(BUILD, "worktree_lease", worktree_lease), \
                patch.object(BUILD, "repository", return_value=self.state), \
                patch.object(BUILD.Path, "cwd", return_value=self.root), \
                patch.object(BUILD.Path, "home", return_value=self.root), patch.object(BUILD, "disk_room"), \
                patch.object(BUILD, "child", return_value=0) as child, patch.dict(os.environ, environment, clear=True), \
                patch.object(sys, "argv", [str(SCRIPT), "locked", *arguments, "--", "true"]):
            BUILD.main()
        return requests, leases, child.call_args

    def test_locked_holds_every_slot_one_or_only_the_worktree(self):
        for hold, requests in ((None, [True]), ("every", [True]), ("one", [False]), ("none", [])):
            with self.subTest(hold=hold):
                seen, leases, call = self.run_locked(["--hold", hold] if hold else [], {})
                self.assertEqual(seen, requests)
                self.assertEqual(leases, [self.root])
                self.assertEqual(call.args[3], (100, 101) if requests else (100,))
                self.assertEqual(call.args[2]["CUPERTINO_BUILD_LOCK_HOLD"], hold or "every")
                self.assertEqual(call.args[2]["CUPERTINO_BUILD_LOCK_ROOT"], str(self.root))

    def test_a_phase_inside_a_worktree_only_hold_takes_slots_without_the_worktree_lease(self):
        parent = {"CUPERTINO_BUILD_LOCK_FD": "100", "CUPERTINO_BUILD_LOCK_HOLD": "none",
                  "CUPERTINO_BUILD_LOCK_ROOT": str(self.root)}
        for hold, requests in (("every", [True]), ("one", [False])):
            with self.subTest(hold=hold):
                seen, leases, call = self.run_locked(["--hold", hold], parent)
                self.assertEqual((seen, leases), (requests, []))
                self.assertEqual(call.args[3], (101,))
                self.assertEqual(call.args[2]["CUPERTINO_BUILD_LOCK_HOLD"], hold)
        refusals = ((["--hold", "none"], parent, "already holds this worktree"),
                    ([], {**parent, "CUPERTINO_BUILD_LOCK_HOLD": "every"}, "nested"),
                    ([], {**parent, "CUPERTINO_BUILD_LOCK_HOLD": "one"}, "nested"),
                    ([], {**parent, "CUPERTINO_BUILD_LOCK_ROOT": "/another/worktree"}, "nested"),
                    ([], {"CUPERTINO_BUILD_LOCK_FD": "100"}, "nested"))
        for arguments, environment, message in refusals:
            with self.subTest(arguments=arguments, environment=environment), self.assertRaisesRegex(ValueError, message):
                self.run_locked(arguments, environment)
        for action in (["run", "--profile", "dev", "--", "cargo", "build"], ["prepare", "--profile", "dev"]):
            with self.subTest(action=action), patch.object(BUILD, "repository", return_value=self.state), \
                    patch.object(BUILD.Path, "cwd", return_value=self.root), patch.dict(os.environ, parent, clear=True), \
                    patch.object(sys, "argv", [str(SCRIPT), *action]), self.assertRaisesRegex(ValueError, "nested"):
                BUILD.main()

    def test_slot_count_comes_from_the_flag_then_the_environment(self):
        self.assertEqual(BUILD.slot_count(None), 4)
        self.assertEqual(BUILD.slot_count(""), 4)
        self.assertEqual(BUILD.slot_count("5"), 5)
        for invalid in ("0", "17", "-1", "two"):
            with self.subTest(invalid=invalid), self.assertRaisesRegex(ValueError, "build slots"):
                BUILD.slot_count(invalid)

    def test_shared_run_caps_jobs_in_the_child_only_and_slots_never_change_the_fingerprint(self):
        identities = []
        def fingerprint(root, profile, environ):
            identities.append(dict(environ))
            return "host"
        for flags, environment, jobs in (([], {}, "4"), (["--slots", "2"], {}, "8"), ([], {"CUPERTINO_BUILD_SLOTS": "3"}, "5"),
                                         (["--slots", "1"], {"CUPERTINO_BUILD_SLOTS": "2"}, "16")):
            command = [str(SCRIPT), "run", "--profile", "dev", *flags, "--", "cargo", "build"]
            with self.subTest(flags=flags, environment=environment), \
                    patch.object(BUILD, "repository", return_value=self.state), \
                    patch.object(BUILD.Path, "cwd", return_value=self.root), \
                    patch.object(BUILD.Path, "home", return_value=self.root), \
                    patch.object(BUILD, "fingerprint", side_effect=fingerprint), patch.object(BUILD, "disk_room"), \
                    patch.object(BUILD.os, "sched_getaffinity", return_value=set(range(16))), \
                    patch.object(BUILD, "child", return_value=0) as child, \
                    patch.dict(os.environ, environment, clear=True), patch.object(sys, "argv", command):
                self.assertEqual(BUILD.main(), 0)
                child_environment = child.call_args.args[2]
                for name in ("CARGO_BUILD_JOBS", "NEXTEST_TEST_THREADS", "RUST_TEST_THREADS"):
                    self.assertEqual(child_environment[name], jobs, name)
                    self.assertNotIn(name, identities[-1])
        with patch.object(BUILD, "output", return_value="stable-toolchain"):
            self.assertEqual(len({BUILD.fingerprint(self.root, "dev", environ) for environ in identities}), 1)
        command = [str(SCRIPT), "run", "--profile", "dev", "--", "cargo", "build"]
        with patch.object(BUILD, "repository", return_value=self.state), patch.object(BUILD.Path, "cwd", return_value=self.root), \
                patch.object(BUILD.Path, "home", return_value=self.root), patch.object(BUILD, "fingerprint", return_value="host"), \
                patch.object(BUILD, "disk_room"), patch.object(BUILD, "child", return_value=0) as child, \
                patch.dict(os.environ, {"CARGO_BUILD_JOBS": "2", "NEXTEST_TEST_THREADS": "1"}, clear=True), \
                patch.object(sys, "argv", command):
            self.assertEqual(BUILD.main(), 0)
            self.assertEqual(child.call_args.args[2]["CARGO_BUILD_JOBS"], "2")
            self.assertEqual(child.call_args.args[2]["NEXTEST_TEST_THREADS"], "1")

    def test_intermediary_exit_stops_descendant_that_closed_lease(self):
        pid_file = self.root / "descendant-pid"
        program = (
            "import subprocess, sys; "
            "child = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(60)'], close_fds=True); "
            "open(sys.argv[1], 'w').write(str(child.pid))"
        )
        with BUILD.acquire(self.root / "locks", 1, True) as lease:
            self.assertEqual(BUILD.child([sys.executable, "-c", program, str(pid_file)],
                                         self.root, dict(os.environ), lease), 0)
        pid = int(pid_file.read_text())
        status = Path("/proc") / str(pid) / "stat"
        if status.exists():
            self.assertEqual(status.read_text().rsplit(")", 1)[1].split()[0], "Z")

    def test_running_child_keeps_lock_when_wrapper_is_killed(self):
        first = self.worker(1, "child")
        self.assertIn("holding", self.line(first))
        self.assertEqual(self.line(first), "child-ready\n")
        first.kill()
        first.wait(timeout=10)
        second = self.worker(1)
        self.waits(second)
        first.stdin.write(b"release\n")
        first.stdin.flush()
        self.granted(second)
        self.release(second)


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == "--lock-worker":
        directory, slots, mode = Path(sys.argv[2]), int(sys.argv[3]), sys.argv[4]
        worktree = Path(sys.argv[5]) if len(sys.argv) > 5 else None
        if mode == "old":
            # How a copy of the wrapper from before build slots takes its single lock.
            directory.mkdir(parents=True, exist_ok=True)
            with (directory / "heavy.lock").open("a+") as lease:
                fcntl.flock(lease, fcntl.LOCK_EX)
                print("acquired", flush=True)
                input()
            sys.exit()
        with contextlib.ExitStack() as held:
            own = () if worktree is None else (held.enter_context(BUILD.worktree_lease(directory, worktree)),)
            leases = own + held.enter_context(BUILD.acquire(directory, slots, mode == "whole"))
            if mode == "child":
                sys.exit(BUILD.child([sys.executable, "-c", "print('child-ready', flush=True); input()"],
                                     Path.cwd(), dict(os.environ), leases))
            print("acquired", flush=True)
            input()
    else:
        unittest.main()
