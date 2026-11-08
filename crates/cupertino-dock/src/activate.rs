//! Asking a running application for a window over D-Bus: `org.freedesktop.Application.Activate` on the bus name of
//! its desktop file ID, as the Desktop Entry specification's `DBusActivatable` defines it, with the activation token in
//! the platform data so the window it opens may take the focus.

use std::collections::HashMap;

use zbus::zvariant::{ObjectPath, Value};

/// The interface a D-Bus activatable application implements.
pub const INTERFACE: &str = "org.freedesktop.Application";

/// The object path of the application with desktop file ID `id` (without `.desktop`): a slash, then the ID with its
/// dots as slashes and its dashes as underscores.
pub fn object_path(id: &str) -> String {
    format!("/{}", id.replace('.', "/").replace('-', "_"))
}

/// The platform data of an activation: the token as `activation-token` and as `desktop-startup-id`.
pub fn platform_data(token: Option<&str>) -> HashMap<&'static str, Value<'_>> {
    let mut data = HashMap::new();
    if let Some(token) = token {
        data.insert("activation-token", Value::from(token));
        data.insert("desktop-startup-id", Value::from(token));
    }
    data
}

/// Call `Activate` on the application with desktop file ID `id` over `conn`.
pub async fn activate(conn: &zbus::Connection, id: &str, token: Option<&str>) -> zbus::Result<()> {
    let path = object_path(id);
    let path = ObjectPath::try_from(path.as_str())?;
    conn.call_method(
        Some(id),
        path,
        Some(INTERFACE),
        "Activate",
        &(platform_data(token),),
    )
    .await
    .map(|_| ())
}

/// [`activate`] on the session bus, on a thread of its own so the dock's loop does not wait for the application: the
/// future ends in whether the call succeeded. A failure is reported on standard error, prefixed with `process`.
pub fn activate_in_thread(
    process: &'static str,
    id: String,
    token: Option<String>,
) -> impl std::future::Future<Output = bool> + Send + 'static {
    let (done, result) = async_channel::bounded(1);
    let spawned = std::thread::Builder::new()
        .name("dbus-activate".into())
        .spawn(move || {
            let outcome = async_io::block_on(async {
                let conn = zbus::Connection::session().await?;
                activate(&conn, &id, token.as_deref()).await
            });
            if let Err(e) = &outcome {
                eprintln!("{process}: cannot ask {id} for a window: {e}");
            }
            let _ = done.send_blocking(outcome.is_ok());
        });
    if let Err(e) = spawned {
        eprintln!("{process}: cannot start the activation thread: {e}");
    }
    async move { result.recv().await.unwrap_or(false) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_object_path_follows_the_desktop_file_id() {
        assert_eq!(object_path("org.example.Maps"), "/org/example/Maps");
        assert_eq!(object_path("org.example.my-app"), "/org/example/my_app");
    }

    #[test]
    fn the_token_goes_in_the_platform_data_twice() {
        let data = platform_data(Some("t-1"));
        assert_eq!(data.get("activation-token"), Some(&Value::from("t-1")));
        assert_eq!(data.get("desktop-startup-id"), Some(&Value::from("t-1")));
        assert!(platform_data(None).is_empty());
    }
}
