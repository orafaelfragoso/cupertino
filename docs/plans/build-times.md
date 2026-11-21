# Plan: development, test and build times

Six changes to how the workspace builds and tests, measured on 2026-11-21 at `db3a6549d` (16 cores, 31 GiB, sccache,
rust-lld). Each task is one change with its own commit; the architecture work (shell process isolation, the
transaction protocol, readiness, the compositor's scope) waits until this plan is done.

## Baseline

| What | Time or size |
|---|---|
| One edited file in `cupertino-server` until its tests are built | 16 s (lib 11.8 s, lib test copy 5.5 s, 20 test binaries about 3 s each) |
| `--profile integration` after a 7-crate change (30 crates) | 207 s: nextest 177 s, of which build about 110 s and run 68 s |
| `--profile full` | 1,008 s |
| Test time by crate in that run | `cupertino-session-tests` 655 of 720 test-seconds; 11 restart tests 40–52 s each |
| `target/` | 104 GiB now, 639 GiB before the `target_gib` limit |
| sccache Rust hit rate | 65 % |

Every task records its own before and after with the same commands, in its commit message.

## Rules

- One task per implementer, never two in the same crate or the same file. Tasks 2, 3 and 4 touch every crate's
  manifest or `tests/`, so they run one after the other; task 1 runs beside them.
- Each implementer validates its own change (`cargo xtask ci --profile agent`); the lead runs
  `--profile integration --base <wave base>` after each wave and `--profile full` once at the end.
- No test may get weaker: the number of tests nextest lists, and their names, stay the same unless a task says why.
- No scripts; any new automation goes into `cargo xtask`.

## Wave 1 (in parallel)

### 1. Restart tests on a short clock (`cupertino-server`, `cupertino-kit-shell`, `cupertino-session-tests`)

The restart tests crash a component and wait for the real delays: `RESTART_THROTTLE` 1 s doubling to `BACKOFF_CAP`
30 s (`cupertino-server/src/restart_policy.rs`), `RESTART_QUIET_*` (`host.rs`), and the host's liveness thresholds
(`cupertino-kit-shell/src/hosting.rs`).

- [ ] Find which delays each of the 11 slow tests actually waits for (a `CUPERTINO_PROFILE` timeline of one run, not
      prints).
- [ ] A test-only scale for those delays, set by the test harness. Decision: an environment variable the server and
      host read once at start, only with the `test` feature (`cupertino-kit/test` already exists for this kind of
      thing), so a production build cannot be slowed or sped up by the environment. The policy's order of events
      stays the same; only the durations scale.
- [ ] Each of the 11 tests takes under 8 s and still checks the same restarts, places and counts.
- [ ] Done when `cupertino-session-tests` runs in under 20 s of wall time.

### 2. Incremental builds back in the main checkout (`.cargo/config.toml`, `xtask`)

`incremental = false` lets sccache share the workspace's own crates between worktrees, but every edit rebuilds a
whole crate (53,000 lines for the server). `cargo xtask ci` already sets `CARGO_INCREMENTAL=0` when it runs in CI
(`xtask/src/run.rs:183`).

- [ ] `[profile.dev] incremental = true` again; agent worktrees and CI keep it off through `CARGO_INCREMENTAL=0`.
      Decision: xtask sets it off whenever the checkout is a linked worktree (`git rev-parse --git-common-dir` differs
      from `--git-dir`) or CI, and the agent briefs do not need to know.
- [ ] `target_gib` still bounds `target/` with incremental data in it.
- [ ] Done when one edited server file reaches built tests in under 6 s in the main checkout, and an agent worktree's
      first build still hits sccache for the workspace's crates.

## Wave 2 (one after the other)

### 3. No unit-test copy of the libraries (every crate's `Cargo.toml`, `xtask` gate)

Unit tests in `src/` are forbidden, yet Cargo builds a test copy of every library (5.5 s for the server) and clippy
`--all-targets` checks it again.

- [x] `test = false` under `[lib]` (and `[[bin]]`) in every crate; doc tests keep running (`doctest` stays on).
- [x] The gate fails a crate whose `[lib]` or `[[bin]]` lacks `test = false`, so a new crate cannot bring the copy back.
- [x] Done when the server's timing report has no `lib (test)` or `bin (test)` unit and nextest lists the same tests.

### 4. One test binary per crate (`crates/*/tests/`, gate)

386 files in `tests/` are 386 binaries, each linked against the whole dependency tree.

- [x] Each crate's tests become one binary: `tests/main.rs` declares the current files as modules
      (`autotests = false` and one `[[test]]`). Files move into `tests/suite/` only where module names would clash;
      test names keep their file as their module path, so nextest filters still work.
- [x] Tests that must be their own process (they install a global, a signal handler, a panic hook, or set the
      environment) stay separate binaries, listed in the crate's manifest with a comment saying why. Nextest
      already runs each test in its own process, so most do not need this.
- [x] The gate fails a `tests/*.rs` file that is neither the crate's main test file nor listed as its own binary.
- [ ] Done when the workspace has about 80 test binaries instead of 386, nextest lists the same tests, and the
      integration run's build part drops by at least a third.

## Wave 3 (in parallel)

### 5. One feature build per run for pushes (`.config/validation.toml`, `xtask`)

The `pr` and `full` profiles build and test everything twice: once with all features and once with default
features one package at a time. Each feature set gets its own copy in `target/`, and sccache misses across them.

- [ ] The `default` configuration becomes `cargo check` only (it exists to catch a missing feature, which a build
      catches; tests add nothing there), in `pr` and `full`.
- [ ] Done when `--profile pr` builds tests once, and `full` is at least 25 % faster than 1,008 s.

### 6. Fewer crates rebuilt per change (manifests of the hub crates)

A change to `cupertino-kit`, `cupertino-ui-render` or the server reaches 30 crates; the longest chain is 11 crates.

- [ ] List each crate's reverse dependencies and the dependency edges nothing uses (a declared dependency no `use`
      reaches, a dev-dependency no test needs, a heavy dependency only one feature needs). Remove the unused ones;
      put feature-only ones behind their feature.
- [ ] No new crates and no change to `docs/src/architecture.md`'s layer table here: splitting crates belongs to the
      architecture work. `cargo run -p cupertino-layers` stays at 0 problems.
- [ ] Done when the reverse-dependency count of `cupertino-kit`, `cupertino-ui-render` and `cupertino-server` is
      recorded before and after, and any drop is measured with the same integration run.

## Close

- [ ] `--profile full` on merged `main`, with the baseline table filled in again beside the old numbers.
- [ ] `CONTRIBUTING.md` and the agent instructions say what changed for contributors (incremental in the main
      checkout, one test binary per crate, the gate's two new checks).
