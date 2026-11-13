# Instructions for agents working on Cupertino

Short rules for any agent (and any person) who edits this repository. Details live in each crate's README and in `docs/`.

## Rules

1. **Licence.** The project is `GPL-3.0-or-later`; `cupertino-wayland-client` and the crates in its normal dependency closure are `MPL-2.0` (see CONTRIBUTING.md). Contributions need a signed CLA.
2. **Plain design values.** A constant is a named design value with a comment about what it controls.
3. **Shader and physics math stays as it is.** Change it only with a maintainer's agreement and a test that shows why.
4. **Layers.** A crate depends only on layers below it (`cargo run -p cupertino-layers` checks). Only `cupertino-server` and `cupertino-seat` (which implements Smithay's session trait) depend on Smithay. Model crates are pure logic, tested without a display.
5. **Names.** Crates are `cupertino-*`, files are kebab-case, user-visible terms are the project's own (overview, workspaces, launcher, control panel, file manager).
6. **Scope.** One purpose per change, no drive-by renames or reformatting, no new documents unless asked.

## Git identity and attribution

- Use the user's configured Git author and committer identity. Do not override it with an assistant or service identity.
- Do not add `Co-authored-by` trailers or other assistant attribution to commit messages.
- Rewrite existing authors or committers only when the user explicitly requests it.

## Build and test

Four commands cover the work; use them rather than hand-written cargo lines:

```bash
scripts/test             # the fast tier of the changed crates' tests (and their dependents), within budget
scripts/test --all       # the fast tier of the workspace: 5 minutes at most, 2 seconds a test
scripts/test --slow      # the slow tier (whole programs, buses, displays, measurements); CI runs it too
scripts/dev/build        # the release build of every program, one feature set, into target/release
scripts/dev/shot OUT.png [server flags]   # one headless screenshot of the current checkout (builds first)
scripts/dev/review-sheet [OUT_DIR]       # windows overlapping, dock name and menu, menus, control panel, both appearances, with crops of corners and pointers; look at it before packaging
scripts/preview          # every scenario in scenarios/, this checkout beside the installed release, changed pixels marked
scripts/done             # everything a change must pass before it is called done
```

- A change is done when `scripts/done` passes: the gate, formatting, clippy on the changed crates, a test change with
  every source change (or `NO_TEST_REASON`), the fast tier, the release build and the changed programs' screens.
  Report its last line; never call work done on a partial run.
- A test over 2 seconds fails `scripts/test`. Make it fast (a manual clock, no sleeps, no real timeouts) or move it to
  the slow tier in `.config/nextest.toml` with the reason.
- Each crate's integration tests are modules of one program, `tests/suite.rs`, written by `ci/test_suite.py --write`
  after a test file is added or removed (CI checks it). A test's name starts with its file's module
  (`cargo test -p CRATE --test suite store::`); a test that runs itself again names itself through
  `cupertino_test_bus::test_path`.
- Builds link with mold when it is installed (`ci/linker.sh`); install it (`pacman -S mold`).
- A visible change gets a scenario in `scenarios/` when none shows it.

The underlying commands, for CI and for anything the scripts do not cover:

```bash
python3 ci/build-test-fixtures.py --locked -- cargo nextest run --locked --workspace --all-features --profile ci
cargo test --workspace --all-features --doc
cargo clippy --workspace --all-targets
cargo fmt --check
cargo run -p cupertino-layers
```

- `cargo fmt --all --check` is clean, vendored crates in `third-party/` included; CI runs `cargo fmt` without `--all`.
- A test that runs over 2 minutes is killed and fails by name, in either tier. To use libtest, replace `cargo nextest run` with `cargo test` inside the fixture script above; libtest has no per-test limit. Stop any stray test `dbus-daemon` after a killed run; never touch the real session bus.
- Run the compositor nested: `cargo run -p cupertino-server -- --winit --spawn foot`, with `XDG_DATA_DIRS="$PWD/crates/cupertino-files/data:$PWD/crates/cupertino-settings/data:${XDG_DATA_DIRS:-/usr/local/share:/usr/share}"` so the menu bar and dock find the apps' desktop entries (name, icon). Headless screenshots: `crates/cupertino-server/scripts/capture.sh out.png [server flags]` (fixture home and XDG directories, cleared environment, your asset pack kept, then the checkout icon pack); never capture with your real `HOME`. One window surface at a time (Settings panes, Files, Kit controls, the overview) in light and dark at any scale: `ci/capture.sh APP/SURFACE [flags]` (`crates/cupertino-capture/README.md`).
- Tests run with the built-in font and never depend on a pack installed in your home directory; set `CUPERTINO_ASSET_DIR` only to try another pack.
- CI runs the tests a second time as the Arch package's `check()` does: binaries compiled with `CUPERTINO_THEME_DIR=/usr/share/cupertino/theme` (a directory that does not exist while testing) and `CUPERTINO_SPEC_DIR` pointing at the checkout. Reproduce it with `CUPERTINO_THEME_DIR=/usr/share/cupertino/theme CUPERTINO_SPEC_DIR="$PWD/data/theme" python3 ci/build-test-fixtures.py --locked -- cargo nextest run --locked --workspace --all-features`. A test that starts a Cupertino binary with a cleared environment builds it with `common::spawn::cleared` (`crates/cupertino-server/tests/common/spawn.rs`), which always sets `CUPERTINO_SPEC_DIR` and the empty asset search path (`CUPERTINO_ASSET_SEARCH_PATH=`, the built-in assets alone); every other test spawn sets that path itself (`crates/cupertino-server/tests/asset-isolation.rs` checks the sources); a nightly workflow runs the whole Arch container build (`packaging/ci/build-in-container.sh arch DIST --upgrade`: `check()`, the upgrade test from one package release to the next, and the same install test the release build runs) without publishing.

## Installed-session development

- When working inside the maintainer's Cupertino session, build only the affected package/binary and required
  consumers with production features. Validate implementation changes in a separate, scoped test step.
- Apply a compatible candidate by replacing and restarting only the affected application or supervised shell
  component when its work can be preserved. Keep a rollback copy, record exact artifact hashes and source state,
  and verify both the replacement and the changed interaction. Follow `docs/src/building.md#installed-session-iteration`.
- Identify the target through its live compositor/socket, parent process and process start time. Never use a
  name-wide kill: nested sessions may run the same binaries. Preserve trusted installed paths and ownership.
- A component restart can lose its transient state; defer it while the component has active work or a screen share.
  Login, authentication, system daemons and the session manager need their own scoped service-update plan.
- Test server changes nested or headless and leave activation in the installed desktop for a planned session restart.
  Do not restart the active compositor or session as part of ordinary iteration.
- Implement desktop control in-tree in Rust through Cupertino-owned protocols and the existing Wayland/Smithay
  infrastructure. Add no third-party control framework, MCP SDK, or native runtime library for this integration.
  Reuse Kit's own semantic model; retain existing accessibility integrations without making automation depend on them.
- Desktop automation work follows `docs/src/building.md#desktop-automation-development-strategy`; use semantic actions
  for navigation, actual input for input-routing tests, and observable results to verify completion.

## Scripted edits

Change files with a script only through `python3 ci/replace.py [--regex] [--count N] OLD NEW FILE...`. It exits 1, names the file and writes nothing when a file does not hold OLD (with `--count`, exactly N times), so an edit whose pattern drifted cannot pass silently. It prints the changed lines; read them back. Its tests: `python3 -I ci/test_replace.py`.

## Git and disk

- Do not push or merge unless asked.
- Check `df -h /home` first: a full build needs tens of GiB free. Never put target directories or large files in `/tmp`. The package and goldens containers use a per-user sccache and download cache in `~/.cache/cupertino-build/containers` (`CUPERTINO_CONTAINER_CACHE`; empty turns it off; off on GitHub Actions); packages are still built from the clean source archive in a fresh target, with `check()`.

## Finishing a task

Run the build, tests, clippy, format and layer checks above for the crates you touched, state what ran and what did not, and leave the commit to the maintainer unless told to commit.
