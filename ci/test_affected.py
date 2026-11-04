"""Tests of affected-crate selection: python3 -I ci/test_affected.py."""
from pathlib import Path
import os
import sys
from types import SimpleNamespace
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
from affected import Package, affected, main


class AffectedTests(unittest.TestCase):
    def setUp(self) -> None:
        self.packages = [
            Package("core", "crates/core", frozenset(), False),
            Package("app", "crates/app", frozenset({"core"}), True),
            Package("consumer", "crates/consumer", frozenset({"app"}), False),
            Package("unrelated", "crates/unrelated", frozenset(), False),
            Package("cupertino-server", "crates/cupertino-server", frozenset(), True),
        ]

    def test_transitive_and_runtime_dependents(self) -> None:
        self.assertEqual(affected(self.packages, ["crates/core/src/lib.rs"]),
                         ["app", "consumer", "core", "cupertino-server"])

    def test_leaf_does_not_select_dependencies_or_unrelated_crates(self) -> None:
        self.assertEqual(affected(self.packages, ["crates/consumer/tests/test.rs"]), ["consumer"])

    def test_manifest_or_shared_input_selects_every_package(self) -> None:
        for path in ("Cargo.lock", "crates/app/Cargo.toml", ".github/workflows/ci.yml", "data/theme/colors.json",
                     "third-party/library/src/lib.rs", "crates/deleted/src/lib.rs"):
            with self.subTest(path=path):
                self.assertEqual(affected(self.packages, [path]), sorted(p.name for p in self.packages))

    def test_docs_only_selects_nothing(self) -> None:
        paths = ["docs/src/index.md", "README.md", "SECURITY.md", ".github/ISSUE_TEMPLATE/bug.yml",
                 "crates/app/README.md", "crates/cupertino-server/README.md"]
        self.assertEqual(affected(self.packages, paths), [])

    def test_docs_do_not_hide_code_changes(self) -> None:
        paths = ["crates/app/README.md", "crates/consumer/src/lib.rs"]
        for changed in (paths, list(reversed(paths))):
            with self.subTest(changed=changed):
                self.assertEqual(affected(self.packages, changed), ["consumer"])

    def test_markdown_fixture_inside_crate_is_not_skipped(self) -> None:
        for path in ("crates/consumer/tests/fixture.md", "crates/consumer/tests/README.md"):
            with self.subTest(path=path):
                self.assertEqual(affected(self.packages, [path]), ["consumer"])

    def test_protocol_contract_selects_server_tests(self) -> None:
        self.assertEqual(affected(self.packages, ["docs/architecture/contracts.yaml"]), ["cupertino-server"])

    def test_rename_selects_both_sides(self) -> None:
        self.assertEqual(affected(self.packages, ["crates/consumer/old.rs", "crates/unrelated/new.rs"]),
                         ["consumer", "unrelated"])

    def test_cross_crate_font_input(self) -> None:
        packages = [Package("cupertino-assets", "crates/cupertino-assets", frozenset(), False),
                    Package("cupertino-ui-text", "crates/cupertino-ui-text", frozenset(), False)]
        self.assertEqual(affected(packages, ["crates/cupertino-assets/fonts/font.ttf"]),
                         ["cupertino-assets", "cupertino-ui-text"])

    def test_runner_selects_packages_and_propagates_failure(self) -> None:
        for check, command in (
            ("clippy", ["cargo", "clippy", "--all-targets", "--all-features", "-p", "core", "-p", "app", "--", "-D", "warnings"]),
            ("tests", ["cargo", "nextest", "run", "--all-features", "--no-fail-fast", "-p", "core", "-p", "app"]),
            ("doctests", ["cargo", "test", "--all-features", "--doc", "-p", "core", "-p", "app"]),
        ):
            with self.subTest(check=check), patch.object(sys, "argv", ["affected.py", "run", check]), \
                    patch.dict(os.environ, {"CUPERTINO_CI_PACKAGES": '["core","app"]'}), \
                    patch("affected.subprocess.run", return_value=SimpleNamespace(returncode=17)) as run:
                self.assertEqual(main(), 17)
                run.assert_called_once_with(command, check=False)

    def test_empty_selection_cannot_run_the_default_workspace(self) -> None:
        with patch.object(sys, "argv", ["affected.py", "run", "tests"]), \
                patch.dict(os.environ, {"CUPERTINO_CI_PACKAGES": '[]'}), \
                patch("affected.subprocess.run") as run, patch("sys.stderr"):
            with self.assertRaises(SystemExit) as error:
                main()
            self.assertEqual(error.exception.code, 2)
            run.assert_not_called()


if __name__ == "__main__":
    unittest.main()
