"""Tests of the checked replace: `python3 -I ci/test_replace.py`."""
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent


def replace(files: dict[str, str], *args: str) -> tuple[int, str, dict[str, str]]:
    """Run replace.py (args, then the files) over a temporary tree; the exit status, stderr and the files after."""
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        for name, text in files.items():
            (root / name).write_bytes(text.encode())
        done = subprocess.run([sys.executable, "-I", str(HERE / "replace.py"), *args, *[str(root / n) for n in files]],
                              capture_output=True, text=True)
        return done.returncode, done.stderr.replace(tmp + "/", ""), {n: (root / n).read_bytes().decode() for n in files}


class ReplaceTest(unittest.TestCase):
    def test_a_literal_is_replaced_everywhere_it_occurs(self):
        status, _, after = replace({"a": "x.y x.y xzy"}, "x.y", "Q")
        self.assertEqual((status, after["a"]), (0, "Q Q xzy"))

    def test_a_file_without_a_match_fails_by_name_and_nothing_is_written(self):
        status, err, after = replace({"a": "one", "b": "two"}, "one", "1")
        self.assertEqual(status, 1)
        self.assertEqual(err, "replace: b: expected at least one match, found 0\n")
        self.assertEqual(after, {"a": "one", "b": "two"})

    def test_count_must_match_exactly(self):
        status, err, after = replace({"a": "x x"}, "--count", "1", "x", "y")
        self.assertEqual((status, after["a"]), (1, "x x"))
        self.assertEqual(err, "replace: a: expected 1 matches, found 2\n")
        self.assertEqual(replace({"a": "x x"}, "--count", "2", "x", "y")[2]["a"], "y y")

    def test_a_regex_uses_groups_and_line_anchors(self):
        status, _, after = replace({"a": "v=1\nw=v=2\n"}, "--regex", r"^v=(\d)", r"n=\1")
        self.assertEqual((status, after["a"]), (0, "n=1\nw=v=2\n"))

    def test_an_invalid_regex_or_a_missing_file_fails(self):
        self.assertEqual(replace({"a": "x"}, "--regex", "(", "y")[0], 1)
        with tempfile.TemporaryDirectory() as tmp:
            done = subprocess.run([sys.executable, "-I", str(HERE / "replace.py"), "x", "y", f"{tmp}/missing"],
                                  capture_output=True, text=True)
            self.assertEqual(done.returncode, 1)

    def test_line_endings_and_non_ascii_text_are_kept(self):
        status, _, after = replace({"a": "caf\u00e9 x\r\ny\r\n"}, "x", "z")
        self.assertEqual((status, after["a"]), (0, "caf\u00e9 z\r\ny\r\n"))

    def test_an_empty_pattern_or_a_zero_width_match_fails(self):
        for args in (("", "y"), ("--regex", "x*", "y")):
            status, err, after = replace({"a": "x"}, *args)
            self.assertEqual((status, after["a"]), (1, "x"), args)
        self.assertEqual(err, "replace: a: the pattern matches zero characters\n")

    def test_a_write_failure_leaves_every_file_unchanged(self):
        with tempfile.TemporaryDirectory() as tmp:
            first, second = Path(tmp, "a"), Path(tmp, "locked", "b")
            first.write_text("x")
            second.parent.mkdir()
            second.write_text("x")
            second.parent.chmod(0o500)
            try:
                done = subprocess.run([sys.executable, "-I", str(HERE / "replace.py"), "x", "y", str(first), str(second)],
                                      capture_output=True, text=True)
            finally:
                second.parent.chmod(0o700)
            self.assertEqual(done.returncode, 1)
            self.assertEqual((first.read_text(), second.read_text()), ("x", "x"))
            self.assertEqual(sorted(p.name for p in Path(tmp).iterdir()), ["a", "locked"])

    def test_the_file_mode_is_kept(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp, "run.sh")
            path.write_text("x")
            path.chmod(0o755)
            subprocess.run([sys.executable, "-I", str(HERE / "replace.py"), "x", "y", str(path)], check=True, capture_output=True)
            self.assertEqual((path.read_text(), path.stat().st_mode & 0o777), ("y", 0o755))


if __name__ == "__main__":
    unittest.main()
