//! A `com.canonical.AppMenu.Registrar` that registers nothing. Qt, KDE Frameworks, Chromium and LibreOffice export
//! their menus and hide their in-window menu bar only while the name has an owner; the bar reads those menus through
//! the window's Wayland announcement, so the registrations themselves are never needed.

use zbus::zvariant::{ObjectPath, OwnedObjectPath};
use zbus::Connection;

pub const REGISTRAR_NAME: &str = "com.canonical.AppMenu.Registrar";
pub const REGISTRAR_PATH: &str = "/com/canonical/AppMenu/Registrar";

struct Registrar;

#[zbus::interface(name = "com.canonical.AppMenu.Registrar")]
impl Registrar {
    fn register_window(&self, _window_id: u32, _menu_object_path: ObjectPath<'_>) {}

    fn unregister_window(&self, _window_id: u32) {}

    /// No window is ever known: an empty service and the root path.
    fn get_menu_for_window(&self, _window_id: u32) -> (String, OwnedObjectPath) {
        (
            String::new(),
            OwnedObjectPath::from(ObjectPath::from_static_str_unchecked("/")),
        )
    }
}

/// Serve the registrar on `conn` and claim its name; `NameTaken` when another process owns it.
pub async fn serve(conn: &Connection) -> zbus::Result<()> {
    conn.object_server().at(REGISTRAR_PATH, Registrar).await?;
    super::claim_name(conn, REGISTRAR_NAME).await
}
