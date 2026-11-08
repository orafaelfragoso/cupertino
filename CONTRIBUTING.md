# Contributing to Cupertino

Thanks for helping. This page covers how to build, what a good change looks like and what we cannot accept.

## Build and test

```bash
python3 ci/worktree-build.py run --profile dev -- cargo build --workspace
python3 ci/worktree-build.py run --profile dev -- python3 ci/build-test-fixtures.py --locked -- cargo nextest run --locked --workspace --all-features
python3 ci/worktree-build.py run --profile dev -- cargo test --workspace --all-features --doc
python3 ci/worktree-build.py run --profile dev -- cargo run -p cupertino-layers
python3 ci/worktree-build.py run --profile dev -- cargo clippy --workspace --all-targets
cargo fmt --check
```

Notes:

- Run `cargo fmt` without `--all`, as CI does: `--all` also formats the vendored crates in `third-party/`, which stay as their authors wrote them (and one is missing the test files that its sources declare).
- Shell and app crates build their process code behind the `app` feature. Use `cargo test -p <crate> --features app` for those.
- Install [cargo-nextest](https://nexte.st) (`cargo install cargo-nextest --locked`; CI uses 0.9.151). It runs every test in its own process and kills a test that runs longer than 2 minutes (`.config/nextest.toml`), so a hang fails with the test's name. To use libtest, replace `cargo nextest run` with `cargo test` inside the preparation wrapper above; libtest has no per-test limit. A killed run can leave a test `dbus-daemon` behind: stop it, never your real session bus.
- Work in a linked worktree. `ci/worktree-build.py` keeps its own writable `target/`, disables incremental compilation, requires 30 GiB free before a build and holds one of N per-user build slots until its child exits (default 2; `--slots N` or `CUPERTINO_BUILD_SLOTS`; slot 0 is `~/.cache/cupertino-build/heavy.lock`, which older copies of the script take as their single lock). Waiters are served in arrival order, and operations in one worktree run one at a time. The child gets about 1/N of the processors through `CARGO_BUILD_JOBS`, `NEXTEST_TEST_THREADS` and `RUST_TEST_THREADS` unless you set them; neither these nor N change seed compatibility. `prepare --seed`, `promote`, `cleanup --expired-seeds` and `locked` take every slot. Cargo downloads remain shared in `CARGO_HOME`. Use the wrapper for all local heavyweight builds; direct Cargo commands bypass its guard. For an outer package/container script use `python3 ci/worktree-build.py locked -- COMMAND` once; it preserves that script’s target layout. Do not nest invocations.
- `run --profile dev` prepares a cold target automatically and sets matching profiles on Cargo and `ci/build-test-fixtures.py`. Use the same profile and build environment inputs for later commands; a changed build input requires cleanup and a new preparation. The fixture wrapper prebuilds the requested binary/example/library fixtures once, then runs tests with a checked artifact manifest. Individual tests use the same wrapper with nextest selection arguments. Existing unmanaged targets are refused rather than adopted.
- Run the compositor nested while you work: `cargo run -p cupertino-server -- --winit --spawn foot`. Export `XDG_DATA_DIRS="$PWD/crates/cupertino-files/data:$PWD/crates/cupertino-settings/data:${XDG_DATA_DIRS:-/usr/local/share:/usr/share}"` first, or the menu bar and dock show `Cupertino-files` and a generic icon instead of the apps' names and icons (see [Building](docs/src/building.md)). Headless screenshots are the quickest visual check and work in CI. Take them with `crates/cupertino-server/scripts/capture.sh out.png --spawn cupertino-files`: it runs the whole desktop with a temporary sample home folder, so a screenshot never shows your own files (see the script header for more examples).
- Try the full desktop from a text console: `./run-cupertino.sh` (switch with `Ctrl+Alt+F3`). It builds the release binaries and starts a fresh session bus; the log is `~/cupertino-tty.log`, `Ctrl+Alt+Backspace` quits, and `Ctrl+Alt+F<n>` returns to your previous console. See [Building and running](docs/src/building.md#the-whole-desktop-on-a-tty).

### Optional local cache lifecycle

Promote a successful managed build as a warm seed, then remove its writable target when the task is finished:

```bash
python3 ci/worktree-build.py promote --profile dev --name host-dev --max-seed-gib 12
python3 ci/worktree-build.py cleanup --target --expired-seeds
```

In another linked worktree, `python3 ci/worktree-build.py prepare --profile dev --seed host-dev` clones that seed using `cp --reflink=always`. A filesystem without reflinks fails visibly. Seeds live in the shared Git directory, are never build destinations, and are compatible only when host, Rust/Cargo versions, profile, manifests, Cargo configuration and known build environment inputs match. The environment scope includes PATH/home, Cargo/Rust/toolchain settings, project variables such as `CUPERTINO_THEME_DIR`, `CUPERTINO_SPEC_DIR` and `CUPERTINO_INSTALL_PREFIX`, sccache settings, and native compiler/linker/build-tool variables including target-specific flags. Unrelated agent/session identifiers, terminal settings, general authentication tokens, SSH sockets and fixture leases are excluded. Cargo still checks source freshness and its tracked environment inputs after cloning. Host and container targets are not interchangeable; the separate `target/package-build/` subtree is excluded from host seeds and their size budget. Keep paths stable where compile-time variables embed paths; no path remapping is applied.

There is at most one seed per repository, retained for seven days. Promotion requires an explicit apparent-size budget in GiB; reflink sharing is accounted for by the actual free-space check, not by reserving the logical copy size. Cleanup removes only this worktree’s marked target and expired managed seeds; removing seeds holds every build slot. It does not scan or delete unrelated worktrees, Cargo downloads or active managed builds. A disk check runs before preparation, build and promotion; it does not reserve disk space against unrelated processes or stop a build that grows beyond the floor.

The optional compiler-cache pilot uses the installed sccache 0.19.0 with a dedicated local server and a 2 GiB cache. Set these before preparing a fresh target, keep the environment for every run, and compare counters before and after builds in two worktrees:

```bash
mkdir -p "$HOME/.cache/cupertino-build"
export RUSTC_WRAPPER="$(command -v sccache)"
export SCCACHE_CONF=/dev/null
export SCCACHE_DIR="$HOME/.cache/cupertino-build/sccache"
export SCCACHE_CACHE_SIZE=2G
export SCCACHE_SERVER_UDS="$HOME/.cache/cupertino-build/sccache.sock"
sccache --show-stats
```

Run the managed build commands above, then `sccache --show-stats` again. This is a pilot, not an assumed speedup: report Rust hits, misses, non-cacheable invocations and elapsed time. Linking binaries and procedural macros is not cached; cross-worktree Rust hits must be measured. The dedicated server prevents changing another sccache session. Stop it with the same environment and `sccache --stop-server` when the pilot ends. See the [sccache configuration](https://github.com/mozilla/sccache/blob/v0.19.0/docs/Configuration.md) and [Rust limitations](https://github.com/mozilla/sccache/blob/v0.19.0/docs/Rust.md).

## Docs

The book in `docs/` builds with mdBook 0.5.4 and checks its internal links and anchors with lychee 0.24.2:

```bash
cargo install mdbook --version 0.5.4 --locked
cargo install lychee --version 0.24.2 --locked
mdbook build docs
lychee --offline --no-progress --include-fragments 'docs/src/**/*.md'
```

A broken link or anchor in the book fails both commands. CI installs the same two versions from their released binaries (`.github/workflows/ci.yml`). The link check runs offline, so external web links are not checked and another site's outage cannot fail it.

## What a good change looks like

1. **One purpose.** A bug fix, a feature or a refactor, not a mix.
2. **A test.** Model crates are pure logic and cheap to test: put the behaviour there and test it without a display. Add a headless compositor test only when the behaviour needs a real client.
3. **Layers respected.** A crate may depend only on the layers below it. `cupertino-layers` fails the build on any other edge. Only `cupertino-server` and `cupertino-seat` (which implements Smithay's session trait) depend on Smithay.
4. **Plain design values.** A constant that shapes the look (a duration, a radius, a spring) is a named value in one place, with a comment that says what it controls. Do not change the shader or physics math without a maintainer's agreement.
5. **Names.** Crates are `cupertino-*`, files are kebab-case, and user-visible text uses the project's own terms (overview, workspaces, launcher, control panel, file manager).
6. **Small diffs.** No drive-by renames or reformatting outside the change.

## What we cannot accept

- Code, text, images, fonts, icons, cursors or wallpapers that are not yours to license. Contribute only work that is yours or that you have the right to license under the terms of the contributor licence agreement below.
- Third-party assets without a licence compatible with `GPL-3.0-or-later` (and with `MPL-2.0` in an MPL-2.0 crate). Asset packs are loaded from a directory and are not added to the tree.
- Binary blobs. Add the generator, not the output.

Maintainers (and a CI check, once it is enabled) reject files and strings that suggest otherwise; if a check flags your change by mistake, say so in the pull request.

## Pull requests

- Describe what changes and why in the first lines.
- For visible changes attach a screenshot (dark and light when both are affected).
- Keep commits tidy; we squash on merge when the history is noisy.
- A maintainer reviews every change. Expect questions about tests and about where numbers come from.

## CI and releases

A pull request runs `gate` and then what its diff needs: a docs-only change (`docs/`, top-level Markdown, the licence, the issue and pull request templates) builds the book; any other change runs fmt, deny, the lint and test build (`check`), the Weston test and the golden scenes. **CI result** is the check that must pass to merge. A push to `main` runs only the quick jobs, because its pull request ran the rest.

Every night, if `main` changed, **Nightly** runs every job, including `check` in the package build environment, and builds and install-tests the Arch packages. A tag `vX.Y.Z-rcN` makes a release candidate: the full CI, the Arch packages, signing and a draft prerelease. A tag `vX.Y.Z` makes the final release, with the Debian and Fedora packages and the ISO. A maintainer publishes the draft by hand. The **ISO** workflow builds an image from any branch or tag when run from the Actions tab.

## Issues

Include the version or commit, the backend (`--winit`, `--udev`), your compositor if nested, and the output of the failing command. For crashes include the log of the failing process (`cupertino-server` prints the shell process that exited).

## Licence and contributor licence agreement

The project is licensed under `GPL-3.0-or-later`, except the library crates that carry `license = "MPL-2.0"` in their manifest and a `LICENSE` file: a change to one of them is under the MPL-2.0. An MPL-2.0 crate may not depend on a GPL crate (`cargo run -p cupertino-layers` fails on it), so a dependency you add to one must be MPL-2.0 or a permissive licence. Before your first pull request can be merged, agree to the [contributor licence agreement](CLA.md).

The licence texts and NOTICE files of the registry crates in the shipped binaries are in `packaging/licenses/THIRD-PARTY-LICENSES`, generated with [cargo-about](https://github.com/EmbarkStudios/cargo-about) 0.9.3 (`cargo install cargo-about --version 0.9.3 --locked --features cli`). After a change to `Cargo.lock` that adds, removes or updates a linked crate, run `python3 packaging/release.py third-party` and commit the result; `python3 packaging/release.py check-metadata` fails on a stale or missing file. A crate whose licence notice sits in an input file that a build script compiles (as `khronos_api`'s `egl.xml` and `gl.xml` do for smithay's bindings) goes into `EMBEDDED_NOTICES` in `packaging/release.py`; the protocol XML files that `wayland-scanner` compiles come from the `wayland-*` crates named in `SCANNER_CRATES` there. Every `SPDX-License-Identifier` in those notices needs its text in the file (taken from the crate's `LICENSES/` directory) and an entry in the package licence fields; `check-metadata` fails otherwise. A new licence needs an entry in `packaging/licenses/about.toml` and in the package licence fields.

To agree, post a one-time public comment on your first pull request: `I have read and agree to the Cupertino Contributor Licence Agreement at <link to the exact CLA revision>. My name is <full name>.` A maintainer records the agreement and checks it before merging later contributions. If an entity owns the contribution, an authorised representative must accept in its name and state their name and title. If you are under eighteen, a parent or guardian must also accept and state their name and relationship. Do not post a private address or other unnecessary personal information.



## Conduct

Everyone follows the [Code of Conduct](CODE_OF_CONDUCT.md).
