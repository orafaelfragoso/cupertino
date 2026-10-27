# Instructions for agents working on Cupertino

Short rules for any agent (and any person) who edits this repository. Details live in each crate's README and in `docs/`.

## Rules

1. **Licence.** The project is `GPL-3.0-or-later`. Contributions need a signed CLA.
2. **Plain design values.** A constant is a named design value with a comment about what it controls.
3. **Shader and physics math stays as it is.** Change it only with a maintainer's agreement and a test that shows why.
4. **Layers.** A crate depends only on layers below it (`cargo run -p cupertino-layers` checks). Only `cupertino-server` and `cupertino-seat` (which implements Smithay's session trait) depend on Smithay. Model crates are pure logic, tested without a display.
5. **Names.** Crates are `cupertino-*`, files are kebab-case, user-visible terms are the project's own (overview, workspaces, launcher, control panel, file manager).
6. **Scope.** One purpose per change, no drive-by renames or reformatting, no new documents unless asked.

## Build and test

```bash
export PATH=$HOME/.cargo/bin:$PATH
CARGO_INCREMENTAL=0 cargo build --workspace
CARGO_INCREMENTAL=0 cargo nextest run --workspace --all-features   # 2-minute limit per test, see .config/nextest.toml
cargo test --workspace --all-features --doc
cargo clippy --workspace --all-targets
cargo fmt --check
cargo run -p cupertino-layers
```

- `cargo fmt` without `--all`, as CI runs it: `--all` also formats the vendored crates in `third-party/`.
- A test that runs over 2 minutes is killed and fails by name. Plain `cargo test --workspace --all-features` still works but has no per-test limit. Stop any stray test `dbus-daemon` after a killed run; never touch the real session bus.
- Run the compositor nested: `cargo run -p cupertino-server -- --winit --spawn foot`, with `XDG_DATA_DIRS="$PWD/crates/cupertino-files/data:$PWD/crates/cupertino-settings/data:${XDG_DATA_DIRS:-/usr/local/share:/usr/share}"` so the menu bar and dock find the apps' desktop entries (name, icon). Headless screenshots: `crates/cupertino-server/scripts/capture.sh out.png [server flags]` (fixture home and XDG directories, cleared environment, your asset pack kept); never capture with your real `HOME`.
- Tests run with the built-in font and never depend on a pack installed in your home directory; set `CUPERTINO_ASSET_DIR` only to try another pack.

## Worktrees and disk

- Work in your own git worktree under `~/Development/cupertino-worktrees/<name>`, on its own branch. Do not work in the main checkout. Do not push or merge unless asked.
- One cargo target directory per worktree, copied from another with `cp --reflink=always`, with `CARGO_INCREMENTAL=0`. Delete the target directory when the work is done.
- Check `df -h /home` first: do not start a build with less than 30 GB free. Never put target directories or large files in `/tmp`.

## Finishing a task

Run the build, tests, clippy, format and layer checks above for the crates you touched, state what ran and what did not, and leave the commit to the maintainer unless told to commit.
