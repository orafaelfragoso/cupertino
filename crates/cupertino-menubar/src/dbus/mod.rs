//! D-Bus layer (feature `dbus`): menu sources and status items over open freedesktop / KDE / GTK /
//! Canonical interfaces.
//!
//! * [`dbusmenu`]: client for `com.canonical.dbusmenu` (layout parsing, events, update signals).
//! * [`sni`]: `org.kde.StatusNotifierWatcher` service plus `org.kde.StatusNotifierItem` client.
//! * [`gtk`]: reader for `org.gtk.Menus` / `org.gtk.Actions` (GMenuModel).
//! * [`registrar`]: `com.canonical.AppMenu.Registrar`, owned so toolkits hide their in-window menu bar.
//!
//! All futures are runtime-agnostic (zbus is built with its own `async-io` executor); use any
//! executor or [`zbus::block_on`] to drive them.

pub mod dbusmenu;
pub mod gtk;
pub mod registrar;
pub mod sni;

pub use zbus;

use std::collections::HashMap;

use zbus::zvariant::{OwnedValue, Value};

/// Strip any number of nested variant layers.
pub(crate) fn unwrap_variant<'a, 'b>(mut v: &'b Value<'a>) -> &'b Value<'a> {
    while let Value::Value(inner) = v {
        v = inner;
    }
    v
}

pub(crate) fn prop<'a>(props: &'a HashMap<String, OwnedValue>, key: &str) -> Option<&'a Value<'a>> {
    props.get(key).map(|v| unwrap_variant(v))
}

pub(crate) fn prop_str(props: &HashMap<String, OwnedValue>, key: &str) -> Option<String> {
    match prop(props, key)? {
        Value::Str(s) => Some(s.as_str().to_string()),
        Value::ObjectPath(p) => Some(p.as_str().to_string()),
        _ => None,
    }
}

pub(crate) fn prop_bool(props: &HashMap<String, OwnedValue>, key: &str) -> Option<bool> {
    match prop(props, key)? {
        Value::Bool(b) => Some(*b),
        _ => None,
    }
}

pub(crate) fn prop_i32(props: &HashMap<String, OwnedValue>, key: &str) -> Option<i32> {
    match prop(props, key)? {
        Value::I32(i) => Some(*i),
        Value::U32(i) => i32::try_from(*i).ok(),
        Value::I64(i) => i32::try_from(*i).ok(),
        Value::U8(i) => Some(i32::from(*i)),
        Value::Bool(b) => Some(i32::from(*b)),
        _ => None,
    }
}

/// Claim a well-known name without replacing or queueing behind another owner.
pub(crate) async fn claim_name(conn: &zbus::Connection, name: &'static str) -> zbus::Result<()> {
    use zbus::fdo::{RequestNameFlags, RequestNameReply};
    match conn
        .request_name_with_flags(name, RequestNameFlags::DoNotQueue.into())
        .await?
    {
        RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner => Ok(()),
        RequestNameReply::Exists | RequestNameReply::InQueue => Err(zbus::Error::NameTaken),
    }
}
