"""Tests of the source gate: `python3 -I ci/test_gate.py`."""
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))  # isolated mode (-I) does not add the script's directory
import gate  # noqa: E402

(PATH, ((KEYS, VALUE),)) = next(iter(gate.ALLOWED_VALUES.items()))


def run(files: dict[str, str]) -> list[str]:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        for rel, text in files.items():
            (root / rel).parent.mkdir(parents=True, exist_ok=True)
            (root / rel).write_text(text)
        return gate.gate(root.resolve())[1]


def manifest(**cursors: str) -> str:
    return json.dumps({"version": 2, "cursors": cursors, "name": "pack"}, indent=2)


class AllowedValueTest(unittest.TestCase):
    def test_the_pinned_value_is_accepted_at_its_key_in_its_file(self):
        self.assertEqual(run({PATH: manifest(theme=VALUE)}), [])

    def test_the_same_value_at_another_key_of_that_file_fails(self):
        problems = run({PATH: manifest(theme=VALUE, other=VALUE)})
        self.assertEqual(len(problems), 1, problems)
        self.assertIn("vendor or product name", problems[0])

    def test_the_same_value_in_another_file_fails(self):
        problems = run({"packaging/data/other.json": manifest(theme=VALUE)})
        self.assertEqual(len(problems), 1, problems)

    def test_a_pinned_key_with_another_value_gets_no_exemption(self):
        self.assertEqual(run({PATH: manifest(theme="Adwaita")}), [])
        problems = run({PATH: manifest(theme=VALUE + "-White")})
        self.assertEqual(len(problems), 1, problems)

    def test_the_pinned_value_in_prose_of_the_pinned_file_fails(self):
        text = json.dumps({"cursors": {"theme": VALUE}, "credits": f"cursors by {VALUE}"})
        self.assertEqual(len(run({PATH: text})), 1)

    def test_a_file_that_is_not_json_gets_no_exemption(self):
        self.assertEqual(len(run({PATH: f'{{"cursors": {{"theme": "{VALUE}"'})), 1)


class ForbiddenFileTest(unittest.TestCase):
    def test_a_font_file_fails_unless_it_is_the_pinned_one(self):
        self.assertEqual(run({"crates/x/fonts/a.ttf": "x"}), ["forbidden file type: crates/x/fonts/a.ttf"])

    def test_a_provenance_word_fails(self):
        word = "recover" + "ed"  # built from parts so that this file passes the gate itself
        problems = run({"crates/x/src/lib.rs": f"// {word} from a binary\n"})
        self.assertEqual(len(problems), 1, problems)


class IgnoredOutputTest(unittest.TestCase):
    def test_a_gitignored_build_file_is_not_scanned(self):
        word = "recover" + "ed"  # built from parts so that this file passes the gate itself
        files = {
            ".gitignore": "docs/book/\n",
            "docs/book/search.js": f"// {word}\n",
            "docs/visible.js": f"// {word}\n",
        }
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            for rel, text in files.items():
                (root / rel).parent.mkdir(parents=True, exist_ok=True)
                (root / rel).write_text(text)
            subprocess.run(["git", "init", "--quiet"], cwd=root, check=True, capture_output=True)
            problems = gate.gate(root.resolve())[1]
        self.assertEqual(len(problems), 1, problems)
        self.assertIn("docs/visible.js", problems[0])


if __name__ == "__main__":
    unittest.main()
