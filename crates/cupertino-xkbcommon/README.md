# cupertino-xkbcommon

A pure-Rust drop-in for the `xkbcommon` 0.8 crate: the package name, module layout and signatures of the C-binding crate
that Smithay and the client toolkit use, so the workspace's `[patch.crates-io]` entry replaces it without touching either
of them. Underneath it is the vendored `xkbcommon-rs` port of libxkbcommon (`third-party/xkbcommon-rs`), so no C library
is linked. Only the API that Smithay and `cupertino-client` call is provided; `src/xkb.rs` lists the deviations.

## Licence

MIT, on purpose and unlike the rest of the workspace: this crate is a drop-in fork of the MIT-licensed `xkbcommon` crate
and keeps its licence (`LICENSE`). MIT is compatible with the workspace's GPL-3.0-or-later. The vendored `xkbcommon-rs`
keeps its own MIT/X11-style licence (`third-party/xkbcommon-rs/LICENSE`).

## Tests

`cargo test -p xkbcommon`. The oracle tests compare keymaps compiled from the machine's xkeyboard-config data with recordings of libxkbcommon 1.13 made on xkeyboard-config 2.48 (`tests/fixtures/xkb-data.txt`); the modifier order and key tables change between releases of that data, so on any other release the tests fail with a message naming both. Point `XKB_CONFIG_ROOT` at a copy of 2.48 to run them there; CI downloads one. `tools/cupertino_xkb_keysyms.py` regenerates the keysym names from the installed
`xkbcommon-keysyms.h`.
