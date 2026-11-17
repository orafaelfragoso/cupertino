//! Asking a running application for a window over D-Bus: `org.freedesktop.Application.Activate` on the bus name of
//! its desktop file ID, as the Desktop Entry specification's `DBusActivatable` defines it, with the activation token in
//! the platform data so the window it opens may take the focus. The dock and the launcher ask a resident application for
//! a window this way (feature `dbus`).

use std::collections::HashMap;
use std::time::Duration;

use zbus::zvariant::{ObjectPath, Value};

/// The interface a D-Bus activatable application implements.
pub const INTERFACE: &str = "org.freedesktop.Application";

/// How long an application has to answer `Activate`: one that has not answered by then counts as failed, so the caller
/// starts its entry again instead of waiting on it.
pub const ACTIVATE_TIMEOUT: Duration = Duration::from_secs(3);

/// `builder` with its method calls bounded by `timeout` ([`ACTIVATE_TIMEOUT`] for the caller's own connection): a hung
/// application fails the call instead of holding it open.
pub fn bounded(
    builder: zbus::connection::Builder<'_>,
    timeout: Duration,
) -> zbus::connection::Builder<'_> {
    builder.method_timeout(timeout)
}

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

/// How long [`activate_on_session`] should take: [`ACTIVATE_TIMEOUT`] for the answer and a second for connecting to the
/// session bus. The deadline its callers give the task that runs it.
pub const ACTIVATE_DEADLINE: Duration = Duration::from_secs(4);

/// [`activate`] on a connection of its own to the session bus, bounded by [`ACTIVATE_TIMEOUT`], waiting for the answer:
/// whether the call succeeded. A failure is reported on standard error, prefixed with `process`. It blocks; a program's
/// loop runs it as a task.
pub fn activate_on_session(process: &str, id: &str, token: Option<&str>) -> bool {
    let outcome = async_io::block_on(async {
        let conn = bounded(zbus::connection::Builder::session()?, ACTIVATE_TIMEOUT)
            .build()
            .await?;
        activate(&conn, id, token).await
    });
    if let Err(e) = &outcome {
        eprintln!("{process}: cannot ask {id} for a window: {e}");
    }
    outcome.is_ok()
}
