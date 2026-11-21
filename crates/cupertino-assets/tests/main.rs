//! The integration tests of `cupertino-assets`, one binary: each file beside this one is a module of it, so a
//! test's name starts with its file's.

mod common;

#[path = "builtin-font.rs"]
mod builtin_font;
mod packs;
#[path = "search-path.rs"]
mod search_path;
mod theme;
