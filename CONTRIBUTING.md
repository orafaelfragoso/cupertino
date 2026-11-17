# Contributing to Cupertino

Thanks for helping. This page covers how to build, what a good change looks like and what we cannot accept. `AGENTS.md` has the same rules in short form for coding agents.

## Build and test

`cargo xtask ci` is the one validation command. It plans from git, runs formatting, clippy and the tests of the crates your change touches, and writes `target/xtask/ci-latest.json`:

```bash
cargo xtask ci --profile agent         # the crates you changed
cargo xtask ci --profile pr            # what CI runs on every push
cargo xtask ci --profile full          # the whole workspace, every feature configuration
cargo xtask ci --profile agent --dry-run   # the plan only
```

The rules are in `.config/validation.toml`. Notes:

- The toolchain is Rust 1.100.0, pinned by `rust-toolchain.toml`. `sccache` is the compiler wrapper; the default linker is used.
- Tests live only in `crates/*/tests/` and go through the crate's public interface: no unit tests of private items, no `#[test]` or `#[cfg(test)]` in `src/`. Install [cargo-nextest](https://nexte.st) (`cargo install cargo-nextest --locked`); it runs every test in its own process and kills one that hangs.
- Shell and app crates build their process code behind the `app` feature; the validation passes it.
- Dev builds optimise the crates where a CPU-drawn frame spends its time (`[profile.dev.package.*]` in `Cargo.toml`): the 2D rasteriser, the font readers, shaper and glyph rasteriser, the SVG and PNG coders and `cupertino-ui-render`. Unoptimised, a 2x frame takes several times as long. Debug assertions and overflow checks stay on.
- Run the compositor nested while you work: `cargo run -p cupertino-server -- --winit --spawn foot`. Export `XDG_DATA_DIRS="$PWD/crates/cupertino-files/data:$PWD/crates/cupertino-settings/data:${XDG_DATA_DIRS:-/usr/local/share:/usr/share}"` first, or the menu bar and dock show `Cupertino-files` and a generic icon (see [Building](docs/src/building.md)).
- One surface in light and dark at any scale: `cargo run -p cupertino-files --example files_capture -- files/window --scale 2` (see `crates/cupertino-capture/README.md`). Never capture with your real `HOME`.
- A release build of every program is `cargo build-programs`; Arch packages are `makepkg -si` in `packaging/arch/cupertino`.
- To try the full desktop from a text console, see [Building and running](docs/src/building.md#the-whole-desktop-on-a-tty).

### Performance

Profile before adding timing code: `samply`, `perf` and `heaptrack` on the profiling build, and `CUPERTINO_PROFILE` timelines across processes. See [Profiling and performance](docs/src/building.md#profiling-and-performance).

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
2. **A test.** Model crates are pure logic and cheap to test: put the behaviour there and test it without a display, in the crate's `tests/`.
3. **Layers respected.** A crate may depend only on the layers below it. `cargo run -p cupertino-layers` fails on any other edge. Only `cupertino-server` and `cupertino-seat` (which implements Smithay's session trait) depend on Smithay.
4. **Plain design values.** A constant that shapes the look (a duration, a radius, a spring) is a named value in one place, with a comment that says what it controls. Do not change the shader or physics math without a maintainer's agreement.
5. **Names.** Crates are `cupertino-*`, files are kebab-case, and user-visible text uses the project's own terms (overview, workspaces, launcher, control panel, file manager).
6. **Small diffs.** No drive-by renames or reformatting outside the change.
7. **A benchmark for hot code.** A change to a function a frame spends its time in comes with its instruction-count benchmark in the crate's `benches/instructions.rs` (`cargo bench -p CRATE --bench instructions`; see [Building](docs/src/building.md#instruction-count-benchmarks)).

## What we cannot accept

- Code, text, images, fonts, icons, cursors or wallpapers that are not yours to license. Contribute only work that is yours or that you have the right to license under the terms of the contributor licence agreement below.
- Third-party assets without a licence compatible with `GPL-3.0-or-later` (and with `MPL-2.0` in an MPL-2.0 crate). Asset packs are loaded from a directory and are not added to the tree; the one exception is the first-party icon pack in `data/assets/cupertino` (`GPL-3.0-or-later`).
- Binary blobs. The first-party icon pack is the exception: its SVG and PNG renditions are committed because the shell loads them as they are.

Maintainers (and the gate in `cargo xtask ci`) reject files and strings that suggest otherwise; if a check flags your change by mistake, say so in the pull request.

## Pull requests

- Describe what changes and why in the first lines.
- For visible changes attach a screenshot (dark and light when both are affected).
- Keep commits tidy; we squash on merge when the history is noisy.
- A maintainer reviews every change. Expect questions about tests and about where numbers come from.
- CI runs `cargo xtask ci --profile pr`; **CI result** is the check that must pass to merge. Arch is the only target.

## Issues

Include the version or commit, the backend (`--winit`, `--udev`), your compositor if nested, and the output of the failing command. For crashes include the log of the failing process (`cupertino-server` prints the shell process that exited).

## Licence and contributor licence agreement

The project is licensed under `GPL-3.0-or-later`, except the library crates that carry `license = "MPL-2.0"` in their manifest and a `LICENSE` file: a change to one of them is under the MPL-2.0. An MPL-2.0 crate may not depend on a GPL crate (`cargo run -p cupertino-layers` fails on it), so a dependency you add to one must be MPL-2.0 or a permissive licence. Before your first pull request can be merged, agree to the [contributor licence agreement](CLA.md).

The licence texts and NOTICE files of the registry crates in the shipped binaries are in `packaging/licenses/THIRD-PARTY-LICENSES`, generated with [cargo-about](https://github.com/EmbarkStudios/cargo-about) 0.9.3 (`cargo install cargo-about --version 0.9.3 --locked --features cli`). After a change to `Cargo.lock` that adds, removes or updates a linked crate, run `python3 packaging/release.py third-party` and commit the result; `python3 packaging/release.py check-metadata` fails on a stale or missing file. A crate whose licence notice sits in an input file that a build script compiles (as `khronos_api`'s `egl.xml` and `gl.xml` do for smithay's bindings) goes into `EMBEDDED_NOTICES` in `packaging/release.py`; the protocol XML files that `wayland-scanner` compiles come from the `wayland-*` crates named in `SCANNER_CRATES` there. Every `SPDX-License-Identifier` in those notices needs its text in the file (taken from the crate's `LICENSES/` directory) and an entry in the package licence fields; `check-metadata` fails otherwise. A new licence needs an entry in `packaging/licenses/about.toml` and in the package licence fields.

To agree, post a one-time public comment on your first pull request: `I have read and agree to the Cupertino Contributor Licence Agreement at <link to the exact CLA revision>. My name is <full name>.` A maintainer records the agreement and checks it before merging later contributions. If an entity owns the contribution, an authorised representative must accept in its name and state their name and title. If you are under eighteen, a parent or guardian must also accept and state their name and relationship. Do not post a private address or other unnecessary personal information.



## Conduct

Everyone follows the [Code of Conduct](CODE_OF_CONDUCT.md).
