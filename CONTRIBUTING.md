# Contributing to Cupertino

Thanks for helping. This page covers how to build, what a good change looks like and what we cannot accept.

## Build and test

```bash
cargo build --workspace
cargo nextest run --workspace --all-features
cargo test --workspace --all-features --doc
cargo run -p cupertino-layers      # dependency rules between layers
cargo clippy --workspace --all-targets
cargo fmt --check
```

Notes:

- Run `cargo fmt` without `--all`, as CI does: `--all` also formats the vendored crates in `third-party/`, which stay as their authors wrote them (and one is missing the test files that its sources declare).
- Shell and app crates build their process code behind the `app` feature. Use `cargo test -p <crate> --features app` for those.
- Install [cargo-nextest](https://nexte.st) (`cargo install cargo-nextest --locked`; CI uses 0.9.149). It runs every test in its own process and kills a test that runs longer than 2 minutes (`.config/nextest.toml`), so a hang fails with the test's name. `cargo test` has no time limit. A killed run can leave a test `dbus-daemon` behind: stop it, never your real session bus. Plain `cargo test --workspace --all-features` still works but has no per-test limit.
- Build artifacts are large. Keep several GB free and do not point `CARGO_TARGET_DIR` at a small or shared disk.
- Run the compositor nested while you work: `cargo run -p cupertino-server -- --winit --spawn foot`. Export `XDG_DATA_DIRS="$PWD/crates/cupertino-files/data:$PWD/crates/cupertino-settings/data:${XDG_DATA_DIRS:-/usr/local/share:/usr/share}"` first, or the menu bar and dock show `Cupertino-files` and a generic icon instead of the apps' names and icons (see [Building](docs/src/building.md)). Headless screenshots are the quickest visual check and work in CI. Take them with `crates/cupertino-server/scripts/capture.sh out.png --spawn cupertino-files`: it runs the whole desktop with a temporary sample home folder, so a screenshot never shows your own files (see the script header for more examples).
- Try the full desktop from a text console: `./run-cupertino.sh` (switch with `Ctrl+Alt+F3`). It builds the release binaries and starts a fresh session bus; the log is `~/cupertino-tty.log`, `Ctrl+Alt+Backspace` quits, and `Ctrl+Alt+F<n>` returns to your previous console. See [Building and running](docs/src/building.md#the-whole-desktop-on-a-tty).

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

## Issues

Include the version or commit, the backend (`--winit`, `--udev`), your compositor if nested, and the output of the failing command. For crashes include the log of the failing process (`cupertino-server` prints the shell process that exited).

## Licence and contributor licence agreement

The project is licensed under `GPL-3.0-or-later`, except the library crates that carry `license = "MPL-2.0"` in their manifest and a `LICENSE` file: a change to one of them is under the MPL-2.0. An MPL-2.0 crate may not depend on a GPL crate (`cargo run -p cupertino-layers` fails on it), so a dependency you add to one must be MPL-2.0 or a permissive licence. Before your first pull request can be merged, agree to the [contributor licence agreement](CLA.md).

To agree, post a one-time public comment on your first pull request: `I have read and agree to the Cupertino Contributor Licence Agreement at <link to the exact CLA revision>. My name is <full name>.` A maintainer records the agreement and checks it before merging later contributions. If an entity owns the contribution, an authorised representative must accept in its name and state their name and title. If you are under eighteen, a parent or guardian must also accept and state their name and relationship. Do not post a private address or other unnecessary personal information.



## Conduct

Everyone follows the [Code of Conduct](CODE_OF_CONDUCT.md).
