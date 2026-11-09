"""Tests of the contracts check: `python3 -I ci/test_contracts.py`."""
import sys
import tempfile
import unittest
from unittest import mock
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
  source: crates/cupertino-demo/src/lib.rs
  source_match: "org.cupertino.Demo1"
"""
BINARY_ENTRY = """- name: cupertino-demo
  kind: binary
  version: 0.1.0
  shipped: yes
  owner_crate: cupertino-demo
  owner_area: apps
  scope: public
  test_owner: unknown
  source: crates/cupertino-demo/src/main.rs
"""
LIB = '#[zbus::interface(name = "org.cupertino.Demo1")]\nstruct Demo;\n'
BUILT = {"cupertino-demo": ("cupertino-demo", "crates/cupertino-demo/src/main.rs")}


def run(registry, files=None, built=BUILT, shipped=frozenset({"cupertino-demo"}), unknown=None, libraries=None):
    unknown = registry.count("test_owner: unknown") if unknown is None else unknown
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        files = {"crates/cupertino-demo/src/lib.rs": LIB, "crates/cupertino-demo/src/main.rs": "fn main() {}\n",
                 **(files or {}), contracts.REGISTRY: registry}
        for rel, text in files.items():
            (root / rel).parent.mkdir(parents=True, exist_ok=True)
            (root / rel).write_text(text)
        with mock.patch.object(contracts, "UNKNOWN_TEST_OWNERS", unknown), \
                mock.patch.object(contracts, "UNKNOWN_SCOPES", 0):
            return contracts.check(root.resolve(), built, set(shipped), libraries or {})


LIBRARY_ENTRY = ("- name: libdemo.so.1\n  kind: c-library\n  version: 1.0\n  owner_crate: cupertino-demo\n"
                 "  owner_area: apps\n  scope: public\n  test_owner: crates/cupertino-demo/tests/symbols.rs\n"
                 "  source: crates/cupertino-demo/src/lib.rs\n")
HEADER, BUILD = "crates/cupertino-demo/abi/demo.h", "crates/cupertino-demo/build.rs"


def library_run(entry=LIBRARY_ENTRY, files=None, libraries=None):
    """The check of a checkout whose crate cupertino-demo builds libdemo.so.1, ABI 1.0, unless changed."""
    files = {HEADER: "#define DEMO_ABI_MAJOR 1\n\n#define DEMO_ABI_MINOR 0\n",
             BUILD: 'println!("cargo:rustc-cdylib-link-arg=-Wl,-soname,libdemo.so.1");\n',
             "crates/cupertino-demo/tests/symbols.rs": '"libdemo.so.1"\n', **(files or {})}
    libraries = {"cupertino-demo": "crates/cupertino-demo/Cargo.toml"} if libraries is None else libraries
    return run(INTERFACE_ENTRY + BINARY_ENTRY + entry, files, libraries=libraries)


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

    def test_a_window_app_id_is_not_a_bus_name(self):
        files = {"crates/cupertino-demo/src/window.rs": 'let spec = WindowSpec { app_id: "org.cupertino.Other1".into() };\n'}
        self.assertEqual(run(INTERFACE_ENTRY + BINARY_ENTRY, files), [])

    def test_an_error_name_of_a_registered_interface_passes(self):
        files = {"crates/cupertino-demo/src/names.rs": 'const E: &str = "org.cupertino.Demo1.Error.Denied";\n'}
        self.assertEqual(run(INTERFACE_ENTRY + BINARY_ENTRY, files), [])

    def test_an_interface_that_moved_within_its_file_passes(self):
        self.assertEqual(run(INTERFACE_ENTRY + BINARY_ENTRY, {"crates/cupertino-demo/src/lib.rs": "\n" + LIB}), [])

    def test_an_interface_that_moved_to_another_file_fails(self):
        files = {"crates/cupertino-demo/src/lib.rs": 'const N: &str = "org.cupertino.Demo1";\n',
                 "crates/cupertino-demo/src/bus.rs": LIB}
        problems = run(INTERFACE_ENTRY + BINARY_ENTRY, files)
        self.assertEqual(problems, ["crates/cupertino-demo/src/bus.rs:1: org.cupertino.Demo1 is registered in "
                                    "crates/cupertino-demo/src/lib.rs"])

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

    def test_a_binary_test_that_does_not_name_it_fails(self):
        entry = BINARY_ENTRY.replace("test_owner: unknown", "test_owner: crates/cupertino-demo/tests/run.rs")
        test = "crates/cupertino-demo/tests/run.rs"
        self.assertEqual(run(INTERFACE_ENTRY + entry, {test: 'env!("CARGO_BIN_EXE_cupertino-demo");\n'}), [])
        problems = run(INTERFACE_ENTRY + entry, {test: "fn main() {}\n"})
        self.assertEqual(problems, ["docs/architecture/contracts.yaml:12: test_owner "
                                    "crates/cupertino-demo/tests/run.rs does not name cupertino-demo"])

    def test_a_c_library_test_must_name_its_soname(self):
        test = "crates/cupertino-demo/tests/symbols.rs"
        self.assertEqual(library_run(files={test: '"libdemo.so.1"\n'}), [])
        problems = library_run(files={test: "fn main() {}\n"})
        self.assertEqual(len(problems), 1, problems)
        self.assertIn("does not name libdemo.so.1", problems[0])

    def test_a_cdylib_without_a_c_library_entry_fails(self):
        problems = library_run(entry="")
        self.assertEqual(problems, ["crates/cupertino-demo/Cargo.toml: cupertino-demo builds a cdylib without a "
                                    "c-library entry in docs/architecture/contracts.yaml"])

    def test_a_test_only_cdylib_needs_no_c_library_entry(self):
        def package(name, publish, kinds):
            return {"name": name, "manifest_path": f"/r/crates/{name}/Cargo.toml", "publish": publish,
                    "targets": [{"crate_types": kinds}]}
        meta = {"packages": [package("cupertino-ui-abi", None, ["lib", "cdylib"]),
                             package("cupertino-test-pam-shim", [], ["cdylib"]),
                             package("cupertino-kit", None, ["lib"])]}
        self.assertEqual(contracts.c_libraries(meta, Path("/r")),
                         {"cupertino-ui-abi": "crates/cupertino-ui-abi/Cargo.toml"})

    def test_a_c_library_entry_of_a_crate_without_a_cdylib_fails(self):
        problems = library_run(libraries={})
        self.assertEqual(problems, ["docs/architecture/contracts.yaml:21: libdemo.so.1 names cupertino-demo, which "
                                    "builds no cdylib"])

    def test_a_c_library_version_must_be_the_header_abi_version(self):
        problems = library_run(files={HEADER: "#define DEMO_ABI_MAJOR 1\n\n#define DEMO_ABI_MINOR 1\n"})
        self.assertEqual(problems, ["docs/architecture/contracts.yaml:21: libdemo.so.1 version is 1.0, "
                                    "crates/cupertino-demo/abi/demo.h says 1.1"])
        problems = library_run(files={HEADER: "/* no version */\n"})
        self.assertEqual(problems, ["docs/architecture/contracts.yaml:21: crates/cupertino-demo/abi/ has no header "
                                    "defining its ABI MAJOR and MINOR"])

    def test_a_c_library_name_must_be_the_soname_it_links_with(self):
        problems = library_run(files={BUILD: 'println!("cargo:rustc-cdylib-link-arg=-Wl,-soname,libdemo.so.2");\n'})
        self.assertEqual(problems, ["docs/architecture/contracts.yaml:21: libdemo.so.1 is not the soname "
                                    "crates/cupertino-demo/build.rs links with (libdemo.so.2)"])

    def test_an_interface_test_need_not_name_it(self):
        entry = INTERFACE_ENTRY.replace("test_owner: unknown", "test_owner: crates/cupertino-demo/tests/bus.rs")
        self.assertEqual(run(entry + BINARY_ENTRY, {"crates/cupertino-demo/tests/bus.rs": "fn main() {}\n"}), [])

    def test_an_interface_no_crate_serves_fails(self):
        problems = run(INTERFACE_ENTRY + BINARY_ENTRY, {"crates/cupertino-demo/src/lib.rs": "struct Demo;\n"})
        self.assertEqual(problems, ["docs/architecture/contracts.yaml:1: source_match '\"org.cupertino.Demo1\"' does "
                                    "not occur in crates/cupertino-demo/src/lib.rs",
                                    "docs/architecture/contracts.yaml:1: org.cupertino.Demo1 is not served by any "
                                    "crate"])

    def test_a_source_match_absent_from_its_file_fails(self):
        problems = run(INTERFACE_ENTRY.replace('"org.cupertino.Demo1"\n', 'DemoState::new\n') + BINARY_ENTRY)
        self.assertEqual(problems, ["docs/architecture/contracts.yaml:1: source_match 'DemoState::new' does not occur "
                                    "in crates/cupertino-demo/src/lib.rs"])

    def test_a_source_that_is_not_a_file_fails(self):
        problems = run(INTERFACE_ENTRY.replace("src/lib.rs\n", "src/gone.rs\n") + BINARY_ENTRY)
        self.assertEqual(problems, ["docs/architecture/contracts.yaml:1: source 'crates/cupertino-demo/src/gone.rs' is "
                                    "not an existing file",
                                    "crates/cupertino-demo/src/lib.rs:1: org.cupertino.Demo1 is registered in "
                                    "crates/cupertino-demo/src/gone.rs"])


def attribute_case(source):
    """What the check says about a crate whose only interface is declared by `source`."""
    return run(BINARY_ENTRY, {"crates/cupertino-demo/src/lib.rs": source})


class BypassTest(unittest.TestCase):
    def test_an_attribute_that_spans_lines_is_read(self):
        problems = attribute_case('#[zbus::interface(\n    name = "org.freedesktop.portal.FileChooser"\n)]\nstruct A;\n')
        self.assertEqual(problems[:1], ["crates/cupertino-demo/src/lib.rs:1: D-Bus interface "
                                        "org.freedesktop.portal.FileChooser has no entry in "
                                        "docs/architecture/contracts.yaml"])

    def test_a_name_that_is_a_constant_is_unparsed(self):
        problems = attribute_case('#[zbus::interface(name = INTERFACE)]\nstruct A;\n')
        self.assertEqual(problems, ["crates/cupertino-demo/src/lib.rs:1: unparsed interface attribute: the name must "
                                    "be a string literal"])

    def test_the_imported_form_is_read(self):
        problems = attribute_case('use zbus::interface;\n\n#[interface(name = "org.freedesktop.portal.Other")]\n'
                                  'struct A;\n')
        self.assertEqual(problems, ["crates/cupertino-demo/src/lib.rs:3: D-Bus interface "
                                    "org.freedesktop.portal.Other has no entry in docs/architecture/contracts.yaml"])

    def test_an_attribute_without_a_name_is_unparsed(self):
        for source in ("#[zbus::interface]\nstruct A;\n", "#[zbus::interface()]\nstruct A;\n",
                       "#[zbus::interface(spawn = false)]\nstruct A;\n"):
            with self.subTest(source=source):
                self.assertEqual(attribute_case(source), ["crates/cupertino-demo/src/lib.rs:1: unparsed interface "
                                                          "attribute: the name must be a string literal"])

    def test_a_name_with_other_arguments_is_read(self):
        problems = attribute_case('#[zbus::interface(name = "org.cupertino.Demo1", spawn = false)]\nstruct A;\n')
        self.assertTrue(all("org.cupertino.Demo1" in p for p in problems), problems)
        self.assertEqual(len(problems), 2, problems)

    def test_a_leading_path_separator_is_read(self):
        problems = attribute_case('#[::zbus::interface(name = "org.freedesktop.portal.Other")]\nstruct A;\n')
        self.assertEqual(problems, ["crates/cupertino-demo/src/lib.rs:1: D-Bus interface "
                                    "org.freedesktop.portal.Other has no entry in docs/architecture/contracts.yaml"])

    def test_a_cfg_attr_with_a_comma_in_its_condition_is_read(self):
        source = '#[cfg_attr(all(unix, not(test)), zbus::interface(name = "org.freedesktop.portal.Other"))]\nstruct A;\n'
        self.assertEqual(attribute_case(source), ["crates/cupertino-demo/src/lib.rs:1: D-Bus interface "
                                                  "org.freedesktop.portal.Other has no entry in "
                                                  "docs/architecture/contracts.yaml"])
        unnamed = '#[cfg_attr(all(unix, not(test)), zbus::interface(spawn = false))]\nstruct A;\n'
        self.assertEqual(attribute_case(unnamed), ["crates/cupertino-demo/src/lib.rs:1: unparsed interface attribute: "
                                                   "the name must be a string literal"])

    def test_a_proxy_argument_and_a_doc_string_are_not_interface_attributes(self):
        source = '#[proxy(interface = "org.gtk.Menus")]\ntrait P {}\n#[doc = "the interface"]\nstruct A;\n'
        self.assertEqual(attribute_case(source), [])

    def test_a_commented_out_attribute_is_ignored(self):
        self.assertEqual(attribute_case('// #[zbus::interface(name = INTERFACE)]\n'), [])


class TestModuleTest(unittest.TestCase):
    """A fake served by a `#[cfg(test)]` module serves nothing; the real interface beside it is still read."""

    FAKE = '#[cfg(test)]\nmod tests {\n    #[zbus::interface(name = "org.cupertino.Demo1")]\n    impl Fake {}\n\n' \
           '    #[zbus::interface(name = "org.freedesktop.portal.Fake")]\n    impl Other {}\n}\n'

    def test_an_inline_test_module_fake_is_not_counted_and_the_real_interface_is(self):
        self.assertEqual(run(INTERFACE_ENTRY + BINARY_ENTRY, {"crates/cupertino-demo/src/lib.rs": LIB + self.FAKE}), [])
        # without the real one, only the fake is left: nothing serves the entry
        problems = run(INTERFACE_ENTRY + BINARY_ENTRY, {"crates/cupertino-demo/src/lib.rs": self.FAKE})
        self.assertEqual(problems, ["docs/architecture/contracts.yaml:1: org.cupertino.Demo1 is not served by any "
                                    "crate"])

    def test_an_interface_after_a_test_module_keeps_its_line(self):
        source = self.FAKE + '\n#[zbus::interface(name = "org.freedesktop.portal.Other")]\nstruct A;\n'
        # the fake's bus name is still checked
        self.assertEqual(attribute_case(source), ["crates/cupertino-demo/src/lib.rs:10: D-Bus interface "
                                                  "org.freedesktop.portal.Other has no entry in "
                                                  "docs/architecture/contracts.yaml",
                                                  "crates/cupertino-demo/src/lib.rs:3: bus name org.cupertino.Demo1 "
                                                  "has no entry in docs/architecture/contracts.yaml"])

    def test_the_files_of_a_test_module_are_not_counted(self):
        fake = '#[zbus::interface(name = "org.cupertino.Demo1")]\nimpl Fake {}\n'
        files = {"crates/cupertino-demo/src/lib.rs": LIB + "mod shell;\n",
                 # an out-of-line test module, and the files of an inline one's children
                 "crates/cupertino-demo/src/shell/mod.rs": "#[cfg(test)]\nmod tests;\n#[cfg(test)]\nmod fakes {\n"
                                                         "    mod layout;\n}\n",
                 "crates/cupertino-demo/src/shell/tests.rs": fake,
                 "crates/cupertino-demo/src/shell/fakes/layout.rs": fake}
        self.assertEqual(run(INTERFACE_ENTRY + BINARY_ENTRY, files), [])

    def test_a_module_built_outside_tests_too_is_counted(self):
        source = '#[cfg(any(test, feature = "demo"))]\nmod demo {\n' \
                 '    #[zbus::interface(name = "org.freedesktop.portal.Other")]\n    impl A {}\n}\n'
        self.assertEqual(attribute_case(source), ["crates/cupertino-demo/src/lib.rs:3: D-Bus interface "
                                                  "org.freedesktop.portal.Other has no entry in "
                                                  "docs/architecture/contracts.yaml"])

    def test_a_test_module_that_is_not_closed_at_its_indentation_is_read(self):
        source = '#[cfg(test)]\nmod tests {\n#[zbus::interface(name = "org.freedesktop.portal.Other")]\n' \
                 'impl A {}  }\n'
        self.assertEqual(attribute_case(source), ["crates/cupertino-demo/src/lib.rs:3: D-Bus interface "
                                                  "org.freedesktop.portal.Other has no entry in "
                                                  "docs/architecture/contracts.yaml"])

    def test_a_bus_name_in_a_test_module_is_still_checked(self):
        source = '#[cfg(test)]\nmod tests {\n    const NAME: &str = "org.cupertino.Unknown1";\n}\n'
        self.assertEqual(attribute_case(source), ["crates/cupertino-demo/src/lib.rs:3: bus name "
                                                  "org.cupertino.Unknown1 has no entry in "
                                                  "docs/architecture/contracts.yaml"])


class RatchetTest(unittest.TestCase):
    def test_a_new_unknown_scope_fails(self):
        entry = INTERFACE_ENTRY.replace("scope: public", "scope: unknown")
        problems = run(entry + BINARY_ENTRY)
        self.assertEqual(problems, ["docs/architecture/contracts.yaml: 1 entries have scope unknown, at most 0 "
                                    "are allowed"])

    def test_fewer_unknown_scopes_ask_to_lower_the_limit(self):
        entries = contracts.parse(INTERFACE_ENTRY)
        with mock.patch.object(contracts, "UNKNOWN_SCOPES", 1), \
                mock.patch.object(contracts, "UNKNOWN_TEST_OWNERS", 1):
            self.assertEqual(contracts.check_ratchet(entries),
                             ["ci/contracts.py: only 0 entries have scope unknown, lower UNKNOWN_SCOPES from 1"])

    def test_more_unknown_test_owners_than_the_limit_fail(self):
        problems = run(INTERFACE_ENTRY + BINARY_ENTRY, unknown=1)
        self.assertEqual(problems, ["docs/architecture/contracts.yaml: 2 entries have test_owner unknown, at most 1 "
                                    "are allowed"])

    def test_fewer_unknown_test_owners_than_the_limit_ask_to_lower_it(self):
        problems = run(INTERFACE_ENTRY + BINARY_ENTRY, unknown=3)
        self.assertEqual(problems, ["ci/contracts.py: only 2 entries have test_owner unknown, lower "
                                    "UNKNOWN_TEST_OWNERS from 3"])


class RepositoryTest(unittest.TestCase):
    def test_the_registry_matches_this_checkout(self):
        root = Path(__file__).resolve().parent.parent
        self.assertEqual(contracts.check(root), [])


if __name__ == "__main__":
    unittest.main()
