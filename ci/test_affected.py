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
from affected import Package, affected, checks, main


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
            ("tests", ["cargo", "nextest", "run", "--all-features", "--no-fail-fast", "--timings", "-p", "core", "-p", "app"]),
            ("doctests", ["cargo", "test", "--all-features", "--doc", "-p", "core", "-p", "app"]),
        ):
            with self.subTest(check=check), patch.object(sys, "argv", ["affected.py", "run", check]), \
                    patch.dict(os.environ, {"CUPERTINO_CI_PACKAGES": '["core","app"]'}), \
                    patch("affected.workspace") as workspace, \
                    patch("affected.subprocess.run", return_value=SimpleNamespace(returncode=17)) as run:
                self.assertEqual(main(), 17)
                run.assert_called_once_with(command, check=False, pass_fds=())
                workspace.assert_not_called()

    def test_resource_lease_reaches_each_check_process(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            cargo = root / "cargo"
            # Every descriptor in the list, as the wrapper passes a worktree lock and one or more slots.
            cargo.write_text('#!' + sys.executable + '\nimport os, sys\n'
                             + 'for d in os.environ["CUPERTINO_BUILD_LOCK_FD"].split(","):\n'
                             + '    os.write(int(d), (sys.argv[1] + "\\n").encode())\n')
            cargo.chmod(0o700)
            script = Path(__file__).resolve().with_name("affected.py")
            with (root / "lease").open("w+b") as lease, (root / "slot").open("w+b") as slot:
                descriptors = (lease.fileno(), slot.fileno())
                env = dict(os.environ, PATH=f"{root}:{os.environ['PATH']}", CUPERTINO_CI_PACKAGES='["core"]',
                           CUPERTINO_BUILD_LOCK_FD=",".join(map(str, descriptors)))
                for check in ("clippy", "tests", "doctests"):
                    with self.subTest(check=check):
                        result = subprocess.run([sys.executable, str(script), "run", check], env=env,
                                                pass_fds=descriptors, capture_output=True, text=True)
                        self.assertEqual(result.returncode, 0, result.stderr)
                for stream in (lease, slot):
                    stream.seek(0)
                    self.assertEqual(stream.read(), b"clippy\nnextest\ntest\n")

    def test_fixture_and_test_commands_use_the_same_expanded_package_union(self) -> None:
        packages = self.packages + [
            Package("cupertino-kit", "crates/cupertino-kit", frozenset(), True),
            Package("cupertino-wayland-client", "crates/cupertino-wayland-client", frozenset(), True),
            Package("cupertino-ui-abi", "crates/cupertino-ui-abi", frozenset(), False),
        ]
        for selected, expected in (
            (["cupertino-server"], ["app", "cupertino-kit", "cupertino-server", "cupertino-wayland-client"]),
            (["cupertino-wayland-client"], ["cupertino-kit", "cupertino-wayland-client"]),
            (["cupertino-ui-abi"], ["cupertino-ui-abi"]),
            (["unrelated", "cupertino-server"],
             ["app", "cupertino-kit", "cupertino-server", "cupertino-wayland-client", "unrelated"]),
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
                self.assertEqual(command[split + 1:], ["cargo", "nextest", "run", "--all-features", "--no-fail-fast", "--timings",
                                                     *[value for package in expected for value in ("-p", package)]])

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
