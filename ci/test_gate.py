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


class ListedKeyTest(unittest.TestCase):
    def test_listed_key_names_fail_in_any_case(self):
        # built from parts so that this file passes the gate itself
        for key in ("Stay" + "Awake", "stay" + "awake"):
            problems = run({"crates/x/src/lib.rs": f"//! restarted by {key}\n"})
            self.assertEqual(len(problems), 1, (key, problems))
            self.assertIn("provenance wording", problems[0])

    def test_the_words_inside_a_longer_identifier_do_not_count(self):
        self.assertEqual(run({"crates/x/src/lib.rs": "fn stay_awake_interval() {}\nlet exit_ok = true;\n"}), [])


class VendorIdentifierTest(unittest.TestCase):
    name = "pom" + "el"  # built from parts so that this file passes the gate itself

    def test_the_vendor_name_fails_in_any_case_and_inside_an_identifier(self):
        cap = self.name.capitalize()
        for text in (
            f"{self.name}_cursor",
            f"{self.name.upper()}_THEME",
            cap,
            f"{self.name}-cursors",
            f"{cap}ColorEmoji",
            f"{cap}Script",
            f"{self.name}Cursor",
            f"{self.name}2",
            f"com.{self.name}.foo",
        ):
            problems = run({"crates/x/src/lib.rs": f"// {text}\n"})
            self.assertEqual(len(problems), 1, (text, problems))
            self.assertIn("vendor or product name", problems[0])

    def test_longer_words_that_contain_it_pass(self):
        self.assertEqual(run({"crates/x/src/lib.rs": f"// a{self.name} {self.name}o {self.name}s\n"}), [])


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


if __name__ == "__main__":
    unittest.main()
