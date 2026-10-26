# Contributing to Cupertino

Thanks for helping. This page covers how to build, what a good change looks like and what we cannot accept.

## Build and test

```bash
cargo build --workspace
cargo test --workspace
cargo run -p cupertino-layers      # dependency rules between layers
cargo clippy --workspace --all-targets
cargo fmt --all -- --check
```

Notes:

- Shell and app crates build their process code behind the `app` feature. Use `cargo test -p <crate> --features app` for those.
- If a full `cargo test --workspace` hangs on a D-Bus test, test crate by crate and stop any stray test `dbus-daemon` (never your real session bus).
- Build artifacts are large. Keep several GB free and do not point `CARGO_TARGET_DIR` at a small or shared disk.
- Run the compositor nested while you work: `cargo run -p cupertino-server -- --winit --spawn foot`. Export `XDG_DATA_DIRS="$PWD/crates/cupertino-files/data:$PWD/crates/cupertino-settings/data:${XDG_DATA_DIRS:-/usr/local/share:/usr/share}"` first, or the menu bar and dock show `Cupertino-files` and a generic icon instead of the apps' names and icons (see [Building](docs/src/building.md)). Headless screenshots are the quickest visual check and work in CI. Take them with `crates/cupertino-server/scripts/capture.sh out.png --spawn cupertino-files`: it runs the whole desktop with a temporary sample home folder, so a screenshot never shows your own files (see the script header for more examples).

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
4. **Plain design values.** A constant that shapes the look (a duration, a radius, a spring) is a named value in one place, with a comment that says what it controls. Do not write tests that compare pixels with another system's output, and do not change the shader or physics math without a maintainer's agreement.
5. **Names.** Crates are `cupertino-*`, files are kebab-case, and user-visible text uses the project's own terms (overview, workspaces, launcher, control panel, file manager).
6. **Small diffs.** No drive-by renames or reformatting outside the change.

## What we cannot accept

- Code, text, images, fonts, icons, cursors or wallpapers copied or extracted from proprietary software or its assets, or derived from its binaries. Contribute only work that is yours or that you have the right to license under the terms of the contributor licence agreement below.
- Third-party assets without a licence compatible with `GPL-3.0-or-later`. Asset packs are loaded from a directory and are not added to the tree; the default cursor theme is a separate package fetched at build time.
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

The project is licensed under `GPL-3.0-or-later`. Before we can merge your first pull request you sign the contributor licence agreement (CLA) once. A bot (CLA Assistant) comments on the pull request with a link; sign there. The CLA text is at `<PLACEHOLDER: link to the CLA text, to be set by the maintainer before publication>`. A pull request without a signed CLA is not merged.

## Conduct

Everyone follows the [Code of Conduct](CODE_OF_CONDUCT.md).
