# Contributing to Cupertino

Thanks for helping. This page covers how to build, what a good change looks like and what we cannot accept.

## Build and test

```bash
scripts/test           # the fast tier of the tests of the crates you changed, within budget (--all: every crate)
scripts/preview        # the screens your change affects, beside the installed release, in target/preview/index.html
scripts/done           # every check a change must pass
```

The commands under them, which CI runs:

```bash
cargo build --workspace
python3 ci/build-test-fixtures.py --locked -- cargo nextest run --locked --workspace --all-features --profile ci
cargo test --workspace --all-features --doc
cargo run -p cupertino-layers
cargo clippy --workspace --all-targets
cargo fmt --check
```

Notes:

- Run `cargo fmt` without `--all`, as CI does: `--all` also formats the vendored crates in `third-party/`, which stay as their authors wrote them (and one is missing the test files that its sources declare).
- Shell and app crates build their process code behind the `app` feature. Use `cargo test -p <crate> --features app` for those.
- The tests come in two tiers (`.config/nextest.toml`): the fast tier, held to 2 seconds a test and 5 minutes for the workspace, and the slow tier of tests that run whole programs, buses or displays. `scripts/test --slow` runs the slow one; the `ci` profile runs both.
- Each crate's integration tests build as one program, `tests/suite.rs`; after adding or removing a test file run `ci/test_suite.py --write`.
- Install [mold](https://github.com/rui314/mold): builds link through it when it is there (`ci/linker.sh`), several times faster than the default linker.
- Install [cargo-nextest](https://nexte.st) (`cargo install cargo-nextest --locked`; CI uses 0.9.151). It runs every test in its own process and kills a test that runs longer than 2 minutes (`.config/nextest.toml`), so a hang fails with the test's name. To use libtest, replace `cargo nextest run` with `cargo test` inside the fixture script above; libtest has no per-test limit. A killed run can leave a test `dbus-daemon` behind: stop it, never your real session bus.
- Dev builds optimise the crates where a CPU-drawn frame spends its time (`[profile.dev.package.*]` in `Cargo.toml`): the 2D rasteriser (`tiny-skia`), the font readers, shaper and glyph rasteriser, the SVG and PNG coders and their compression, and our own `cupertino-ui-render`, whose shadow and blur loops were half of a frame. Unoptimised, a 2x app-grid frame took about a second and a 2x Files capture three times as long as it does now; optimised, the frame takes about an eighth of that. The third-party crates are compiled once per target, and `cupertino-ui-render` takes about a second to compile optimised, so editing our crates rebuilds about as fast as before. Debug assertions and overflow checks stay on. Add a crate to the list only after a stack sample of a dev binary shows its time.
- Run the compositor nested while you work: `cargo run -p cupertino-server -- --winit --spawn foot`. Export `XDG_DATA_DIRS="$PWD/crates/cupertino-files/data:$PWD/crates/cupertino-settings/data:${XDG_DATA_DIRS:-/usr/local/share:/usr/share}"` first, or the menu bar and dock show `Cupertino-files` and a generic icon instead of the apps' names and icons (see [Building](docs/src/building.md)). Headless screenshots are the quickest visual check and work in CI. Take them with `crates/cupertino-server/scripts/capture.sh out.png --spawn cupertino-files`: it runs the whole desktop with a temporary sample home folder, so a screenshot never shows your own files (see the script header for more examples).
- Try the full desktop from a text console: `./run-cupertino.sh` (switch with `Ctrl+Alt+F3`). It builds the release binaries and starts a fresh session bus; the log is `~/cupertino-tty.log`, `Ctrl+Alt+Backspace` quits, and `Ctrl+Alt+F<n>` returns to your previous console. See [Building and running](docs/src/building.md#the-whole-desktop-on-a-tty).

### Container build cache

The container builds have a compile cache of their own. `packaging/ci/build-in-container.sh` and `ci/goldens.sh` mount a per-user directory, `CUPERTINO_CONTAINER_CACHE` (default `~/.cache/cupertino-build/containers`; set it empty to turn it off; it is always off on GitHub Actions unless named), into their containers and run the images' sccache on it, with Cargo's downloads beside it (`package/` capped at 12 GiB by `CUPERTINO_PACKAGE_SCCACHE_SIZE`, `goldens/` at 6 GiB by `CUPERTINO_GOLDENS_SCCACHE_SIZE`). sccache reuses an output only when the compiler, its arguments, its environment (including what a build script sets for `env!`) and every source file it read hash the same, and the paths inside the containers never change, so outputs carry over between checkouts and commits. The package is still built from the clean `git archive` source in a fresh Cargo target, and `check()` still runs: a makepkg source tree carries the commit time on every file, so a reused target could pass Cargo's timestamp check with old outputs. The goldens target stays per checkout, where file times are real. sccache cannot cache test crates, binaries and build scripts (they are linked), so `check()` keeps most of its compile time: on 16 processors a cold build took about 9 minutes of compiling and tests and a warm one about 6, with every cacheable compile a hit and `build()` down from 2 m 13 s to 32 s.

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

Every night, if `main` changed, **Nightly** runs every job, including `check` in the package build environment, and runs `packaging/ci/build-in-container.sh arch DIST --upgrade`: it builds the Arch packages (their `check()` included), packages them as two releases, upgrades an installation from the first to the second, and runs the install test on the second. The install test is the one the release build runs: in a fresh container it installs the packages and checks the units, the libraries, the `cupertino` command (version, help, `diagnose`, `desktop start --backend headless`, the manual pages, completion), the reinstall and the removal, then installs `cupertino-kit` alone, builds and runs the C samples against it, and runs `cupertino sdk info`, `check` and `new`. So a broken installed command or SDK fails the nightly before it can fail a release build. A tag `vX.Y.Z-rcN` makes a release candidate: the full CI, the Arch packages, signing and a draft prerelease. A tag `vX.Y.Z` makes the final release: the same, plus the ISO. A maintainer publishes the draft by hand. The **ISO** workflow builds an image from any branch or tag when run from the Actions tab.

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
