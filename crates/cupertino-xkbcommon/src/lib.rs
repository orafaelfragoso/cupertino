//! A pure-Rust drop-in for the `xkbcommon` 0.8 crate.
//!
//! The crate has the package name, module layout and signatures of the C-binding crate that Smithay and the client
//! toolkit use, so a `[patch.crates-io]` entry replaces it without touching either of them. Underneath it is the
//! `xkbcommon-rs` port of libxkbcommon 1.7 (rules, keycodes, types, compat and symbols compiled from the XKB data
//! files, the state machine and the keymap serialiser), so no C library is linked.
//!
//! Only the API that Smithay 0.7 and `cupertino-client` call is provided, see [`xkb`] for the deviations.

pub mod xkb;
