"""Tests of the source gate: `python3 -I ci/test_gate.py`."""
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))  # isolated mode (-I) does not add the script's directory
import gate  # noqa: E402

def run(files: dict[str, str]) -> list[str]:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        for rel, text in files.items():
            (root / rel).parent.mkdir(parents=True, exist_ok=True)
            (root / rel).write_text(text)
        return gate.gate(root.resolve())[1]


class PackManifestTest(unittest.TestCase):
    def test_a_listed_cursor_theme_name_fails_in_the_default_pack(self):
        name = "Zorb" + "lax"  # built from parts so that this file passes the gate itself
        problems = run({"packaging/data/manifest.json": f'{{"cursors": {{"theme": "{name}"}}}}'})
        self.assertEqual(len(problems), 1, problems)
        self.assertIn("vendor or product name", problems[0])


class ForbiddenFileTest(unittest.TestCase):
    def test_a_font_file_fails_unless_it_is_the_pinned_one(self):
        self.assertEqual(run({"crates/x/fonts/a.ttf": "x"}), ["forbidden file type: crates/x/fonts/a.ttf"])

    def test_a_provenance_word_fails(self):
        word = "recover" + "ed"  # built from parts so that this file passes the gate itself
        problems = run({"crates/x/src/lib.rs": f"// {word} from a binary\n"})
        self.assertEqual(len(problems), 1, problems)


class HolderTest(unittest.TestCase):
    holder = "Status " + "Re" + "search & Development GmbH"  # built from parts so that this file passes the gate itself
    path = "packaging/licenses/THIRD-PARTY-LICENSES"

    def test_the_named_holder_passes_in_the_third_party_file_only(self):
        self.assertEqual(run({self.path: f"Copyright 2019 {self.holder}.\n"}), [])
        problems = run({"crates/x/src/lib.rs": f"// {self.holder}\n"})
        self.assertEqual(len(problems), 1, problems)

    def test_a_provenance_word_in_the_third_party_file_still_fails_beside_the_holder(self):
        word = "recover" + "ed"
        problems = run({self.path: f"Copyright 2019 {self.holder}.\nThis was {word} from a binary.\n"})
        self.assertEqual(len(problems), 1, problems)
        self.assertIn(f"{self.path}:2: provenance wording", problems[0])


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

    def test_a_force_added_file_under_an_ignored_path_is_still_scanned(self):
        word = "recover" + "ed"  # built from parts so that this file passes the gate itself
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "docs" / "book").mkdir(parents=True)
            (root / ".gitignore").write_text("docs/book/\n")
            (root / "docs" / "book" / "forced.js").write_text(f"// {word}\n")
            subprocess.run(["git", "init", "--quiet"], cwd=root, check=True, capture_output=True)
            subprocess.run(
                ["git", "add", "--force", "docs/book/forced.js"], cwd=root, check=True, capture_output=True
            )
            problems = gate.gate(root.resolve())[1]
        self.assertEqual(len(problems), 1, problems)
        self.assertIn("docs/book/forced.js", problems[0])

    def test_a_failing_metadata_check_is_a_problem(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "packaging").mkdir()
            (root / "packaging" / "release.py").write_text("import sys\nprint('stale', file=sys.stderr)\nsys.exit(1)\n")
            problems = gate.metadata_problems(root.resolve())
        self.assertEqual(problems, ["packaging: check-metadata: stale"])

    def test_a_tree_without_a_release_script_has_no_metadata_problems(self):
        with tempfile.TemporaryDirectory() as tmp:
            self.assertEqual(gate.metadata_problems(Path(tmp).resolve()), [])


if __name__ == "__main__":
    unittest.main()
