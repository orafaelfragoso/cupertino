"""Tests of the source gate: `python3 -I ci/test_gate.py`."""
import subprocess
import sys
import tempfile
import unittest
from unittest import mock
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


class ListedTermTest(unittest.TestCase):
    """The listed terms are digests; these tests list made-up terms in their place."""

    def listing(self, **sets):
        patches = [mock.patch.object(gate, name, {gate.digest(term) for term in terms}) for name, terms in sets.items()]
        for patch in patches:
            patch.start()
            self.addCleanup(patch.stop)

    def test_a_listed_name_fails_in_its_exact_case_as_a_word_or_phrase(self):
        self.listing(LISTED_NAMES={"Zorblax", "Quill Desk"})
        for text in ("Zorblax", "the Zorblax theme", "Quill Desk", "zorblax_theme"):
            problems = run({"crates/x/src/lib.rs": f"// {text}\n"})
            expected = 0 if text.startswith("zorblax") else 1
            self.assertEqual(len(problems), expected, (text, problems))
            if expected:
                self.assertIn("vendor or product name", problems[0])
        self.assertEqual(run({"crates/x/src/lib.rs": "// Quill  Desk, Quill-Desk, Zorblaxes\n"}), [])

    def test_a_listed_name_in_the_pack_manifest_fails(self):
        self.listing(LISTED_NAMES={"Zorblax"})
        problems = run({"packaging/data/manifest.json": '{"cursors": {"theme": "Zorblax"}}'})
        self.assertEqual(len(problems), 1, problems)
        self.assertIn("vendor or product name", problems[0])

    def test_a_listed_name_head_fails_in_any_case_and_inside_an_identifier(self):
        self.listing(LISTED_NAME_HEADS={"pomel"})
        for text in ("pomel_cursor", "POMEL_THEME", "Pomel", "pomel-cursors", "PomelColorEmoji", "pomelCursor", "pomel2", "com.pomel.foo"):
            problems = run({"crates/x/src/lib.rs": f"// {text}\n"})
            self.assertEqual(len(problems), 1, (text, problems))
            self.assertIn("vendor or product name", problems[0])
        self.assertEqual(run({"crates/x/src/lib.rs": "// apomel pomelo pomels\n"}), [])

    def test_a_listed_word_fails_in_any_case_but_not_split_inside_an_identifier(self):
        self.listing(LISTED_WORDS={"stayawake"})
        for text in ("StayAwake", "stayawake", "STAYAWAKE"):
            problems = run({"crates/x/src/lib.rs": f"//! restarted by {text}\n"})
            self.assertEqual(len(problems), 1, (text, problems))
            self.assertIn("provenance wording", problems[0])
        self.assertEqual(run({"crates/x/src/lib.rs": "fn stay_awake_interval() {}\n"}), [])

    def test_a_listed_word_head_fails_at_the_start_of_a_word(self):
        self.listing(LISTED_WORD_HEADS={"unravel"})
        lengths = mock.patch.object(gate, "LISTED_WORD_HEAD_LENGTHS", (7,))
        lengths.start()
        self.addCleanup(lengths.stop)
        self.assertEqual(len(run({"crates/x/src/lib.rs": "// Unravelled from a file\n"})), 1)
        self.assertEqual(run({"crates/x/src/lib.rs": "// an unravle\n"}), [])

    def test_a_listed_phrase_fails_with_a_space_or_a_hyphen(self):
        self.listing(LISTED_PHRASES={"sample tray"})
        for text in ("sample tray", "Sample-Tray"):
            problems = run({"crates/x/src/lib.rs": f"// the {text}\n"})
            self.assertEqual(len(problems), 1, (text, problems))
            self.assertIn("analysis wording", problems[0])

    def test_a_listed_suffix_or_header_fails_as_a_file_type(self):
        self.listing(LISTED_SUFFIXES={".qzx"}, LISTED_MAGIC={b"QZX1"})
        self.assertEqual(run({"crates/x/data/a.qzx": "x"}), ["forbidden file type: crates/x/data/a.qzx"])
        self.assertEqual(run({"crates/x/data/b.bin": "QZX1 rest"}), ["forbidden file type: crates/x/data/b.bin"])

    def test_the_lists_hold_only_digests(self):
        for name in ("LISTED_NAMES", "LISTED_NAME_HEADS", "LISTED_WORDS", "LISTED_WORD_HEADS", "LISTED_PHRASES", "LISTED_SUFFIXES", "LISTED_MAGIC"):
            for entry in getattr(gate, name):
                self.assertRegex(entry, r"^[0-9a-f]{64}$", name)


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

    def test_a_silent_failing_metadata_check_is_a_problem(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "packaging").mkdir()
            (root / "packaging" / "release.py").write_text("import sys\nsys.exit(1)\n")
            problems = gate.metadata_problems(root.resolve())
        self.assertEqual(problems, ["packaging: check-metadata: exit 1 with no output"])

    def test_a_tree_without_a_release_script_has_no_metadata_problems(self):
        with tempfile.TemporaryDirectory() as tmp:
            self.assertEqual(gate.metadata_problems(Path(tmp).resolve()), [])

    def test_a_failing_contracts_check_is_a_problem(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "ci").mkdir()
            (root / "ci" / "contracts.py").write_text("import sys\nprint('no entry', file=sys.stderr)\nsys.exit(1)\n")
            problems = gate.contracts_problems(root.resolve())
        self.assertEqual(problems, ["contracts: no entry"])

    def test_a_tree_without_a_contracts_check_has_no_contracts_problems(self):
        with tempfile.TemporaryDirectory() as tmp:
            self.assertEqual(gate.contracts_problems(Path(tmp).resolve()), [])


class ProtocolXmlTest(unittest.TestCase):
    cargo = "[package]\nname = \"x\"\n"
    generated = 'wayland_scanner::generate_client_code!("protocols/x-v1.xml");\n'

    def problems(self, files: dict[str, str]) -> list[str]:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            for rel, text in {"crates/x/Cargo.toml": self.cargo, **files}.items():
                (root / rel).parent.mkdir(parents=True, exist_ok=True)
                (root / rel).write_text(text)
            return gate.protocol_xml_problems(root.resolve())

    def test_a_library_xml_without_include_bytes_fails(self):
        self.assertEqual(
            self.problems({"crates/x/src/lib.rs": "\n" + self.generated}),
            ["crates/x/src/lib.rs:2: protocol XML read by wayland-scanner is not include_bytes!-ed in its crate: protocols/x-v1.xml"],
        )

    def test_an_include_bytes_anywhere_in_the_crate_counts_relative_to_its_file(self):
        files = {
            "crates/x/src/lib.rs": "mod protocols;\n",
            "crates/x/src/protocols.rs": 'const _: &[u8] = include_bytes!("../protocols/x-v1.xml");\n' + self.generated,
        }
        self.assertEqual(self.problems(files), [])

    def test_an_include_bytes_in_another_crate_does_not_count(self):
        files = {
            "crates/x/src/lib.rs": self.generated,
            "crates/y/Cargo.toml": self.cargo,
            "crates/y/src/lib.rs": 'const _: &[u8] = include_bytes!("../../x/protocols/x-v1.xml");\n',
        }
        self.assertEqual(len(self.problems(files)), 1)

    def test_binaries_and_tests_are_not_checked(self):
        files = {
            "crates/x/src/main.rs": self.generated,
            "crates/x/src/bin/tool.rs": self.generated,
            "crates/x/tests/protocol.rs": self.generated,
        }
        self.assertEqual(self.problems(files), [])


class LockfileTest(unittest.TestCase):
    def problems(self, lock: str) -> list[str]:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "Cargo.lock").write_text(lock)
            return gate.lockfile_problems(root.resolve())

    def entry(self, name: str, version: str) -> str:
        return f'[[package]]\nname = "{name}"\nversion = "{version}"\nsource = "registry"\n\n'

    def test_the_pinned_version_passes(self):
        self.assertEqual(self.problems(self.entry("blocking", "1.6.2") + self.entry("other", "1.7.0")), [])

    def test_another_version_of_a_pinned_crate_fails(self):
        problems = self.problems(self.entry("other", "1.6.2") + self.entry("blocking", "1.7.0"))
        self.assertEqual(len(problems), 1, problems)
        self.assertIn("blocking 1.7.0", problems[0])
        self.assertTrue(problems[0].startswith("Cargo.lock:8:"), problems[0])

    def test_a_second_copy_beside_the_pinned_one_fails(self):
        self.assertEqual(len(self.problems(self.entry("blocking", "1.6.2") + self.entry("blocking", "1.7.1"))), 1)

    def test_the_checkout_lockfile_passes(self):
        self.assertEqual(gate.lockfile_problems(Path(__file__).resolve().parent.parent), [])

    def test_a_tree_without_a_lockfile_has_no_lockfile_problems(self):
        with tempfile.TemporaryDirectory() as tmp:
            self.assertEqual(gate.lockfile_problems(Path(tmp).resolve()), [])


if __name__ == "__main__":
    unittest.main()
