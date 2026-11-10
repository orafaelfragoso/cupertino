# Instructions for agents working on Cupertino

Short rules for any agent (and any person) who edits this repository. Details live in each crate's README and in `docs/`.

## Rules

1. **Licence.** The project is `GPL-3.0-or-later`; `cupertino-wayland-client` and the crates in its normal dependency closure are `MPL-2.0` (see CONTRIBUTING.md). Contributions need a signed CLA.
2. **Plain design values.** A constant is a named design value with a comment about what it controls.
3. **Shader and physics math stays as it is.** Change it only with a maintainer's agreement and a test that shows why.
4. **Layers.** A crate depends only on layers below it (`cargo run -p cupertino-layers` checks). Only `cupertino-server` and `cupertino-seat` (which implements Smithay's session trait) depend on Smithay. Model crates are pure logic, tested without a display.
5. **Names.** Crates are `cupertino-*`, files are kebab-case, user-visible terms are the project's own (overview, workspaces, launcher, control panel, file manager).
6. **Scope.** One purpose per change, no drive-by renames or reformatting, no new documents unless asked.

## Build and test

For build/install-only work, use the managed wrapper with `cargo build --locked -p PACKAGE --bin BINARY` and only the
production features the binary needs (usually `--features app`). Do not add test execution, `--tests`, `--all-targets`
or the test-fixture wrapper to an ordinary development build. Validation of implementation changes is a separate,
scoped test step; package selection on the fixture wrapper and on its child command are independent.

```bash
export PATH=$HOME/.cargo/bin:$PATH
python3 ci/worktree-build.py run --profile dev -- cargo build --workspace
python3 ci/worktree-build.py run --profile dev -- python3 ci/build-test-fixtures.py --locked -- cargo nextest run --locked --workspace --all-features
python3 ci/worktree-build.py run --profile dev -- cargo test --workspace --all-features --doc
python3 ci/worktree-build.py run --profile dev -- cargo clippy --workspace --all-targets
cargo fmt --check
python3 ci/worktree-build.py run --profile dev -- cargo run -p cupertino-layers
```

- `cargo fmt --all --check` is clean, vendored crates in `third-party/` included; CI runs `cargo fmt` without `--all`.
- A test that runs over 2 minutes is killed and fails by name. To use libtest, replace `cargo nextest run` with `cargo test` inside the fixture wrapper above; libtest has no per-test limit. Stop any stray test `dbus-daemon` after a killed run; never touch the real session bus.
- Run the compositor nested: `cargo run -p cupertino-server -- --winit --spawn foot`, with `XDG_DATA_DIRS="$PWD/crates/cupertino-files/data:$PWD/crates/cupertino-settings/data:${XDG_DATA_DIRS:-/usr/local/share:/usr/share}"` so the menu bar and dock find the apps' desktop entries (name, icon). Headless screenshots: `crates/cupertino-server/scripts/capture.sh out.png [server flags]` (fixture home and XDG directories, cleared environment, your asset pack kept, then the checkout icon pack); never capture with your real `HOME`. One window surface at a time (Settings panes, Files, Kit controls, the overview) in light and dark at any scale: `ci/capture.sh APP/SURFACE [flags]` (`crates/cupertino-capture/README.md`).
- Tests run with the built-in font and never depend on a pack installed in your home directory; set `CUPERTINO_ASSET_DIR` only to try another pack.
- CI runs the tests a second time as the Arch package's `check()` does: binaries compiled with `CUPERTINO_THEME_DIR=/usr/share/cupertino/theme` (a directory that does not exist while testing) and `CUPERTINO_SPEC_DIR` pointing at the checkout. With a fresh managed target, reproduce it with `CUPERTINO_THEME_DIR=/usr/share/cupertino/theme CUPERTINO_SPEC_DIR="$PWD/data/theme" python3 ci/worktree-build.py run --profile dev -- python3 ci/build-test-fixtures.py --locked -- cargo nextest run --locked --workspace --all-features`. A test that starts a Cupertino binary with a cleared environment builds it with `common::spawn::cleared` (`crates/cupertino-server/tests/common/spawn.rs`), which always sets `CUPERTINO_SPEC_DIR` and the empty asset search path (`CUPERTINO_ASSET_SEARCH_PATH=`, the built-in assets alone); every other test spawn sets that path itself (`crates/cupertino-server/tests/asset-isolation.rs` checks the sources); a nightly workflow runs the whole Arch container build (`packaging/ci/build-in-container.sh arch DIST --upgrade`: `check()`, the upgrade test from one package release to the next, and the same install test the release build runs) without publishing.

## Scripted edits

Change files with a script only through `python3 ci/replace.py [--regex] [--count N] OLD NEW FILE...`. It exits 1, names the file and writes nothing when a file does not hold OLD (with `--count`, exactly N times), so an edit whose pattern drifted cannot pass silently. It prints the changed lines; read them back. Its tests: `python3 -I ci/test_replace.py`.

## Worktrees and disk

- Work in your own git worktree under `~/Development/cupertino-worktrees/<name>`, on its own branch. Do not work in the main checkout. Do not push or merge unless asked.
- Use `python3 ci/worktree-build.py run --profile dev -- COMMAND` for local Cargo builds and the fixture wrapper above. It keeps `target/` private to this worktree, disables incremental compilation, checks a 30 GiB free-space floor and holds one of N per-user build slots (default 4; `--slots N` or `CUPERTINO_BUILD_SLOTS`) for the whole child lifetime, giving the child about 1/N of the processors (`CARGO_BUILD_JOBS`, `NEXTEST_TEST_THREADS`, `RUST_TEST_THREADS`; a value you set wins). Neither changes seed compatibility. Operations in one worktree run one at a time. Keep the same profile and build environment inputs throughout a target lifecycle. Do not nest this wrapper.
- Fingerprint rule: a target or seed is refused only when something Cargo does not check crate by crate changed: host/OS, rustc/cargo versions, profile, `rust-toolchain` files, Cargo configuration, the root `[profile.*]` tables, or build environment variables (Cargo/Rust/sccache/native tool settings and the compile-time `CUPERTINO_THEME_DIR`, `CUPERTINO_INSTALL_PREFIX`, `CUPERTINO_BUILD_COMMIT`, `CUPERTINO_ADMIN_GROUPS`). Adding a dependency or changing `Cargo.lock` keeps the target; Cargo rebuilds what it affects. Test-only variables (`CUPERTINO_UPDATE_GOLDENS`, `CUPERTINO_REPAINT_*`, `CUPERTINO_SPEC_DIR`, `RUST_LOG`, …) never change it. A new variable read by a build script or `env!`/`option_env!` goes into `PROJECT_BUILD_VARIABLES` in `ci/worktree-build.py`; its test enforces that. Targets and seeds from the older wrapper (schema 1) are reused only while their inputs are exactly unchanged.
- `prepare --profile dev --seed NAME` clones an explicitly promoted compatible seed with `cp --reflink=always`, after giving every worktree file not newer than the seed the current time (Cargo compares file times, not contents); incompatible toolchains, profiles, Cargo configuration or environments fail visibly. Existing unmanaged targets are never adopted. `promote --profile dev --name NAME --max-seed-gib 32` (a dev target is about 29 GiB) snapshots only a successful managed run. After each merge into an integration branch, once `run --profile dev -- cargo test --workspace --all-features --no-run` succeeded there, the orchestrator runs `refresh-seed --profile dev --name NAME --max-seed-gib 32`: it replaces the seed of that name atomically (staging copy, then one exchange) under every slot, without waiting for it to expire. One seed per repository expires after seven days; the explicit size budget bounds its apparent size, while free space measures actual filesystem allocation.
- Finish with `python3 ci/worktree-build.py cleanup --target --expired-seeds`. This removes only this worktree’s marked target and expired managed seeds. It never sweeps other checkouts. `prepare --seed`, `promote`, `refresh-seed`, `cleanup --expired-seeds` and `locked` take every slot. For package/container scripts use `python3 ci/worktree-build.py locked -- COMMAND` once around the outermost operation; it preserves their target layout. `locked --hold none` holds only the worktree, so the script can take `locked --hold every|one` per phase: `packaging/ci/build-in-container.sh` holds every slot only while makepkg compiles and tests. All local heavyweight work must cooperate with the slots; raw Cargo bypasses the guard.
- Check `df -h /home` first: do not start a build with less than 30 GiB free. Never put target directories or large files in `/tmp`. Cargo downloads remain shared in `CARGO_HOME`; the optional host sccache setup is documented in CONTRIBUTING.md. The package and goldens containers use a per-user sccache and download cache in `~/.cache/cupertino-build/containers` (`CUPERTINO_CONTAINER_CACHE`; empty turns it off; off on GitHub Actions); packages are still built from the clean source archive in a fresh target, with `check()`.

## Finishing a task

Run the build, tests, clippy, format and layer checks above for the crates you touched, state what ran and what did not, and leave the commit to the maintainer unless told to commit.
