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
- Work in a linked worktree. `ci/worktree-build.py` keeps its own writable `target/`, disables incremental compilation, requires 30 GiB free before a build and holds one of N per-user build slots until its child exits (default 4; `--slots N` or `CUPERTINO_BUILD_SLOTS`; slot 0 is `~/.cache/cupertino-build/heavy.lock`, which older copies of the script take as their single lock). Waiters are served in arrival order, and operations in one worktree run one at a time. The child gets about 1/N of the processors through `CARGO_BUILD_JOBS`, `NEXTEST_TEST_THREADS` and `RUST_TEST_THREADS` unless you set them; neither these nor N change seed compatibility. On a 16-processor machine that is four jobs per slot, at most sixteen compilers or linkers at once: building `cupertino-server`'s 79 test binaries in one slot added about 2.2 GiB in use, and the largest single process was 1.4 GiB, so four slots fit in 31 GiB with room for the desktop. Use fewer slots on a machine with less memory. `prepare --seed`, `promote`, `refresh-seed`, `cleanup --expired-seeds` and `locked` take every slot. Cargo downloads remain shared in `CARGO_HOME`. Use the wrapper for all local heavyweight builds; direct Cargo commands bypass its guard. For an outer package/container script use `python3 ci/worktree-build.py locked -- COMMAND` once; it preserves that script’s target layout. `locked --hold one` takes a single slot and `locked --hold none` only the worktree; inside a `--hold none` command a script may call `locked --hold every|one -- PHASE` from the same worktree to hold slots for one phase (the package build does). Do not nest invocations otherwise.
- `run --profile dev` prepares a cold target automatically and sets matching profiles on Cargo and `ci/build-test-fixtures.py`. Use the same profile and build environment inputs for later commands; a changed build input (see the fingerprint rule below) requires cleanup and a new preparation. Adding or updating a dependency is not one: Cargo rebuilds the crates it affects. The fixture wrapper prebuilds the requested binary/example/library fixtures once, then runs tests with a checked artifact manifest. Individual tests use the same wrapper with nextest selection arguments. Existing unmanaged targets are refused rather than adopted.
- Dev builds optimise the crates where a CPU-drawn frame spends its time (`[profile.dev.package.*]` in `Cargo.toml`): the 2D rasteriser (`tiny-skia`), the font readers, shaper and glyph rasteriser, the SVG and PNG coders and their compression, and our own `cupertino-ui-render`, whose shadow and blur loops were half of a frame. Unoptimised, a 2x app-grid frame took about a second and a 2x Files capture three times as long as it does now; optimised, the frame takes about an eighth of that. The third-party crates are compiled once per target, and `cupertino-ui-render` takes about a second to compile optimised, so editing our crates rebuilds about as fast as before. Debug assertions and overflow checks stay on. Add a crate to the list only after a stack sample of a dev binary shows its time.
- Run the compositor nested while you work: `cargo run -p cupertino-server -- --winit --spawn foot`. Export `XDG_DATA_DIRS="$PWD/crates/cupertino-files/data:$PWD/crates/cupertino-settings/data:${XDG_DATA_DIRS:-/usr/local/share:/usr/share}"` first, or the menu bar and dock show `Cupertino-files` and a generic icon instead of the apps' names and icons (see [Building](docs/src/building.md)). Headless screenshots are the quickest visual check and work in CI. Take them with `crates/cupertino-server/scripts/capture.sh out.png --spawn cupertino-files`: it runs the whole desktop with a temporary sample home folder, so a screenshot never shows your own files (see the script header for more examples).
- Try the full desktop from a text console: `./run-cupertino.sh` (switch with `Ctrl+Alt+F3`). It builds the release binaries and starts a fresh session bus; the log is `~/cupertino-tty.log`, `Ctrl+Alt+Backspace` quits, and `Ctrl+Alt+F<n>` returns to your previous console. See [Building and running](docs/src/building.md#the-whole-desktop-on-a-tty).

### Optional local cache lifecycle

Promote a successful managed build as a warm seed, then remove its writable target when the task is finished:

```bash
python3 ci/worktree-build.py promote --profile dev --name host-dev --max-seed-gib 12
python3 ci/worktree-build.py cleanup --target --expired-seeds
```

In another linked worktree, `python3 ci/worktree-build.py prepare --profile dev --seed host-dev` clones that seed using `cp --reflink=always`. A filesystem without reflinks fails visibly. Seeds live in the shared Git directory and are never build destinations.

The fingerprint rule: a target or seed is reused only when everything that can make it wrong without Cargo noticing crate by crate is unchanged, and nothing else is compared. That is the host and OS, `rustc -vV` and `cargo -vV`, the profile, the `rust-toolchain` files, every Cargo configuration file (tracked, the worktree's and its parents', and `CARGO_HOME`'s), the root manifest's `[profile.*]` tables, and the build environment: PATH/home, Cargo/Rust/rustup settings (`RUSTFLAGS`, `RUSTC_WRAPPER`, `CARGO_PROFILE_*`, …), sccache settings, native compiler/linker/build-tool variables including target-specific flags, and the project's compile-time variables, those a build script or `env!`/`option_env!` reads (`CUPERTINO_THEME_DIR`, `CUPERTINO_INSTALL_PREFIX`, `CUPERTINO_BUILD_COMMIT`, `CUPERTINO_ADMIN_GROUPS`; `ci/test-worktree-build.py` fails when the sources read one that is not listed in `PROJECT_BUILD_VARIABLES`). Manifests' dependencies, `Cargo.lock`, `[workspace.lints]` and sources are not compared: Cargo checks them after cloning and rebuilds only the crates they affect, so a branch that adds a dependency keeps its target and can still start from the seed. Variables only a test or a running program reads (`CUPERTINO_UPDATE_GOLDENS`, `CUPERTINO_REPAINT_*`, `CUPERTINO_SPEC_DIR`, `RUST_LOG`, `CARGO_TERM_*`, …), agent/session identifiers, terminal settings, tokens, SSH sockets and fixture leases are excluded. Markers record the fingerprint's schema (now 2). A target or seed made by an older wrapper (schema 1, which also hashed every manifest and `Cargo.lock`) is reused while its inputs are exactly unchanged; otherwise it is refused with a message to prepare again or refresh the seed. Host and container targets are not interchangeable; the separate `target/package-build/` subtree is excluded from host seeds and their size budget. Keep paths stable where compile-time variables embed paths; no path remapping is applied.

After each merge into an integration branch, the orchestrator refreshes its seed from the merged worktree once the merged tree builds:

```bash
python3 ci/worktree-build.py run --profile dev -- cargo test --workspace --all-features --no-run
python3 ci/worktree-build.py refresh-seed --profile dev --name parity-dev --max-seed-gib 32
```

`refresh-seed` holds every slot, copies the target under a staging name in the seeds directory, then swaps it with the seed of that name in one `renameat2` exchange and removes the old copy, so `prepare --seed` never finds a half-written seed. It replaces a seed of the same name without waiting for it to expire; a different name still counts against the limit. A staging copy left by a killed refresh is removed by the next refresh or `cleanup --expired-seeds`.

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

The container builds have a compile cache of their own. `packaging/ci/build-in-container.sh` and `ci/goldens.sh` mount a per-user directory, `CUPERTINO_CONTAINER_CACHE` (default `~/.cache/cupertino-build/containers`; set it empty to turn it off; it is always off on GitHub Actions unless named), into their containers and run the images' sccache on it, with Cargo's downloads beside it (`package/` capped at 12 GiB by `CUPERTINO_PACKAGE_SCCACHE_SIZE`, `goldens/` at 6 GiB by `CUPERTINO_GOLDENS_SCCACHE_SIZE`). sccache reuses an output only when the compiler, its arguments, its environment (including what a build script sets for `env!`) and every source file it read hash the same, and the paths inside the containers never change, so outputs carry over between worktrees and commits. The package is still built from the clean `git archive` source in a fresh Cargo target, and `check()` still runs: a makepkg source tree carries the commit time on every file, so a reused target could pass Cargo's timestamp check with old outputs. The goldens target stays per worktree, where file times are real. The package build holds its worktree for the whole run but every build slot only while makepkg compiles and tests (using every processor), one slot for the builder image, and none for the source archive, namcap, verification and the install test. sccache cannot cache test crates, binaries and build scripts (they are linked), so `check()` keeps most of its compile time: on 16 processors a cold build held every slot for about 9 minutes and a warm one for about 6, with every cacheable compile a hit and `build()` down from 2 m 13 s to 32 s.

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
- Third-party assets without a licence compatible with `GPL-3.0-or-later` (and with `MPL-2.0` in an MPL-2.0 crate). Asset packs are loaded from a directory and are not added to the tree; the one exception is the first-party icon pack in `data/assets/cupertino` (`GPL-3.0-or-later`).
- Binary blobs. Add the generator, not the output. The first-party icon pack is the exception: its SVG and PNG renditions are committed beside their generator (`data/assets/cupertino/generate.py`), because the shell loads them as they are, and running the generator must reproduce them exactly.

Maintainers (and a CI check, once it is enabled) reject files and strings that suggest otherwise; if a check flags your change by mistake, say so in the pull request.

## Pull requests

- Describe what changes and why in the first lines.
- For visible changes attach a screenshot (dark and light when both are affected).
- Keep commits tidy; we squash on merge when the history is noisy.
- A maintainer reviews every change. Expect questions about tests and about where numbers come from.

## CI and releases

A pull request runs `gate` and then what its diff needs: a docs-only change (`docs/`, top-level Markdown, the licence, the issue and pull request templates) builds the book; any other change runs fmt, deny, the lint and test build (`check`), the Weston test and the golden scenes. **CI result** is the check that must pass to merge. A push to `main` runs only the quick jobs, because its pull request ran the rest.

Arch is the only target. Every job that builds, lints or tests the code runs in an Arch container (`archlinux:base-devel`, pinned by digest in `.github/workflows/ci.yml`), as an unprivileged user; the Ubuntu runner only hosts the container. The packages those jobs install are listed once, in `ci/arch-ci.sh`. Install the same set on your machine with `sudo pacman -S --needed $(ci/arch-ci.sh list test)`.

Every night, if `main` changed, **Nightly** runs every job, including `check` in the package build environment, and builds and install-tests the Arch packages. A tag `vX.Y.Z-rcN` makes a release candidate: the full CI, the Arch packages, signing and a draft prerelease. A tag `vX.Y.Z` makes the final release: the same, plus the ISO. A maintainer publishes the draft by hand. The **ISO** workflow builds an image from any branch or tag when run from the Actions tab.

### Golden scenes and the pinned renderer

The headless golden scenes (`crates/cupertino-server/tests/goldens.rs`) are valid on one renderer only: the image of `ci/goldens/Dockerfile`, Arch pinned by digest with every package from one [Arch Linux Archive](https://archive.archlinux.org/) date, so Mesa and LLVM do not move. `ci/goldens.sh` runs them there (it needs Docker), as the `goldens` CI job does, and fails on any other renderer.

A renderer bump (a new image digest or archive date, that is a new Mesa or LLVM) changes the pixels, so the same change re-records the baselines. Run `ci/goldens.sh --update`, or the **Record goldens** workflow from the Actions tab on your branch, which uploads the new baselines as the `goldens` artifact. Review the images, then commit `crates/cupertino-server/tests/fixtures/goldens`, `renderer.txt` included. Until then the `goldens` job fails with `baseline recorded on …, running on …`.

## Issues

Include the version or commit, the backend (`--winit`, `--udev`), your compositor if nested, and the output of the failing command. For crashes include the log of the failing process (`cupertino-server` prints the shell process that exited).

## Licence and contributor licence agreement

The project is licensed under `GPL-3.0-or-later`, except the library crates that carry `license = "MPL-2.0"` in their manifest and a `LICENSE` file: a change to one of them is under the MPL-2.0. An MPL-2.0 crate may not depend on a GPL crate (`cargo run -p cupertino-layers` fails on it), so a dependency you add to one must be MPL-2.0 or a permissive licence. Before your first pull request can be merged, agree to the [contributor licence agreement](CLA.md).

The licence texts and NOTICE files of the registry crates in the shipped binaries are in `packaging/licenses/THIRD-PARTY-LICENSES`, generated with [cargo-about](https://github.com/EmbarkStudios/cargo-about) 0.9.3 (`cargo install cargo-about --version 0.9.3 --locked --features cli`). After a change to `Cargo.lock` that adds, removes or updates a linked crate, run `python3 packaging/release.py third-party` and commit the result; `python3 packaging/release.py check-metadata` fails on a stale or missing file. A crate whose licence notice sits in an input file that a build script compiles (as `khronos_api`'s `egl.xml` and `gl.xml` do for smithay's bindings) goes into `EMBEDDED_NOTICES` in `packaging/release.py`; the protocol XML files that `wayland-scanner` compiles come from the `wayland-*` crates named in `SCANNER_CRATES` there. Every `SPDX-License-Identifier` in those notices needs its text in the file (taken from the crate's `LICENSES/` directory) and an entry in the package licence fields; `check-metadata` fails otherwise. A new licence needs an entry in `packaging/licenses/about.toml` and in the package licence fields.

To agree, post a one-time public comment on your first pull request: `I have read and agree to the Cupertino Contributor Licence Agreement at <link to the exact CLA revision>. My name is <full name>.` A maintainer records the agreement and checks it before merging later contributions. If an entity owns the contribution, an authorised representative must accept in its name and state their name and title. If you are under eighteen, a parent or guardian must also accept and state their name and relationship. Do not post a private address or other unnecessary personal information.



## Conduct

Everyone follows the [Code of Conduct](CODE_OF_CONDUCT.md).
