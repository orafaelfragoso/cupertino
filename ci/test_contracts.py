"""Tests of the contracts check: `python3 -I ci/test_contracts.py`."""
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))  # isolated mode (-I) does not add the script's directory
import contracts  # noqa: E402

INTERFACE_ENTRY = """- name: org.cupertino.Demo1
  kind: dbus-interface
  version: 1
  bus: session
  bus_name: org.cupertino.Demo1
  owner_crate: cupertino-demo
  owner_area: apps
  scope: public
  test_owner: unknown
  source: crates/cupertino-demo/src/lib.rs:1
"""
BINARY_ENTRY = """- name: cupertino-demo
  kind: binary
  version: 0.1.0
  shipped: yes
  owner_crate: cupertino-demo
  owner_area: apps
  scope: public
  test_owner: unknown
  source: crates/cupertino-demo/src/main.rs:1
"""
LIB = '#[zbus::interface(name = "org.cupertino.Demo1")]\nstruct Demo;\n'
BUILT = {"cupertino-demo": ("cupertino-demo", "crates/cupertino-demo/src/main.rs")}


def run(registry, files=None, built=BUILT, shipped=frozenset({"cupertino-demo"})):
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        files = {"crates/cupertino-demo/src/lib.rs": LIB, "crates/cupertino-demo/src/main.rs": "fn main() {}\n",
                 **(files or {}), contracts.REGISTRY: registry}
        for rel, text in files.items():
            (root / rel).parent.mkdir(parents=True, exist_ok=True)
            (root / rel).write_text(text)
        return contracts.check(root.resolve(), built, set(shipped))


class RegistryTest(unittest.TestCase):
    def test_a_complete_registry_passes(self):
        self.assertEqual(run(INTERFACE_ENTRY + BINARY_ENTRY), [])

    def test_a_zbus_interface_without_an_entry_fails(self):
        problems = run(BINARY_ENTRY)
        self.assertEqual(problems, ["crates/cupertino-demo/src/lib.rs:1: D-Bus interface org.cupertino.Demo1 has no "
                                    "entry in docs/architecture/contracts.yaml",
                                    "crates/cupertino-demo/src/lib.rs:1: bus name org.cupertino.Demo1 has no entry in "
                                    "docs/architecture/contracts.yaml"])

    def test_a_new_bus_name_without_an_entry_fails(self):
        problems = run(INTERFACE_ENTRY + BINARY_ENTRY,
                       {"crates/cupertino-demo/src/names.rs": 'const N: &str = "org.cupertino.Other1";\n'})
        self.assertEqual(len(problems), 1, problems)
        self.assertIn("org.cupertino.Other1", problems[0])

    def test_an_error_name_of_a_registered_interface_passes(self):
        files = {"crates/cupertino-demo/src/names.rs": 'const E: &str = "org.cupertino.Demo1.Error.Denied";\n'}
        self.assertEqual(run(INTERFACE_ENTRY + BINARY_ENTRY, files), [])

    def test_an_interface_that_moved_fails(self):
        problems = run(INTERFACE_ENTRY + BINARY_ENTRY, {"crates/cupertino-demo/src/lib.rs": "\n" + LIB})
        self.assertTrue(any("is registered at crates/cupertino-demo/src/lib.rs:1" in p for p in problems), problems)

    def test_a_binary_without_an_entry_fails(self):
        problems = run(INTERFACE_ENTRY)
        self.assertEqual(problems, ["cupertino-demo: binary cupertino-demo (crates/cupertino-demo/src/main.rs) has no "
                                    "entry in docs/architecture/contracts.yaml"])

    def test_a_binary_that_is_shipped_but_registered_as_not_fails(self):
        problems = run(INTERFACE_ENTRY + BINARY_ENTRY.replace("shipped: yes", "shipped: no"))
        self.assertEqual(len(problems), 1, problems)
        self.assertIn("shipped is no, packaging says yes", problems[0])

    def test_a_shipped_name_no_crate_builds_fails(self):
        problems = run(INTERFACE_ENTRY + BINARY_ENTRY, shipped={"cupertino-demo", "cupertino-ghost"})
        self.assertEqual(problems, ["packaging/release.py ships cupertino-ghost, which no workspace crate builds"])

    def test_a_missing_field_and_a_missing_test_file_fail(self):
        entry = INTERFACE_ENTRY.replace("  scope: public\n", "").replace("test_owner: unknown", "test_owner: no/such.rs")
        problems = run(entry + BINARY_ENTRY)
        self.assertEqual(sorted(p.split(": ", 1)[1] for p in problems), [
            "org.cupertino.Demo1 has no scope",
            "scope None is not one of ['public', 'shell-private', 'system', 'unknown']",
            "test_owner no/such.rs does not exist",
        ])

    def test_an_interface_no_crate_serves_fails(self):
        problems = run(INTERFACE_ENTRY + BINARY_ENTRY, {"crates/cupertino-demo/src/lib.rs": "struct Demo;\n"})
        self.assertEqual(problems, ["docs/architecture/contracts.yaml:1: org.cupertino.Demo1 is not served by any "
                                    "crate"])

    def test_a_source_line_outside_the_file_fails(self):
        problems = run(INTERFACE_ENTRY.replace("lib.rs:1", "lib.rs:99") + BINARY_ENTRY)
        self.assertTrue(any("is not path:line of an existing line" in p for p in problems), problems)


class RepositoryTest(unittest.TestCase):
    def test_the_registry_matches_this_checkout(self):
        root = Path(__file__).resolve().parent.parent
        self.assertEqual(contracts.check(root), [])


if __name__ == "__main__":
    unittest.main()
