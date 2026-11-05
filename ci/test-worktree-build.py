"""Run without compiling Rust: python3 -I ci/test-worktree-build.py."""
import importlib.util
import os
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
        BUILD.write_marker(directory, {"schema": 1, "owner": str(owner), "fingerprint": "host", "successful": False,
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
                BUILD.prepare(self.root, self.state, "host", None, 0)
        self.assertFalse((self.root / "target").exists())

    def test_input_matching_covers_toolchain_profile_and_package_environment(self):
        (self.root / "Cargo.toml").write_text("[workspace]\n")
        def version(command, root):
            return "Cargo.toml" if command[0] == "git" else "toolchain-one"
        with patch.object(BUILD, "output", side_effect=version):
            identity = BUILD.fingerprint(self.root, "dev", {})
            for name in ("CUPERTINO_THEME_DIR", "CUPERTINO_SPEC_DIR", "CUPERTINO_INSTALL_PREFIX",
                         "CUPERTINO_BUILD_COMMIT", "RUSTFLAGS", "LIBCLANG_PATH"):
                self.assertNotEqual(identity, BUILD.fingerprint(self.root, "dev", {name: "changed"}), name)
            self.assertNotEqual(identity, BUILD.fingerprint(self.root, "release", {}))
            self.assertEqual(identity, BUILD.fingerprint(self.root, "dev", {"PWD": "other", "CARGO_TARGET_DIR": "other"}))
        with patch.object(BUILD, "output", return_value="toolchain-two"):
            self.assertNotEqual(identity, BUILD.fingerprint(self.root, "dev", {}))

    def test_matching_seed_is_reflinked_and_writes_do_not_change_seed(self):
        source = self.state / "seeds" / "host"
        self.metadata(source, self.state, fingerprint="matching", created=time.time())
        (source / "artifact").write_text("before")
        with patch.object(BUILD, "disk_room"), BUILD.exclusive(self.root / "lock") as lease:
            BUILD.prepare(self.root, self.state, "matching", "host", lease)
        (self.root / "target" / "artifact").write_text("after")
        self.assertEqual((source / "artifact").read_text(), "before")
        self.assertFalse(BUILD.read_marker(self.root / "target", str(self.root))["successful"])

    def test_incompatible_or_expired_seed_refuses_before_clone(self):
        source = self.state / "seeds" / "host"
        self.metadata(source, self.state, fingerprint="container", created=0)
        with patch.object(BUILD, "disk_room"), patch.object(BUILD, "clone") as clone:
            with self.assertRaisesRegex(ValueError, "incompatible"):
                BUILD.prepare(self.root, self.state, "host", "host", 0)
            with self.assertRaisesRegex(ValueError, "expired"):
                BUILD.prepare(self.root, self.state, "container", "host", 0)
        clone.assert_not_called()

    def test_copy_failure_is_visible_and_partial_destination_removed(self):
        source = self.root / "source"
        source.mkdir()
        (source / "artifact").write_text("partial")
        target = self.root / "target"
        with patch.object(BUILD, "child", return_value=1):
            with self.assertRaisesRegex(ValueError, "reflink clone failed"):
                BUILD.clone(source, target, self.root, 0)
        self.assertFalse(target.exists())

    def test_promote_requires_success_and_bounds_seed_count_and_size(self):
        target = self.root / "target"
        self.metadata(target, self.root, fingerprint="host", successful=False)
        with self.assertRaisesRegex(ValueError, "successful run"):
            BUILD.promote(self.root, self.state, "host", "host", 12, 0)
        BUILD.write_marker(target, {"schema": 1, "owner": str(self.root), "fingerprint": "host", "successful": True,
                                    "created": time.time()})
        with self.assertRaisesRegex(ValueError, "budget"):
            BUILD.promote(self.root, self.state, "host", "host", 0, 0)
        (target / "package-build").mkdir()
        (target / "package-build" / "container-artifact").write_text("do not clone")
        with patch.object(BUILD, "disk_room"), BUILD.exclusive(self.root / "lock") as lease:
            BUILD.promote(self.root, self.state, "host", "host", 12, lease)
            self.assertFalse((self.state / "seeds" / "host" / "package-build").exists())
            with self.assertRaisesRegex(ValueError, "at most 1 seed"):
                BUILD.promote(self.root, self.state, "host", "another", 12, lease)

    def test_cleanup_refuses_unmanaged_or_other_worktree_targets_and_symlinks(self):
        target = self.root / "target"
        target.mkdir()
        with self.assertRaises(FileNotFoundError):
            BUILD.cleanup(self.root, self.state, True, False)
        BUILD.write_marker(target, {"schema": 1, "owner": "other-worktree"})
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
            self.assertEqual(child.call_args.args[2]["CARGO_INCREMENTAL"], "0")
            self.assertEqual(child.call_args.args[2]["CARGO_TARGET_DIR"], str(self.root / "target"))
            child.return_value = 0
            self.assertEqual(BUILD.main(), 0)
            self.assertTrue(BUILD.read_marker(self.root / "target", str(self.root))["successful"])

    def worker(self, mode):
        worker = subprocess.Popen([sys.executable, "-I", str(Path(__file__)), "--lock-worker", str(self.root / "lock"), mode],
                                  stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        def stop():
            if worker.poll() is None:
                worker.kill()
            worker.communicate(timeout=10)
        self.addCleanup(stop)
        self.assertIn("waiting", worker.stdout.readline())
        return worker

    def test_second_operation_waits_for_entire_protected_operation(self):
        first = self.worker("wait")
        self.assertEqual(first.stdout.readline(), "acquired\n")
        second = self.worker("wait")
        self.assertEqual(select.select([second.stdout], [], [], 0.1)[0], [])
        first.stdin.write("release\n")
        first.stdin.flush()
        first.wait(timeout=10)
        self.assertEqual(second.stdout.readline(), "acquired\n")
        second.stdin.write("release\n")
        second.stdin.flush()
        second.wait(timeout=10)

    def test_running_child_keeps_lock_when_wrapper_is_killed(self):
        first = self.worker("child")
        self.assertEqual(first.stdout.readline(), "child-ready\n")
        first.kill()
        first.wait(timeout=10)
        second = self.worker("wait")
        self.assertEqual(select.select([second.stdout], [], [], 0.1)[0], [])
        first.stdin.write("release\n")
        first.stdin.flush()
        self.assertEqual(second.stdout.readline(), "acquired\n")
        second.stdin.write("release\n")
        second.stdin.flush()
        second.wait(timeout=10)


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == "--lock-worker":
        with BUILD.exclusive(Path(sys.argv[2])) as lease:
            if sys.argv[3] == "child":
                sys.exit(BUILD.child([sys.executable, "-c", "print('child-ready', flush=True); input()"],
                                     Path.cwd(), dict(os.environ), lease))
            print("acquired", flush=True)
            input()
    else:
        unittest.main()
