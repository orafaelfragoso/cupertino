//! The integration tests of `cupertino-shell-agent`, one binary: each file beside this one is a module of it, so a
//! test's name starts with its file's.

mod glass;
#[path = "login-items.rs"]
mod login_items;
mod panels;
#[path = "power-key.rs"]
mod power_key;
#[path = "session-menu.rs"]
mod session_menu;
