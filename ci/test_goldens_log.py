"""Tests of the golden log check and the renderer image's package source guard: `python3 -I ci/test_goldens_log.py`."""
import os
import hashlib
import itertools
import json
import re
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))  # isolated mode (-I) does not add the script's directory
from goldens_log import TESTS, verdict  # noqa: E402

*UNIT, DESKTOP, SHELL = TESTS


def failure(name: str, message: str, later: str | None = None) -> str:
    """A failed isolated test's section as libtest prints it: the worker's own output (its result line and its
    `failures:` lists), then the parent's panic. The parent repeats a re-record message alone when it is the worker's
    only failure; otherwise it fails with the worker's indented stderr, which holds `later` panics too."""
    worker = f"""    thread '{name}' (5631) panicked at crates/cupertino-server/tests/goldens.rs:365:9:
    {message}
"""
    if later is not None:
        worker += f"""    thread '{name}' (5631) panicked at crates/cupertino-server/tests/goldens.rs:801:13:
    {later}
"""
    parent = (
        f"{message}\n"
        if message.startswith("re-recorded ") and later is None
        else f"isolated golden worker failed (exit status: 101):\n{worker}"
    )
    return f"""---- {name} stdout ----

running 1 test
{message}
test {name} ... FAILED

failures:

failures:
    {name}

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 3 filtered out; finished in 1.65s


thread '{name}' (5622) panicked at crates/cupertino-server/tests/goldens.rs:474:9:
{parent}"""


def log(passed: list[str], failed: dict[str, str], later: dict[str, str] | None = None) -> str:
    later = later or {}
    lines = ["running 3 tests"]
    lines += [f"test {name} ... ok" for name in passed]
    lines += [f"test {name} ... FAILED" for name in failed]
    sections = "\n".join(failure(name, message, later.get(name)) for name, message in failed.items())
    listed = "\n".join(f"    {name}" for name in failed)
    return "\n".join(lines) + f"\n\nfailures:\n\n{sections}\n\nfailures:\n{listed}\n"


RECORDED_3 = "re-recorded 3 goldens; rerun without CUPERTINO_UPDATE_GOLDENS"
RECORDED_5 = "re-recorded 5 goldens; rerun without CUPERTINO_UPDATE_GOLDENS"


class ListTest(unittest.TestCase):
    def test_each_listed_test_is_a_test_of_the_suite_s_goldens_module_by_its_path(self):
        source = (Path(__file__).resolve().parent.parent / "crates/cupertino-server/tests/goldens.rs").read_text()
        for name in TESTS:
            module, _, function = name.partition("::")
            self.assertEqual(module, "goldens", name)
            self.assertRegex(source, rf"\bfn {re.escape(function)}\(\)", name)


class CompareTest(unittest.TestCase):
    def test_every_golden_test_passing_passes(self):
        self.assertIsNone(verdict(log([*UNIT, DESKTOP, SHELL], {}), update=False))

    def test_a_failure_fails(self):
        mismatch = "desktop-windows: pixels exceed 2/255 at 120 pixels"
        self.assertEqual(
            verdict(log([*UNIT, SHELL], {DESKTOP: mismatch}), update=False),
            f"tests failed: {DESKTOP}",
        )

    def test_a_missing_test_fails(self):
        self.assertEqual(
            verdict(log([*UNIT, DESKTOP], {}), update=False),
            f"expected tests did not run: {SHELL}",
        )

    def test_a_test_outside_the_list_fails(self):
        self.assertEqual(
            verdict(log([*UNIT, DESKTOP, SHELL, "new_scene_goldens"], {}), update=False),
            "tests not in the golden list ran: new_scene_goldens",
        )


class UpdateTest(unittest.TestCase):
    def test_tests_that_re_recorded_pass_whatever_their_number(self):
        self.assertIsNone(verdict(log([*UNIT], {DESKTOP: RECORDED_3, SHELL: RECORDED_5}), update=True))
        self.assertIsNone(verdict(log([*UNIT, DESKTOP], {SHELL: RECORDED_5}), update=True))

    def test_the_repeated_re_record_line_of_a_worker_is_not_a_problem(self):
        # the section holds the line twice: the worker's stdout and the parent's panic
        self.assertIsNone(verdict(log([*UNIT, SHELL], {DESKTOP: RECORDED_3}), update=True))

    def test_a_failure_that_did_not_re_record_fails(self):
        self.assertEqual(
            verdict(
                log([*UNIT], {DESKTOP: RECORDED_3, SHELL: "timed out waiting for the lock screen"}),
                update=True,
            ),
            f"tests failed without re-recording: {SHELL}",
        )

    def test_a_test_that_failed_after_re_recording_fails(self):
        self.assertEqual(
            verdict(
                log([*UNIT], {DESKTOP: RECORDED_3, SHELL: RECORDED_5}, later={SHELL: "the lock screen did not exit"}),
                update=True,
            ),
            f"tests failed without re-recording: {SHELL}",
        )

    def test_an_update_that_recorded_nothing_fails(self):
        self.assertEqual(
            verdict(log([*UNIT, DESKTOP, SHELL], {}), update=True),
            "an update re-recorded nothing",
        )

    def test_a_missing_test_fails(self):
        self.assertEqual(
            verdict(log([*UNIT], {DESKTOP: RECORDED_3}), update=True),
            f"expected tests did not run: {SHELL}",
        )


class RealLogTest(unittest.TestCase):
    """The shape of the `ci/goldens.sh --update` log that a hard-coded count misread (row 80 part 3)."""

    def test_two_re_recording_tests_with_their_lines_repeated_pass(self):
        text = log([*UNIT], {DESKTOP: RECORDED_3, SHELL: RECORDED_5})
        self.assertEqual(text.count("re-recorded"), 4)
        self.assertIsNone(verdict(text, update=True))


class UpdateStatusTest(unittest.TestCase):
    """`ci/goldens.sh --update` with a fake docker that prints a complete update log and exits with a given status."""

    def run_update(self, status: int, definition: str | None, image_ids: tuple[str, ...],
                   container_cache: str = "", github: str = "") -> subprocess.CompletedProcess[str]:
        with tempfile.TemporaryDirectory(dir=Path.home()) as scratch:
            root = Path(scratch)
            container_cache = container_cache.replace("{root}", str(root))
            bin_dir = root / "bin"
            bin_dir.mkdir()
            update_log = root / "update.log"
            update_log.write_text(log([*UNIT], {DESKTOP: RECORDED_3, SHELL: RECORDED_5}))
            source = Path(__file__).resolve().parent
            (root / "ci/goldens").mkdir(parents=True)
            for name in ("goldens.sh", "goldens_log.py", "goldens/Dockerfile"):
                (root / "ci" / name).write_bytes((source / name).read_bytes())
            script = root / "ci/goldens.sh"
            actual = definition if definition is not None else hashlib.sha256(
                (script.parent / "goldens/Dockerfile").read_bytes()).hexdigest()
            arguments = root / "arguments"
            removed = root / "removed"
            docker = bin_dir / "docker"
            docker.write_text('#!/bin/sh\nif [ "$1" = image ]; then\n'
                              'if [ "$4" = "{{.Id}}" ]; then printf "%s\\n" "$TEST_IMAGE_ID";\n'
                              f'else echo "{actual}"; fi; exit 0; fi\n'
                              f'if [ "$1" = rm ]; then printf "%s\\n" "$@" > "{removed}"; exit 0; fi\n'
                              f'if [ "$1" = create ]; then printf "%s\\n" "$@" > "{arguments}"\n'
                              'while [ "$#" -gt 0 ]; do if [ "$1" = --cidfile ]; then echo fixture-container > "$2"; break; fi; shift; done\n'
                              'exit 0; fi\n'
                              f'[ "$1" = start ] || exit 0\ncat "{update_log}"\nexit {status}\n')
            docker.chmod(0o755)
            env = dict(os.environ, PATH=f"{bin_dir}:{os.environ['PATH']}", CARGO_BUILD_JOBS="4")
            env.pop("CUPERTINO_GOLDENS_CACHE", None)
            env.pop("CUPERTINO_GOLDENS_IMAGE", None)
            env.pop("CUPERTINO_GOLDENS_SCCACHE_SIZE", None)
            env["HOME"] = str(root / "home")
            env["GITHUB_ACTIONS"] = github
            if container_cache == "default":
                env.pop("CUPERTINO_CONTAINER_CACHE", None)
                shared = root / "home/.cache/cupertino-build/containers/goldens"
            else:
                env["CUPERTINO_CONTAINER_CACHE"] = container_cache
                shared = Path(container_cache) / "goldens" if container_cache else None
            if github == "true":
                shared = None
            if definition is not None:
                env["CUPERTINO_GOLDENS_IMAGE"] = "prebuilt"
            cache = root / "target/package-build/goldens"
            downloads = shared / "cargo" if shared else cache / "cargo"
            compiled_mounts = []
            cached_source = downloads / "cached-source"
            cached_stamp = None
            for image_id in image_ids:
                arguments.unlink(missing_ok=True)
                removed.unlink(missing_ok=True)
                env["TEST_IMAGE_ID"] = image_id
                result = subprocess.run(["bash", str(script), "--update"], env=env, capture_output=True, text=True)
                self.assertTrue(downloads.is_dir())
                if arguments.exists():
                    command = arguments.read_text().splitlines()
                    target = cache / "targets" / image_id.removeprefix("sha256:")
                    self.assertTrue(target.is_dir())
                    self.assertIn("CARGO_BUILD_JOBS=4", command)
                    self.assertIn(f"{downloads}:/cache/cargo", command)
                    if shared:
                        self.assertIn(f"{shared}/sccache:/cache/sccache", command)
                        for setting in ("RUSTC_WRAPPER=/usr/bin/sccache", "SCCACHE_DIR=/cache/sccache",
                                        "SCCACHE_CACHE_SIZE=6G", "SCCACHE_IDLE_TIMEOUT=0"):
                            self.assertIn(setting, command)
                        self.assertFalse((cache / "cargo").exists())
                    else:
                        self.assertFalse(any("sccache" in argument for argument in command))
                    compiled = f"{target}:/cache/target"
                    self.assertIn(compiled, command)
                    compiled_mounts.append(compiled)
                    self.assertEqual(command[command.index("python3") - 1], image_id)
                    self.assertEqual(removed.read_text().splitlines(), ["rm", "--force", "fixture-container"])
                    self.assertFalse(Path(command[command.index("--cidfile") + 1]).exists())
                    if cached_source.exists():
                        self.assertEqual(cached_source.read_text(), "downloaded once")
                        self.assertEqual(cached_source.stat().st_mtime_ns, cached_stamp)
                    else:
                        cached_source.write_text("downloaded once")
                        cached_stamp = cached_source.stat().st_mtime_ns
                else:
                    self.assertFalse((cache / "targets").exists())
            if len(image_ids) > 1:
                self.assertEqual(len(set(compiled_mounts)), len(image_ids))
            return result

    def test_cargo_tests_failure_status_passes(self):
        self.assertEqual(self.run_update(101, None, ("sha256:" + "a" * 64,)).returncode, 0)

    def test_a_killed_container_fails_with_its_status(self):
        result = self.run_update(137, None, ("sha256:" + "a" * 64,))
        self.assertEqual(result.returncode, 1)
        self.assertIn("goldens: the update run exited 137, not with cargo test's failure status 101", result.stderr)

    def test_prebuilt_image_with_another_definition_fails(self):
        result = self.run_update(101, "another-definition", ("sha256:" + "a" * 64,))
        self.assertEqual(result.returncode, 1)
        self.assertIn("image definition does not match", result.stderr)

    def test_prebuilt_image_with_the_pinned_definition_passes(self):
        dockerfile = Path(__file__).resolve().parent / "goldens/Dockerfile"
        self.assertEqual(self.run_update(101, hashlib.sha256(dockerfile.read_bytes()).hexdigest(), ("sha256:" + "a" * 64,)).returncode, 0)

    def test_changed_image_separates_compiled_outputs_and_preserves_downloads(self):
        ids = ("sha256:" + "a" * 64, "sha256:" + "b" * 64)
        result = self.run_update(101, None, ids)
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_compile_cache_shares_sccache_and_downloads_but_keeps_the_target_per_checkout(self):
        ids = ("sha256:" + "a" * 64, "sha256:" + "b" * 64)
        for container_cache in ("{root}/container-cache", "default"):
            with self.subTest(container_cache=container_cache):
                self.assertEqual(self.run_update(101, None, ids, container_cache).returncode, 0)

    def test_compile_cache_is_off_on_github_actions_unless_named(self):
        self.assertEqual(self.run_update(101, None, ("sha256:" + "a" * 64,), "default", "true").returncode, 0)

    def test_invalid_image_identity_is_refused_before_using_it_as_a_path(self):
        result = self.run_update(101, None, ("sha256:../../escape",))
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn("invalid content identity", result.stderr)

    def test_term_removes_container_while_attachment_ignores_signal(self):
        with tempfile.TemporaryDirectory(prefix="cupertino-golden-signal-", dir=Path.home()) as scratch:
            root = Path(scratch)
            source = Path(__file__).resolve().parent
            (root / "ci/goldens").mkdir(parents=True)
            for name in ("goldens.sh", "goldens_log.py", "goldens/Dockerfile"):
                (root / "ci" / name).write_bytes((source / name).read_bytes())
            definition = hashlib.sha256((root / "ci/goldens/Dockerfile").read_bytes()).hexdigest()
            ready = root / "ready"
            removed = root / "removed"
            docker = root / "docker"
            docker.write_text('#!' + sys.executable + '\nimport signal, sys, time\nfrom pathlib import Path\n'
                              + f'ready=Path({str(ready)!r}); removed=Path({str(removed)!r})\n'
                              + 'action=sys.argv[1]\n'
                              + f'if action == "image": print("sha256:" + "a" * 64 if sys.argv[4] == "{{{{.Id}}}}" else {definition!r})\n'
                              + 'elif action == "create": Path(sys.argv[sys.argv.index("--cidfile")+1]).write_text("fixture-container")\n'
                              + 'elif action == "rm": removed.write_text("removed")\n'
                              + 'elif action == "start":\n'
                              + ' signal.signal(signal.SIGTERM, signal.SIG_IGN)\n ready.write_text("ready")\n'
                              + ' while not removed.exists(): time.sleep(0.01)\n')
            docker.chmod(0o755)
            env = dict(os.environ, PATH=f"{root}:{os.environ['PATH']}", CUPERTINO_GOLDENS_IMAGE="prebuilt",
                       HOME=str(root / "home"), CUPERTINO_CONTAINER_CACHE="")
            env.pop("CUPERTINO_GOLDENS_CACHE", None)
            process = subprocess.Popen(["bash", str(root / "ci/goldens.sh")], env=env,
                                       stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, start_new_session=True)
            try:
                deadline = time.monotonic() + 3
                while not ready.exists() and process.poll() is None and time.monotonic() < deadline:
                    time.sleep(0.01)
                self.assertTrue(ready.exists(), "the container attachment did not start")
                process.send_signal(signal.SIGTERM)
                _, stderr = process.communicate(timeout=3)
                self.assertEqual(process.returncode, 143, stderr)
                self.assertTrue(removed.exists(), "TERM released the lease without removing the container")
            finally:
                if process.poll() is None:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.communicate(timeout=3)


# The pacman.conf of the pinned base image: its repositories all read the mirror list.
STOCK_PACMAN_CONF = """[options]
HoldPkg     = pacman glibc
Architecture = auto
#[core-testing]
#Include = /etc/pacman.d/mirrorlist

[core]
Include = /etc/pacman.d/mirrorlist

[extra]
Include = /etc/pacman.d/mirrorlist
"""


class ImageSourceGuardTest(unittest.TestCase):
    """The renderer image's package source guard, run outside docker on stand-in configuration files: everything the
    Dockerfile's package RUN does before it calls pacman, with the two /etc paths moved into a scratch directory."""

    def setUp(self):
        text = (Path(__file__).resolve().parent / "goldens/Dockerfile").read_text()
        self.archive = re.search(r"^ARG ARCHIVE=(\S+)$", text, re.M)[1]
        self.dockerfile = text
        run = next(block for block in re.split(r"\n(?=[A-Z]+ )", text)
                   if block.startswith("RUN ") and "archive.archlinux.org" in block)
        commands = re.sub(r"\\\n", " ", run.removeprefix("RUN ")).split(" && ")
        self.guard = " && ".join(c.strip() for c in itertools.takewhile(lambda c: not c.strip().startswith("pacman"),
                                                                         commands))
        scratch = tempfile.TemporaryDirectory()
        self.addCleanup(scratch.cleanup)
        self.etc = Path(scratch.name)
        (self.etc / "pacman.d").mkdir()
        (self.etc / "pacman.conf").write_text(STOCK_PACMAN_CONF)

    def run_guard(self):
        # the files move; the Include value the guard matches in pacman.conf (after "= ") stays the real path
        script = re.sub(r"(?<!= )/etc/", f"{self.etc}/", self.guard)
        return subprocess.run(["sh", "-c", script], env=dict(os.environ, ARCHIVE=self.archive),
                              capture_output=True, text=True)

    def mirrorlist(self):
        return (self.etc / "pacman.d/mirrorlist").read_text()

    def test_the_archive_date_becomes_the_only_server(self):
        self.assertIn("printf", self.guard)
        result = self.run_guard()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.mirrorlist(),
                         f"Server = https://archive.archlinux.org/repos/{self.archive}/$repo/os/$arch\n")

    def test_a_server_in_pacman_conf_fails(self):
        (self.etc / "pacman.conf").write_text(STOCK_PACMAN_CONF + "\n[extra]\nServer = https://mirror.example/$repo\n")
        self.assertNotEqual(self.run_guard().returncode, 0)

    def test_a_repository_that_reads_another_mirror_list_fails(self):
        (self.etc / "pacman.conf").write_text(STOCK_PACMAN_CONF + "\n[other]\nInclude = /etc/pacman.d/mirrorlist-fast\n")
        self.assertNotEqual(self.run_guard().returncode, 0)

    def test_the_archive_date_is_the_base_image_date(self):
        # base-devel-YYYYMMDD.N.BUILD: the image's own packages are from that day, so the archive date moves them by
        # hours, not months, and its keyring knows every key the archive signed with.
        image = re.search(r"archlinux:base-devel-(\d{4})(\d{2})(\d{2})\.", self.dockerfile)
        self.assertIsNotNone(image, "the Dockerfile names the image tag its digest pins")
        self.assertEqual(self.archive, "/".join(image.groups()))


if __name__ == "__main__":
    unittest.main()
