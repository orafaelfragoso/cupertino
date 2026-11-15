"""Tests of the performance gate's choices, without a session: `python3 -I scripts/dev/test_perf_gate.py`."""
import importlib.machinery
import importlib.util
import pathlib
import unittest

HERE = pathlib.Path(__file__).resolve().parent
loader = importlib.machinery.SourceFileLoader("perf_gate", str(HERE / "perf-gate"))
spec = importlib.util.spec_from_loader("perf_gate", loader)
gate = importlib.util.module_from_spec(spec)
loader.exec_module(gate)

Package = gate.affected.Package


def packages():
    return [
        Package("cupertino-ui-text", "crates/cupertino-ui-text", frozenset(), False),
        Package("cupertino-kit", "crates/cupertino-kit", frozenset({"cupertino-ui-text"}), False),
        Package("cupertino-files", "crates/cupertino-files", frozenset({"cupertino-kit"}), True),
        Package("cupertino-launcher", "crates/cupertino-launcher", frozenset({"cupertino-kit"}), True),
        Package("cupertino-glass", "crates/cupertino-glass", frozenset(), False),
        Package("cupertino-server", "crates/cupertino-server", frozenset({"cupertino-glass"}), True),
        Package("cupertino-netd", "crates/cupertino-netd", frozenset(), True),
    ]


class MappingTest(unittest.TestCase):
    def scenarios(self, *changed):
        return gate.scenarios_for(gate.crates_changed(packages(), list(changed)))

    def test_files_maps_to_its_scroll_and_its_resize(self):
        got = self.scenarios("crates/cupertino-files/src/list.rs")
        self.assertIn("files-scroll", got)
        self.assertIn("resize", got)
        self.assertNotIn("launcher", got)

    def test_the_launcher_maps_to_its_open_and_its_typing(self):
        self.assertEqual(self.scenarios("crates/cupertino-launcher/src/view.rs"), ["idle", "launcher-typing", "launcher"])

    def test_the_server_and_the_glass_map_to_every_scenario(self):
        self.assertEqual(self.scenarios("crates/cupertino-server/src/render.rs"), list(gate.EVERY))
        self.assertEqual(self.scenarios("crates/cupertino-glass/src/lib.rs"), list(gate.EVERY))

    def test_a_library_maps_through_the_programs_built_from_it(self):
        got = self.scenarios("crates/cupertino-ui-text/src/shape.rs")
        self.assertTrue({"files-scroll", "launcher", "launcher-typing"} <= set(got))
        self.assertNotIn("overview", got)

    def test_a_program_the_server_only_launches_in_its_tests_does_not_select_the_server(self):
        self.assertEqual(self.scenarios("crates/cupertino-netd/src/main.rs"), [])

    def test_tests_docs_scripts_and_ci_select_nothing(self):
        self.assertEqual(self.scenarios("crates/cupertino-files/tests/list.rs", "crates/cupertino-launcher/README.md",
                                        "docs/src/building.md", ".config/nextest.toml", "ci/gate.py",
                                        "scripts/dev/bench", "packaging/arch/PKGBUILD", ".github/workflows/ci.yml"), [])

    def test_a_manifest_or_the_sessions_shell_list_selects_every_scenario(self):
        self.assertEqual(self.scenarios("Cargo.lock"), list(gate.EVERY))
        self.assertEqual(self.scenarios("crates/cupertino-files/Cargo.toml"), list(gate.EVERY))
        self.assertEqual(self.scenarios(gate.SESSION_CONFIG), list(gate.EVERY))

    def test_every_scenario_the_table_names_is_one_of_the_benchs(self):
        named = {s for v in gate.SCENARIOS.values() for s in v}
        self.assertEqual(set(gate.EVERY), set(gate.bench.BASELINE + gate.bench.EXTRA))
        self.assertTrue(named <= set(gate.EVERY))


class SummaryTest(unittest.TestCase):
    def test_a_failure_shows_the_heading_and_the_first_rows_of_both_tables(self):
        rows = "\n".join(f"  {30 - i:6.2f}%  f{i}  [a]" for i in range(30))
        text = ("== cupertino-launcher (pid 7): 900 samples of cycles:u, about 0.45 s of CPU\n"
                "   perf report -i /x/s.perf.data --pid 7\n\nthreads, by their share of the samples:\n"
                "  100.00%  cupertino-launc\n\n"
                "top 30 functions by self time (in the function itself):\n   self%  function  [object]\n"
                f"{rows}\n\ntop 30 functions by inclusive time (the function and what it calls):\n"
                f"   incl%   self%  function  [object]\n{rows}\n")
        shown = gate.summary_lines(text, shown=3)
        self.assertIn("== cupertino-launcher (pid 7)", shown)
        self.assertIn("perf report -i /x/s.perf.data --pid 7", shown)
        self.assertNotIn("cupertino-launc\n", shown)
        self.assertEqual(shown.count("f0  [a]"), 2)
        self.assertEqual(shown.count("f2  [a]"), 2)
        self.assertNotIn("f3  [a]", shown)

    def test_the_baseline_is_kept_per_machine_size_and_scale(self):
        path = gate.baseline_path()
        self.assertEqual(path.parent, gate.ROOT / "target/bench/baseline")
        self.assertTrue(path.name.endswith("-3840x2160-1.5.json"))


if __name__ == "__main__":
    unittest.main()
