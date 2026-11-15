"""Tests of the instruction-count script, without Valgrind: `python3 -I scripts/dev/test_microbench.py`."""
import importlib.machinery
import importlib.util
import json
import os
import pathlib
import re
import stat
import tempfile
import unittest
from unittest import mock

HERE = pathlib.Path(__file__).resolve().parent
ROOT = HERE.parent.parent
loader = importlib.machinery.SourceFileLoader("microbench", str(HERE / "microbench"))
spec = importlib.util.spec_from_loader("microbench", loader)
microbench = importlib.util.module_from_spec(spec)
loader.exec_module(microbench)


def manifest(name: str, benches: list[str]) -> str:
    text = f'[package]\nname = "{name}"\nversion = "0.1.0"\n\n[dependencies]\n'
    for bench in benches:
        text += f'\n[[bench]]\nname = "{bench}"\nharness = false\n'
    return text


def summary(name: str, bench_id: str, new: int, old, regressed: bool = False) -> str:
    metrics = {"Ir": {"values": {"new": new, "old": old}}}
    total = {"metrics": metrics, "regressions": [{"metric": "Ir"}] if regressed else []}
    return json.dumps({"module_path": f"instructions::group::{name}", "id": bench_id,
                       "package_dir": "crates/cupertino-example",
                       "profiles": [{"tool": "Callgrind", "data": {"parts": [], "total": total}}]})


def fake_runner(directory: pathlib.Path, version: str) -> pathlib.Path:
    path = directory / "bin" / "gungraun-runner"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(f"#!/bin/sh\necho 'gungraun-runner {version}'\n")
    path.chmod(path.stat().st_mode | stat.S_IXUSR)
    return path


class DiscoveryTest(unittest.TestCase):
    def test_only_crates_with_an_instructions_bench_are_found_by_package_name(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            for directory, name, benches in [("a", "cupertino-a", ["instructions"]),
                                              ("b", "cupertino-b", ["matching"]),
                                              ("c", "cupertino-c", ["matching", "instructions"]),
                                              ("d", "cupertino-d", [])]:
                (root / "crates" / directory).mkdir(parents=True)
                (root / "crates" / directory / "Cargo.toml").write_text(manifest(name, benches))
            self.assertEqual(microbench.bench_crates(root), ["cupertino-a", "cupertino-c"])

    def test_a_checkout_without_crates_has_no_benches(self):
        with tempfile.TemporaryDirectory() as tmp:
            self.assertEqual(microbench.bench_crates(pathlib.Path(tmp)), [])

    def test_crates_are_named_with_or_without_the_prefix_and_an_unknown_one_stops_the_run(self):
        available = ["cupertino-glass", "cupertino-ui-text"]
        self.assertEqual(microbench.select([], available), available)
        self.assertEqual(microbench.select(["ui-text", "cupertino-glass", "glass"], available),
                         ["cupertino-ui-text", "cupertino-glass"])
        with self.assertRaises(SystemExit) as stop:
            microbench.select(["dock"], available)
        self.assertIn("dock", str(stop.exception))

    def test_among_takes_ci_s_json_list_or_a_comma_separated_one(self):
        crates = ["cupertino-glass", "cupertino-ui-text"]
        self.assertEqual(microbench.among(crates, None), crates)
        self.assertEqual(microbench.among(crates, '["cupertino-dock", "cupertino-ui-text"]'), ["cupertino-ui-text"])
        self.assertEqual(microbench.among(crates, "cupertino-glass, cupertino-dock"), ["cupertino-glass"])
        self.assertEqual(microbench.among(crates, "[]"), [])
        self.assertEqual(microbench.among(crates, ""), [])


class RepositoryTest(unittest.TestCase):
    def test_every_bench_crate_has_its_file_and_pins_the_library_to_the_locked_version(self):
        crates = microbench.bench_crates(ROOT)
        self.assertGreaterEqual(len(crates), 6, crates)
        version = microbench.library_version(ROOT / "Cargo.lock")
        self.assertIsNotNone(version, "Cargo.lock holds the library")
        for manifest_path in sorted((ROOT / "crates").glob("*/Cargo.toml")):
            text = manifest_path.read_text()
            name = re.search(r'^name\s*=\s*"([^"]+)"', text, re.M).group(1)
            pinned = re.search(r'^gungraun\s*=\s*"([^"]*)"', text, re.M)
            if name not in crates:
                self.assertIsNone(pinned, f"{name} depends on the library without an instructions bench")
                continue
            self.assertTrue((manifest_path.parent / "benches" / "instructions.rs").exists(), name)
            self.assertIsNotNone(pinned, f"{name} has an instructions bench but no gungraun dev-dependency")
            self.assertEqual(pinned.group(1), f"={version}", f"{name} pins the library exactly to the locked version")


class RunnerTest(unittest.TestCase):
    def test_the_library_version_is_read_from_the_lock_file(self):
        with tempfile.TemporaryDirectory() as tmp:
            lock = pathlib.Path(tmp) / "Cargo.lock"
            self.assertIsNone(microbench.library_version(lock))
            lock.write_text('version = 4\n\n[[package]]\nname = "gungraun-runner"\nversion = "9.9.9"\n\n'
                            '[[package]]\nname = "gungraun"\nversion = "0.20.0"\nsource = "registry"\n')
            self.assertEqual(microbench.library_version(lock), "0.20.0")
            lock.write_text('[[package]]\nname = "serde"\nversion = "1.0.0"\n')
            self.assertIsNone(microbench.library_version(lock))

    def test_a_runner_of_the_version_asked_is_taken_and_one_of_another_is_passed_over(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp = pathlib.Path(tmp)
            old = fake_runner(tmp / "old", "0.19.4")
            right = fake_runner(tmp / "right", "0.20.0")
            self.assertEqual(microbench.runner_version(str(right)), "0.20.0")
            self.assertIsNone(microbench.runner_version(str(tmp / "missing")))
            env = {"GUNGRAUN_RUNNER": str(right), "PATH": str(old.parent)}
            self.assertEqual(microbench.find_runner("0.20.0", tmp / "target", env, install=False), str(right))
            env = {"GUNGRAUN_RUNNER": str(old), "PATH": str(right.parent)}
            self.assertEqual(microbench.find_runner("0.20.0", tmp / "target", env, install=False),
                             str(right.parent / "gungraun-runner"))
            kept = fake_runner(tmp / "target" / "gungraun-runner" / "0.20.0", "0.20.0")
            env = {"GUNGRAUN_RUNNER": str(old), "PATH": str(old.parent)}
            self.assertEqual(microbench.find_runner("0.20.0", tmp / "target", env, install=False), str(kept))
            with self.assertRaises(SystemExit):
                microbench.find_runner("0.21.0", tmp / "target", env, install=False)

    def test_a_missing_runner_is_installed_at_exactly_the_version_into_its_root(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp) / "home"
            env = {"MICROBENCH_RUNNER_ROOT": str(root), "PATH": ""}
            calls = []

            def install(command, check):
                calls.append(command)
                fake_runner(root, "0.20.0")

            # the install is faked; the versions are read from the fake runners' files
            with mock.patch.object(microbench.subprocess, "run", side_effect=install), \
                    mock.patch.object(microbench, "runner_version", side_effect=real_version):
                path = microbench.find_runner("0.20.0", pathlib.Path(tmp) / "target", env)
            self.assertEqual(path, str(root / "bin" / "gungraun-runner"))
            self.assertEqual(len(calls), 1)
            command = calls[0]
            self.assertEqual(command[:3], ["cargo", "install", "gungraun-runner"])
            self.assertIn("=0.20.0", command)
            self.assertIn("--locked", command)
            self.assertEqual(command[command.index("--root") + 1], str(root))


def real_version(path: str):
    """The version a fake runner prints, read from its file, so the test does not depend on subprocess."""
    try:
        text = pathlib.Path(path).read_text()
    except OSError:
        return None
    match = re.search(r"gungraun-runner (\S+)'", text)
    return match.group(1) if match else None


class ArgumentsTest(unittest.TestCase):
    def test_the_limit_applies_only_when_comparing_with_a_baseline(self):
        self.assertEqual(microbench.runner_args(None, None, 3.0, []), ["--output-format=json"])
        self.assertEqual(microbench.runner_args(None, "main", 3.0, []),
                         ["--output-format=json", "--save-baseline=main"])
        self.assertEqual(microbench.runner_args("base", None, 3.0, ["--callgrind-metrics=@all"]),
                         ["--output-format=json", "--baseline=base", "--callgrind-limits=ir=3%",
                          "--callgrind-metrics=@all"])
        self.assertIn("--callgrind-limits=ir=2.5%", microbench.runner_args("base", "new", 2.5, []))

    def test_every_crate_is_built_in_one_cargo_run_that_goes_on_past_a_failure(self):
        command = microbench.cargo_command(["cupertino-a", "cupertino-b"], ["--baseline=x"])
        self.assertEqual(command, ["cargo", "bench", "--locked", "--no-fail-fast", "-p", "cupertino-a",
                                   "-p", "cupertino-b", "--bench", "instructions", "--", "--baseline=x"])

    def test_the_default_limit_is_three_percent(self):
        self.assertEqual(microbench.DEFAULT_LIMIT, 3.0)


class ReadingTest(unittest.TestCase):
    def test_readings_come_from_the_json_lines_and_other_lines_are_left_out(self):
        lines = ["   Compiling cupertino-example\n", summary("solve", "cold", 1100, 1000) + "\n", "not json {\n",
                 "{broken\n", summary("solve", "new_one", 50, None) + "\n",
                 summary("solve", "worse", 1040, 1000, regressed=True) + "\n"]
        rows = microbench.readings(lines)
        self.assertEqual([r["name"] for r in rows], ["instructions::group::solve cold",
                                                     "instructions::group::solve new_one",
                                                     "instructions::group::solve worse"])
        self.assertEqual(rows[0]["crate"], "cupertino-example")
        self.assertEqual((rows[0]["new"], rows[0]["old"], rows[0]["regressed"]), (1100, 1000, False))
        self.assertIsNone(rows[1]["old"])
        self.assertTrue(rows[2]["regressed"])

    def test_a_benchmark_without_a_baseline_is_new_and_has_no_change(self):
        rows = microbench.readings([summary("solve", "new_one", 50, None)])
        self.assertIsNone(microbench.change(rows[0]))
        text = microbench.table(rows)
        self.assertIn("new", text.splitlines()[1])
        self.assertNotIn("%", text.splitlines()[1])
        self.assertIn("| new |", microbench.table(rows, markdown=True))

    def test_the_change_is_in_percent_of_the_baseline(self):
        row = {"crate": "c", "name": "n", "new": 1030, "old": 1000, "regressed": False}
        self.assertAlmostEqual(microbench.change(row), 3.0)
        self.assertEqual(microbench.change(dict(row, new=0, old=0)), 0.0)
        self.assertEqual(microbench.change(dict(row, new=5, old=0)), float("inf"))
        self.assertIn("+3.00%", microbench.table([row]))
        marked = microbench.table([dict(row, regressed=True)], markdown=True)
        self.assertIn("**regressed**", marked)


class MainTest(unittest.TestCase):
    def test_a_checkout_without_benches_runs_nothing_and_succeeds(self):
        with tempfile.TemporaryDirectory() as tmp:
            with mock.patch.object(microbench.subprocess, "Popen") as popen:
                self.assertEqual(microbench.main(["--checkout", tmp, "--save-baseline=base"]), 0)
            popen.assert_not_called()

    def test_list_prints_the_crates_as_json(self):
        with mock.patch("builtins.print") as printed:
            self.assertEqual(microbench.main(["--list", "--among", "cupertino-glass,cupertino-dock"]), 0)
        self.assertEqual(json.loads(printed.call_args.args[0]), ["cupertino-glass"])


if __name__ == "__main__":
    os.environ.pop("GUNGRAUN_RUNNER", None)
    unittest.main()
