//! The integration tests of `cupertino-menubar`, one binary: each file beside this one is a module of it, so a
//! test's name starts with its file's.

mod bus;
#[path = "dbusmenu-exporter.rs"]
mod dbusmenu_exporter;
#[path = "dbusmenu-properties.rs"]
mod dbusmenu_properties;
mod glass;
#[path = "gtk-menus.rs"]
mod gtk_menus;
#[path = "late-menu.rs"]
mod late_menu;
#[path = "menu-events.rs"]
mod menu_events;
#[path = "menu-rows.rs"]
mod menu_rows;
mod menus;
mod registrar;
#[path = "renumbered-menu.rs"]
mod renumbered_menu;
#[path = "status-items.rs"]
mod status_items;
