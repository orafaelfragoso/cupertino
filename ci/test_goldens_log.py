"""Tests of the golden log check: `python3 -I ci/test_goldens_log.py`."""
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))  # isolated mode (-I) does not add the script's directory
from goldens_log import TESTS, verdict  # noqa: E402

PIXEL, DESKTOP, SHELL = TESTS


def failure(name: str, message: str) -> str:
    """A failed isolated test's section as libtest prints it: the worker's own output (its result line and its
    `failures:` lists), then the parent's panic with the worker's indented stderr."""
    return f"""---- {name} stdout ----

running 1 test
{message}
test {name} ... FAILED

failures:

failures:
    {name}

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 3 filtered out; finished in 1.65s


thread '{name}' (5622) panicked at crates/cupertino-server/tests/goldens.rs:433:5:
isolated golden worker failed:
    thread '{name}' (5631) panicked at crates/cupertino-server/tests/goldens.rs:331:9:
    {message}
"""


def log(passed: list[str], failed: dict[str, str]) -> str:
    lines = ["running 3 tests"]
    lines += [f"test {name} ... ok" for name in passed]
    lines += [f"test {name} ... FAILED" for name in failed]
    sections = "\n".join(failure(name, message) for name, message in failed.items())
    listed = "\n".join(f"    {name}" for name in failed)
    return "\n".join(lines) + f"\n\nfailures:\n\n{sections}\n\nfailures:\n{listed}\n"


RECORDED_3 = "re-recorded 3 goldens; rerun without CUPERTINO_UPDATE_GOLDENS"
RECORDED_5 = "re-recorded 5 goldens; rerun without CUPERTINO_UPDATE_GOLDENS"


class CompareTest(unittest.TestCase):
    def test_every_golden_test_passing_passes(self):
        self.assertIsNone(verdict(log([PIXEL, DESKTOP, SHELL], {}), update=False))

    def test_a_failure_fails(self):
        mismatch = "desktop-windows: pixels exceed 2/255 at 120 pixels"
        self.assertEqual(
            verdict(log([PIXEL, SHELL], {DESKTOP: mismatch}), update=False),
            f"tests failed: {DESKTOP}",
        )

    def test_a_missing_test_fails(self):
        self.assertEqual(
            verdict(log([PIXEL, DESKTOP], {}), update=False),
            f"expected tests did not run: {SHELL}",
        )

    def test_a_test_outside_the_list_fails(self):
        self.assertEqual(
            verdict(log([PIXEL, DESKTOP, SHELL, "new_scene_goldens"], {}), update=False),
            "tests not in the golden list ran: new_scene_goldens",
        )


class UpdateTest(unittest.TestCase):
    def test_tests_that_re_recorded_pass_whatever_their_number(self):
        self.assertIsNone(verdict(log([PIXEL], {DESKTOP: RECORDED_3, SHELL: RECORDED_5}), update=True))
        self.assertIsNone(verdict(log([PIXEL, DESKTOP], {SHELL: RECORDED_5}), update=True))

    def test_the_repeated_re_record_line_of_a_worker_is_not_a_problem(self):
        # the section holds the line twice: the worker's stdout and its indented stderr
        self.assertIsNone(verdict(log([PIXEL, SHELL], {DESKTOP: RECORDED_3}), update=True))

    def test_a_failure_that_did_not_re_record_fails(self):
        self.assertEqual(
            verdict(
                log([PIXEL], {DESKTOP: RECORDED_3, SHELL: "timed out waiting for the lock screen"}),
                update=True,
            ),
            f"tests failed without re-recording: {SHELL}",
        )

    def test_an_update_that_recorded_nothing_fails(self):
        self.assertEqual(
            verdict(log([PIXEL, DESKTOP, SHELL], {}), update=True),
            "an update re-recorded nothing",
        )

    def test_a_missing_test_fails(self):
        self.assertEqual(
            verdict(log([PIXEL], {DESKTOP: RECORDED_3}), update=True),
            f"expected tests did not run: {SHELL}",
        )


class RealLogTest(unittest.TestCase):
    """The shape of the `ci/goldens.sh --update` log that a hard-coded count misread (row 80 part 3)."""

    def test_two_re_recording_tests_with_their_lines_repeated_pass(self):
        text = log([PIXEL], {DESKTOP: RECORDED_3, SHELL: RECORDED_5})
        self.assertEqual(text.count("re-recorded"), 4)
        self.assertIsNone(verdict(text, update=True))


if __name__ == "__main__":
    unittest.main()
