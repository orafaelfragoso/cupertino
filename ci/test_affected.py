"""Tests of affected-crate selection: python3 -I ci/test_affected.py."""
from pathlib import Path
import json
import os
import subprocess
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
from affected import Fixtures, Package, affected, checks, fixture_packages, fixture_use, main, workspace


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

    def test_agent_skills_and_assistant_configuration_select_nothing(self) -> None:
        paths = [".agents/skills/review/SKILL.md", ".agents/skills/review/agents/openai.yaml", ".claude/skills/review",
                 ".claude/settings.json", ".codex/config.toml", ".mcp.json", "CLAUDE.md"]
        self.assertEqual(affected(self.packages, paths), [])
        self.assertEqual({name for name, enabled in checks(paths).items() if enabled}, set())

    def test_agent_paths_do_not_hide_code_or_shared_inputs(self) -> None:
        self.assertEqual(affected(self.packages, [".agents/skills/review/SKILL.md", "crates/consumer/src/lib.rs"]),
                         ["consumer"])
        self.assertEqual(affected(self.packages, [".mcp.json", "crates/app/.mcp.json"]),
                         ["app", "consumer", "cupertino-server"])
        for path in (".cargo/config.toml", ".github/workflows/ci.yml", "data/.agents/x"):
            with self.subTest(path=path):
                self.assertEqual(affected(self.packages, [".mcp.json", path]), sorted(p.name for p in self.packages))

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
            ("clippy", ["cargo", "clippy", "--all-targets", "--all-features", "--timings", "-p", "core", "-p", "app", "--", "-D", "warnings"]),
            ("tests", ["cargo", "nextest", "run", "--all-features", "--no-fail-fast", "--timings", "--profile", "ci", "-p", "core", "-p", "app"]),
            ("doctests", ["cargo", "test", "--all-features", "--doc", "-p", "core", "-p", "app"]),
        ):
            with self.subTest(check=check), patch.object(sys, "argv", ["affected.py", "run", check]), \
                    patch.dict(os.environ, {"CUPERTINO_CI_PACKAGES": '["core","app"]'}), \
                    patch("affected.workspace", return_value=self.packages) as workspace, \
                    patch("affected.subprocess.run", return_value=SimpleNamespace(returncode=17)) as run:
                self.assertEqual(main(), 17)
                run.assert_called_once_with(command, check=False)
                # Only tests look artifacts up; none of these packages does, so no preparation wraps them.
                self.assertEqual(workspace.called, check == "tests")

    def test_fixture_and_test_commands_use_the_same_expanded_package_union(self) -> None:
        packages = self.fixture_workspace()
        for selected, expected in (
            (["cupertino-server"], ["app", "cupertino-kit", "cupertino-server", "cupertino-test-pam-shim",
                                    "cupertino-wayland-client", "login"]),
            (["cupertino-wayland-client"], ["cupertino-kit", "cupertino-wayland-client"]),
            (["cupertino-ui-abi"], ["cupertino-ui-abi"]),
            (["login"], ["cupertino-test-pam-shim", "login"]),
            (["unrelated", "cupertino-server"],
             ["app", "cupertino-kit", "cupertino-server", "cupertino-test-pam-shim", "cupertino-wayland-client",
              "login", "unrelated"]),
        ):
            with self.subTest(selected=selected), patch.object(sys, "argv", ["affected.py", "run", "tests"]), \
                    patch.dict(os.environ, {"CUPERTINO_CI_PACKAGES": json.dumps(selected)}), \
                    patch("affected.workspace", return_value=packages), \
                    patch("affected.subprocess.run", return_value=SimpleNamespace(returncode=17)) as run:
                self.assertEqual(main(), 17)
                command = run.call_args.args[0]
                split = command.index("--")
                self.assertEqual(command[:3], [sys.executable,
                                              str(Path(__file__).resolve().with_name("build-test-fixtures.py")),
                                              "--locked"])
                self.assertEqual(command[3:split], [value for package in expected for value in ("--package", package)])
                self.assertEqual(command[split + 1:], ["cargo", "nextest", "run", "--all-features", "--no-fail-fast", "--timings", "--profile", "ci",
                                                     *[value for package in expected for value in ("-p", package)]])

    def fixture_workspace(self) -> list[Package]:
        own = Fixtures(tests=True)
        return [p for p in self.packages if p.name != "cupertino-server"] + [
            Package("cupertino-server", "crates/cupertino-server", frozenset({"test-pam"}), True,
                    Fixtures(tests=True, providers=frozenset({"app"}))),
            Package("cupertino-kit", "crates/cupertino-kit", frozenset(), True),
            Package("cupertino-wayland-client", "crates/cupertino-wayland-client", frozenset(), True,
                    Fixtures(tests=True, providers=frozenset({"cupertino-wayland-client"}))),
            Package("cupertino-ui-abi", "crates/cupertino-ui-abi", frozenset(), False, own),
            # A library that looks the shim up for the tests of every package depending on it.
            Package("test-pam", "crates/test-pam", frozenset(), False,
                    Fixtures(tests=True, library=True, providers=frozenset({"cupertino-test-pam-shim"}))),
            Package("cupertino-test-pam-shim", "crates/cupertino-test-pam-shim", frozenset(), False),
            Package("login", "crates/login", frozenset({"auth"}), True),
            Package("auth", "crates/auth", frozenset({"test-pam"}), False),
        ]

    def test_artifacts_named_by_a_dependency_library_are_prepared_for_its_dependents(self) -> None:
        # The shim reached only through the library: missing it fails every PAM test with a missing fixture.
        packages = self.fixture_workspace()
        for selected in (["login"], ["auth"], ["test-pam"]):
            with self.subTest(selected=selected):
                self.assertIn("cupertino-test-pam-shim", fixture_packages(packages, selected))

    def test_packages_that_look_no_artifact_up_run_without_preparation(self) -> None:
        packages = self.fixture_workspace()
        self.assertIsNone(fixture_packages(packages, ["core", "app", "unrelated"]))
        # A test fixture's own tests in another package do not make its dependents consumers.
        self.assertIsNone(fixture_packages(packages + [
            Package("kit-user", "crates/kit-user", frozenset({"cupertino-wayland-client"}), False)], ["kit-user"]))

    def test_fixture_use_reads_the_helper_include_and_named_lookups(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "src").mkdir()
            (root / "tests").mkdir()
            include = 'include!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../ci/test-fixtures.rs"));\n'
            (root / "tests" / "run.rs").write_text(include + 'fn f() { fixtures::artifact(\n    "files", "bin", "x"); '
                                                   'artifact(name, "bin", name); contains("fixtures::artifact("); }\n')
            self.assertEqual(fixture_use(root), Fixtures(tests=True, library=False, providers=frozenset({"files"})))
            (root / "src" / "lib.rs").write_text(include)
            self.assertEqual(fixture_use(root), Fixtures(tests=True, library=True, providers=frozenset({"files"})))

    def test_workspace_fixture_consumers_get_every_artifact_they_look_up(self) -> None:
        packages = {package.name: package for package in workspace()}
        consumers = [name for name, package in packages.items() if package.fixtures.tests]
        self.assertTrue({"cupertino-server", "cupertino-wayland-client", "cupertino-test-pam"} <= set(consumers))
        for name in sorted(packages):
            prepared = fixture_packages(list(packages.values()), [name])
            with self.subTest(package=name):
                if name in consumers:
                    self.assertIsNotNone(prepared)
                if prepared is not None:
                    for provider in packages[name].fixtures.providers & packages.keys():
                        self.assertIn(provider, prepared)
        for name in ("cupertino-login", "cupertino-sessiond", "cupertino-server"):
            with self.subTest(package=name):
                self.assertIn("cupertino-test-pam-shim", fixture_packages(list(packages.values()), [name]))

    def test_empty_selection_cannot_run_the_default_workspace(self) -> None:
        with patch.object(sys, "argv", ["affected.py", "run", "tests"]), \
                patch.dict(os.environ, {"CUPERTINO_CI_PACKAGES": '[]'}), \
                patch("affected.subprocess.run") as run, patch("sys.stderr"):
            with self.assertRaises(SystemExit) as error:
                main()
            self.assertEqual(error.exception.code, 2)
            run.assert_not_called()

    def test_cheap_gates_follow_their_inputs(self) -> None:
        for paths, expected in (
            (["README.md"], set()),
            (["docs/src/index.md"], {"docs"}),
            (["crates/core/src/lib.rs"], {"fmt"}),
            (["crates/core/Cargo.toml"], {"deny", "fmt", "layers"}),
            (["crates/core/LICENSE"], {"deny", "layers"}),
            (["third-party/library/LICENSE"], {"deny", "layers"}),
            (["third-party/smithay/LICENSE.txt"], {"deny", "layers"}),
            (["third-party/library/COPYING"], {"deny", "layers"}),
            (["crates/cupertino-assets/fonts/OFL.txt"], {"deny", "layers"}),
            (["packaging/licenses/THIRD-PARTY-LICENSES"], {"deny", "layers"}),
            ([".cargo/config.toml"], {"deny", "fmt", "layers"}),
            (["deny.toml"], {"deny"}),
            (["rust-toolchain.toml"], {"fmt", "layers"}),
            ([".github/workflows/ci.yml"], {"docs", "deny", "fmt", "layers"}),
            (["ci/affected.py"], {"docs", "deny", "fmt", "layers"}),
        ):
            with self.subTest(paths=paths):
                self.assertEqual({name for name, enabled in checks(paths).items() if enabled}, expected)

    def test_gates_do_not_need_cargo_metadata(self) -> None:
        with patch.object(sys, "argv", ["affected.py", "gates", "base", "head"]), \
                patch("affected.changed_paths", return_value=["docs/src/index.md"]), \
                patch("affected.workspace") as workspace, patch("sys.stdout"):
            self.assertEqual(main(), 0)
            workspace.assert_not_called()


if __name__ == "__main__":
    unittest.main()
