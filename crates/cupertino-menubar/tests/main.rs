//! The integration tests of `cupertino-menubar`, one binary: each file beside this one is a module of it, so a
//! test's name starts with its file's.

mod bus;
mod glass;
#[path = "gtk-menus.rs"]
mod gtk_menus;
#[path = "late-menu.rs"]
mod late_menu;
mod menus;
mod registrar;
#[path = "renumbered-menu.rs"]
mod renumbered_menu;
#[path = "status-items.rs"]
mod status_items;
