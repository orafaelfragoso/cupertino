# Agent instructions

Crate details are in each crate's README; project docs in `docs/`.

## Rules

- Licence `GPL-3.0-or-later`; `cupertino-wayland-client` and its normal dependency closure are `MPL-2.0`. Contributions need a signed CLA.
- A constant is a named design value with a comment saying what it controls.
- Shader and physics math changes only with a maintainer's agreement.
- Layers: a crate depends only on the layers its row of `docs/src/architecture.md` allows, plus the further rules there (purity, facades, external crates); `cargo run -p cupertino-layers` checks them. Violations not fixed yet are listed in `crates/cupertino-layers/remaining.txt`: a new one fails, a fixed one must leave the list (`-- --write-remaining`).
- Names: crates `cupertino-*`, files kebab-case, user-visible terms the project's own (overview, workspaces, launcher, control panel, file manager).
- One purpose per change. No drive-by renames or reformatting, no new documents unless asked.
- No scripts (shell, Python): use cargo, nextest, the workspace's programs and examples. Repository automation is `cargo xtask ci` only.
- Tests live only in `crates/*/tests/` and go through the crate's public interface. No `#[test]` or `#[cfg(test)]` in `src/`. The gate enforces both.
- Find where time goes with `samply`, `perf`, `heaptrack` and `CUPERTINO_PROFILE` timelines, never timing prints.

## Git

- Use the configured author and committer. No `Co-authored-by` or assistant attribution.
- Do not push or merge unless asked.
- Check `df -h /home` before a full build (tens of GiB). Never put target directories in `/tmp`.

## Validation

One command, four levels. It plans from git (committed since the merge base, staged, unstaged, untracked), runs fmt, clippy (`--all-targets`, `CARGO_BUILD_WARNINGS=deny`), nextest and doc tests on explicit `-p` lists, and writes `target/xtask/ci-latest.json`.

```bash
cargo xtask ci --profile agent                 # L1: crates you changed
cargo xtask ci --profile integration --base B  # L2: changed crates and reverse dependencies, + layers, gate
cargo xtask ci --profile pr                    # L3: what CI runs on every push
cargo xtask ci --profile full                  # L4: whole workspace, every feature configuration
cargo xtask ci --profile P --dry-run           # the plan only
```

Rules and feature configurations are in `.config/validation.toml`. The root manifest, `Cargo.lock`, the toolchain, `.cargo/`, `.config/`, `build.rs` and any unmapped path select the whole workspace. Exit 1 is a failed check; exit 2 is a failed plan, never a pass.

### As a subagent

- Validate only the crates you changed: `cargo xtask ci --profile agent`. Never run workspace-wide validation unless the task says so; reverse dependencies are the orchestrator's job.
- Use a focused `cargo check -p CRATE --all-features` while iterating. Validate after a coherent change, not after every edit.
- Work in your own worktree and its own `target/`. Never `cargo clean`, never delete another checkout's artifacts, never change sccache or cargo config, never build on another agent's unmerged work.
- Report: crates changed, other files changed, commands run with pass/fail, checks skipped, possible downstream impact and integration risks. Passing L1 does not prove the integration.

### As the orchestrator

- After merging a batch, run `cargo xtask ci --profile integration --base <batch base>` once: the diff from the batch base is the union of every agent's changes. Never infer integration from agents' results.
- Validate again after anything that invalidates the last run; stop superseded runs.
- Widen validation (`--scope all`) for changes to public APIs used by other crates, manifests or features, build scripts, proc macros, shared protocols or schemas. Unknown impact means full validation, never a pass.
- Builds take turns through `cargo xtask ci`'s slots (`XTASK_SLOTS`, default 2). Do not run more parallel builds than that.

## Running the desktop

- Nested: `cargo run -p cupertino-server -- --winit --spawn foot` with `XDG_DATA_DIRS="$PWD/crates/cupertino-files/data:$PWD/crates/cupertino-settings/data:${XDG_DATA_DIRS:-/usr/local/share:/usr/share}"`.
- One surface in light and dark at any scale: `cargo run -p APP --example APP_capture -- SURFACE [flags]` (`crates/cupertino-capture/README.md`). Never capture with your real `HOME`.
- Release build of every program: `cargo build-programs`. Packages: `makepkg -si` in `packaging/arch/cupertino`.

## Installed-session development

- Inside the maintainer's session, build only the affected binaries with production features, replace and restart only the affected app or shell component, keep a rollback copy, record hashes, and verify the changed interaction (`docs/src/building.md#installed-session-iteration`).
- Find the target by its compositor socket, parent and start time. Never kill by name: nested sessions run the same binaries.
- Defer a restart while the component has active work or a screen share. Login, authentication, system daemons and the session manager need a planned service update. Never restart the active compositor or session during iteration.
- Desktop control is in-tree Rust over Cupertino's own protocols and Kit's semantic model; no third-party control framework or MCP SDK (`docs/src/building.md#desktop-automation-development-strategy`).

## Finishing

Run the validation for your level, state what ran and what did not, and leave the commit to the maintainer unless told to commit.
