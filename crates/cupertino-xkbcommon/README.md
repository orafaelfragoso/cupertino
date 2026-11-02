# cupertino-xkbcommon

A pure-Rust drop-in for the `xkbcommon` 0.8 crate: the package name, module layout and signatures of the C-binding crate
that Smithay and the client transport use, so the workspace's `[patch.crates-io]` entry replaces it without touching
either of them. Underneath it is the vendored `xkbcommon-rs` port of libxkbcommon (`third-party/xkbcommon-rs`), so no C
library is linked. Only the API that Smithay and `cupertino-wayland-client` (its keymaps and key state) call is provided;
`src/xkb.rs` lists the deviations.

## Local changes to the vendored port

`third-party/xkbcommon-rs` is the crates.io release `xkbcommon-rs` 0.1.2 with these changes:

| File | Change |
|---|---|
| `Cargo.toml` | The dev-dependencies (`criterion`, `evdev`, `icu_casemap`, `simplelog`, `xkbcommon`) and the `[profile.release]` section are removed. `Cargo.lock`, `Cargo.toml.orig` and `.cargo_vcs_info.json` are not vendored. |
| `src/test.rs`, `src/lib.rs` | The test module and its `mod test` declaration, and the `log_init!` test macro, are removed; they need the dropped dev-dependencies. |
| `src/xkbcomp/rules.rs` | `test_lex_rmlvo` is ignored (marked `Cupertino patch`): it reads `test/data/rules/evdev` from the upstream repository, which the crates.io package leaves out. |
| `src/xkbcomp/symbols.rs` | The two modifier-map warnings for an unknown key or keysym are logged only at verbosity 5 and above, as libxkbcommon 1.7.0 does; the port logged them at every verbosity. Both are marked `Cupertino patch`. |
| `src/keysyms_generated_phf.rs` | 63 keysym names from libxkbcommon 1.13.2 are added to `NAME_TO_KEYSYM` and `KEYSYM_TO_NAME` (2575 to 2638 entries) by `tools/cupertino_xkb_keysyms.py`, then formatted with `cargo fmt`. No existing entry changed. |

To list them again, unpack `https://static.crates.io/crates/xkbcommon-rs/xkbcommon-rs-0.1.2.crate` and run
`diff -r` against `third-party/xkbcommon-rs`.

## Licence

MIT, on purpose and unlike the rest of the workspace: this crate is a drop-in fork of the MIT-licensed `xkbcommon` crate
and keeps its licence (`LICENSE`). MIT is compatible with the workspace's GPL-3.0-or-later. The vendored `xkbcommon-rs`
keeps its own MIT/X11-style licence (`third-party/xkbcommon-rs/LICENSE`).

## Tests

`cargo test -p xkbcommon`. The oracle tests compare keymaps compiled from the machine's xkeyboard-config data with recordings of libxkbcommon 1.13 made on xkeyboard-config 2.48 (`tests/fixtures/xkb-data.txt`); the modifier order and key tables change between releases of that data, so on any other release the layout tests are skipped with a message naming both, and fail instead with `CUPERTINO_XKB_DATA_REQUIRED=1`, which CI and the gate set. Point `XKB_CONFIG_ROOT` at a copy of 2.48 to run them there; CI downloads one. `tools/cupertino_xkb_keysyms.py` regenerates the keysym names from the installed
`xkbcommon-keysyms.h`.

The vendored port is outside the workspace. Its own unit tests run with
`cargo test --lib --manifest-path third-party/xkbcommon-rs/Cargo.toml` (3 pass and `test_lex_rmlvo` is ignored; the
command writes an ignored `Cargo.lock` beside it). CI does not run them: the crates.io package ships without upstream's
test suite and data, so only three small checks remain (a keysym constant, string unescaping, the LED count), and the
oracle tests above cover the port end to end through this crate. Its doc examples are fragments of upstream's guide, not
programs, and do not compile, so `--doc` fails.
