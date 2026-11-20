//! The screenshot program as the portal's colour picker and chooser of what a screen share shows: the server starts
//! `cupertino-screenshot --serve` when asked (`org.cupertino.Session1.StartScreenshot`, which only the portal may call),
//! and the program serves `org.cupertino.Screenshot1`, which only the portal may call in turn. The portal shows neither
//! itself: [`pick_color`] and [`select_sources`] start the program unless it runs, wait for it to own its name, and ask
//! it; [`close`] takes a call's window down when the frontend closes the request.

use std::time::Duration;

use cupertino_peer::component::{SESSION_BUS_NAME, SESSION_INTERFACE, SESSION_OBJECT_PATH};
use futures_lite::StreamExt;
use zbus::fdo::DBusProxy;
use zbus::names::BusName;
use zbus::Connection;

use crate::screen_cast::{Ask, Source};

/// Name, path and interface of the program's service.
pub const BUS_NAME: &str = "org.cupertino.Screenshot1";
pub const OBJECT_PATH: &str = "/org/cupertino/Screenshot1";
const INTERFACE: &str = BUS_NAME;
/// How long the program has to own its name once the server started it; a start that takes longer fails the call.
pub const START_TIMEOUT: Duration = Duration::from_secs(10);
/// The responses of the program's answers.
const SUCCESS: u32 = 0;
const CANCELLED: u32 = 1;

/// How the program answered.
#[derive(Clone, Debug, PartialEq)]
pub enum Answer<T> {
    Done(T),
    Cancelled,
    /// It could not show its window or pick, and said so.
    Failed,
}

fn error(what: &str, e: impl std::fmt::Display) -> String {
    format!("{what}: {e}")
}

/// Start the program unless it owns its name, and wait until it does, at most [`START_TIMEOUT`].
async fn ready(connection: &Connection) -> Result<(), String> {
    let name = BusName::try_from(BUS_NAME).map_err(|e| error("the program's name", e))?;
    let dbus = DBusProxy::new(connection)
        .await
        .map_err(|e| error("the bus", e))?;
    // followed before the name is asked for, so an owner that comes between is not missed
    let mut owners = dbus
        .receive_name_owner_changed_with_args(&[(0, BUS_NAME)])
        .await
        .map_err(|e| error("the bus", e))?;
    if dbus
        .name_has_owner(name)
        .await
        .map_err(|e| error("the bus", e))?
    {
        return Ok(());
    }
    connection
        .call_method(
            Some(SESSION_BUS_NAME),
            SESSION_OBJECT_PATH,
            Some(SESSION_INTERFACE),
            "StartScreenshot",
            &(),
        )
        .await
        .map_err(|e| error("the server did not start the screenshot program", e))?;
    let owned = async {
        while let Some(change) = owners.next().await {
            if change.args().is_ok_and(|a| a.new_owner().is_some()) {
                return true;
            }
        }
        false
    };
    let late = async {
        async_io::Timer::after(START_TIMEOUT).await;
        false
    };
    if futures_lite::future::or(owned, late).await {
        Ok(())
    } else {
        Err(format!(
            "the screenshot program did not serve {BUS_NAME} within {START_TIMEOUT:?}"
        ))
    }
}

/// Whether the call failed because nobody owned the name: the program had exited, idle, just before.
fn gone(e: &zbus::Error) -> bool {
    matches!(e, zbus::Error::MethodError(name, ..)
        if matches!(name.as_str(), "org.freedesktop.DBus.Error.ServiceUnknown" | "org.freedesktop.DBus.Error.NameHasNoOwner"))
}

/// Call `method` with `body` on the program, started first unless it runs; once more when it had just exited.
async fn call<B, R>(connection: &Connection, method: &str, body: &B) -> Result<R, String>
where
    B: zbus::export::serde::Serialize + zbus::zvariant::DynamicType,
    R: for<'d> zbus::export::serde::Deserialize<'d> + zbus::zvariant::Type,
{
    let mut again = true;
    loop {
        ready(connection).await?;
        let reply = connection
            .call_method(Some(BUS_NAME), OBJECT_PATH, Some(INTERFACE), method, body)
            .await;
        match reply {
            Ok(message) => {
                return message
                    .body()
                    .deserialize()
                    .map_err(|e| error("the program's answer", e))
            }
            Err(e) if again && gone(&e) => again = false,
            Err(e) => return Err(error(&format!("the screenshot program's {method}"), e)),
        }
    }
}

fn answer<T>(response: u32, value: T) -> Answer<T> {
    match response {
        SUCCESS => Answer::Done(value),
        CANCELLED => Answer::Cancelled,
        _ => Answer::Failed,
    }
}

/// The colour of a pixel the person clicks on any display, for the call at `handle`: red, green and blue in sRGB from
/// 0 to 1.
pub async fn pick_color(connection: &Connection, handle: &str) -> Result<Answer<[f64; 3]>, String> {
    let (response, (r, g, b)): (u32, (f64, f64, f64)) =
        call(connection, "PickColor", &(handle,)).await?;
    Ok(answer(response, [r, g, b]))
}

/// The source the person chooses among `ask`'s, by its index, and whether they let the app share it again without
/// asking, for the call at `handle`.
pub async fn select_sources(
    connection: &Connection,
    handle: &str,
    ask: &Ask,
) -> Result<Answer<(usize, bool)>, String> {
    let sources: Vec<(&str, &str, &str)> = ask
        .sources
        .iter()
        .map(|source| match source {
            Source::Display {
                name, description, ..
            } => ("display", name.as_str(), description.as_str()),
            Source::Window { title, app_id, .. } => ("window", title.as_str(), app_id.as_str()),
        })
        .collect();
    let body = (
        handle,
        ask.app_name.as_deref().unwrap_or(""),
        ask.app_icon.as_deref().unwrap_or(""),
        ask.parent_window.as_str(),
        ask.offer_remember,
        sources,
    );
    let (response, index, remember): (u32, u32, bool) =
        call(connection, "SelectSources", &body).await?;
    Ok(match answer(response, (index as usize, remember)) {
        // an index past the list chooses nothing
        Answer::Done((index, _)) if index >= ask.sources.len() => Answer::Failed,
        answer => answer,
    })
}

/// Take down what shows the call at `handle`, when the program runs; it answers that call no more.
pub async fn close(connection: &Connection, handle: &str) {
    let _ = connection
        .call_method(
            Some(BUS_NAME),
            OBJECT_PATH,
            Some(INTERFACE),
            "Close",
            &(handle,),
        )
        .await;
}
