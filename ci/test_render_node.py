"""Tests of the render node check scripts/test --slow and scripts/done make: python3 -I ci/test_render_node.py."""
import os
import pathlib
import subprocess
import sys
import tempfile
import unittest

HERE = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import render_node  # noqa: E402


class RenderNodeTests(unittest.TestCase):
    def setUp(self) -> None:
        self.dir = tempfile.TemporaryDirectory()
        self.root = pathlib.Path(self.dir.name)

    def tearDown(self) -> None:
        self.dir.cleanup()

    def node(self, name: str, mode: int = 0o666) -> pathlib.Path:
        path = self.root / name
        path.write_bytes(b"")
        path.chmod(mode)
        return path

    def test_the_first_render_node_that_opens_is_used_and_other_device_files_are_not(self) -> None:
        self.node("card0")
        self.node("renderD129")
        first = self.node("renderD128")
        self.assertEqual(render_node.usable(self.root, {}), first)

    def test_a_node_that_does_not_open_is_passed_over(self) -> None:
        if os.geteuid() == 0:
            self.skipTest("root opens any file")
        self.node("renderD128", 0o000)
        second = self.node("renderD129")
        self.assertEqual(render_node.usable(self.root, {}), second)
        second.chmod(0o444)
        self.assertIsNone(render_node.usable(self.root, {}), "a node is opened for writing too")

    def test_no_node_no_directory_or_a_forced_software_renderer_is_none(self) -> None:
        self.assertIsNone(render_node.usable(self.root, {}))
        self.assertIsNone(render_node.usable(self.root / "missing", {}))
        self.node("renderD128")
        self.assertIsNone(render_node.usable(self.root, {"LIBGL_ALWAYS_SOFTWARE": "1"}))

    def test_a_usable_node_sets_the_requirement_and_the_line_says_so(self) -> None:
        self.node("renderD128")
        env = {}
        line = render_node.require(env, self.root)
        self.assertEqual(env, {"CUPERTINO_GPU_REQUIRED": "1"})
        self.assertIn("renderD128", line)
        self.assertIn("cannot skip", line)

    def test_without_a_node_nothing_is_set_and_the_tests_skip(self) -> None:
        env = {}
        line = render_node.require(env, self.root)
        self.assertEqual(env, {})
        self.assertIn("skip", line)

    def test_a_value_the_caller_set_is_kept(self) -> None:
        self.node("renderD128")
        env = {"CUPERTINO_GPU_REQUIRED": "0"}
        line = render_node.require(env, self.root)
        self.assertEqual(env, {"CUPERTINO_GPU_REQUIRED": "0"})
        self.assertIn("as given", line)

    def test_the_export_line_sets_the_variable_only_when_the_caller_did_not(self) -> None:
        script = str(HERE / "render_node.py")
        given = dict(os.environ, CUPERTINO_GPU_REQUIRED="0")
        out = subprocess.run([sys.executable, "-I", script, "--export"], env=given, capture_output=True, text=True)
        self.assertEqual((out.returncode, out.stdout), (0, ""))
        self.assertIn("as given", out.stderr)
        software = {k: v for k, v in os.environ.items() if k != "CUPERTINO_GPU_REQUIRED"}
        software["LIBGL_ALWAYS_SOFTWARE"] = "1"
        out = subprocess.run([sys.executable, "-I", script, "--export"], env=software, capture_output=True, text=True)
        self.assertEqual((out.returncode, out.stdout), (0, ""))
        self.assertIn("skip", out.stderr)


if __name__ == "__main__":
    unittest.main()
