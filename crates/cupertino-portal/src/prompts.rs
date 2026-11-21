//! The session's prompts (`cupertino-prompts`) as the portal's consent questions: whether an app may take a
//! screenshot, and whether it may use keyboard shortcuts while another app has the keyboard. The portal shows neither
//! itself: [`Prompts`] asks the prompts over `org.cupertino.Prompts1`, which only the portal may call, and hands the
//! answer to the call's [`screenshot::Reply`] or [`global_shortcuts::Reply`]. Each question is a task on the session
//! bus connection's executor; a call that fails answers as Cancel (a screenshot) or as dismissed (shortcuts), so nothing
//! is granted or remembered. The rows of the shortcut list are worded here ([`shortcut_rows`]), so the prompts know
//! nothing of the portal's types.

use std::collections::HashMap;

use zbus::zvariant::ObjectPath;
use zbus::Connection;

use crate::global_shortcuts::{self, Offered, Refusal};
use crate::screenshot;

/// Name, path and interface of the prompts' service.
pub const BUS_NAME: &str = "org.cupertino.Prompts1";
pub const OBJECT_PATH: &str = "/org/cupertino/Prompts1";
const INTERFACE: &str = BUS_NAME;
/// The responses of `AskScreenshot`.
const SCREENSHOT_GRANTED: u32 = 0;
/// The responses of `AskShortcuts`.
const SHORTCUTS_ALLOWED: u32 = 0;
const SHORTCUTS_DENIED: u32 = 1;
/// Characters of a shortcut's description its row shows before it ends it with an ellipsis.
const DESCRIPTION_CHARS: usize = 80;

/// The prompts, asked on the portal's session bus connection.
#[derive(Clone)]
pub struct Prompts {
    connection: Connection,
}

/// `text` as a shortcut's row shows it: at most [`DESCRIPTION_CHARS`] characters, the rest left out.
fn description(text: &str) -> String {
    match text.char_indices().nth(DESCRIPTION_CHARS) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.to_owned(),
    }
}

/// One row of the shortcut list: what the shortcut does, its keys, and why it gets no key when it does not; without
/// control or bidirectional-override characters.
fn shortcut_row(offered: &Offered, holders: &HashMap<String, String>) -> (String, String, String) {
    let keys = offered
        .trigger
        .map_or_else(|| "No key".to_owned(), |t| t.describe());
    let what = description(if offered.description.is_empty() {
        &offered.id
    } else {
        &offered.description
    });
    let why = match &offered.refusal {
        None => None,
        Some(Refusal::NoKey) => Some("it names no key".to_owned()),
        Some(Refusal::Modifier) => Some("its key is a modifier key".to_owned()),
        Some(Refusal::Typing) => Some("it needs ⌃, ⌥ or ⌘".to_owned()),
        Some(Refusal::System) => Some("the system uses it".to_owned()),
        Some(Refusal::Taken(app)) => {
            let name = holders.get(app).map_or(app.as_str(), String::as_str);
            Some(format!("“{name}” uses it"))
        }
        Some(Refusal::Twice) => Some("another shortcut here uses it".to_owned()),
    };
    let note = why.map_or_else(String::new, |why| format!("not available: {why}"));
    (plain(&what), plain(&keys), plain(&note))
}

fn plain(text: &str) -> String {
    crate::file_chooser::plain(text)
}

/// The rows of the shortcut list of `ask`, as `AskShortcuts` takes them: `(description, keys, note)`, the note
/// empty for a shortcut that gets its key.
pub fn shortcut_rows(ask: &global_shortcuts::Ask) -> Vec<(String, String, String)> {
    ask.shortcuts
        .iter()
        .map(|offered| shortcut_row(offered, &ask.holders))
        .collect()
}

/// Call `method` with `body` on the prompts.
async fn call<B, R>(connection: &Connection, method: &str, body: &B) -> Result<R, String>
where
    B: zbus::export::serde::Serialize + zbus::zvariant::DynamicType,
    R: for<'d> zbus::export::serde::Deserialize<'d> + zbus::zvariant::Type,
{
    connection
        .call_method(Some(BUS_NAME), OBJECT_PATH, Some(INTERFACE), method, body)
        .await
        .map_err(|e| format!("the prompts' {method}: {e}"))?
        .body()
        .deserialize()
        .map_err(|e| format!("the prompts' answer to {method}: {e}"))
}

/// Whether the person lets the app of `ask` take a screenshot, for the call at `handle`.
pub async fn ask_screenshot(
    connection: &Connection,
    handle: &ObjectPath<'_>,
    ask: &screenshot::Ask,
) -> Result<screenshot::Answer, String> {
    let body = (
        handle,
        ask.app_id.as_str(),
        ask.app_name.as_deref().unwrap_or(""),
        ask.app_icon.as_deref().unwrap_or(""),
        ask.parent_window.as_str(),
    );
    let response: u32 = call(connection, "AskScreenshot", &body).await?;
    Ok(match response {
        SCREENSHOT_GRANTED => screenshot::Answer::Granted,
        _ => screenshot::Answer::Cancelled,
    })
}

/// Whether the person lets the app of `ask` use its shortcuts, for the call at `handle`.
pub async fn ask_shortcuts(
    connection: &Connection,
    handle: &ObjectPath<'_>,
    ask: &global_shortcuts::Ask,
) -> Result<global_shortcuts::Answer, String> {
    let body = (
        handle,
        ask.app_id.as_str(),
        ask.app_name.as_deref().unwrap_or(""),
        ask.app_icon.as_deref().unwrap_or(""),
        ask.parent_window.as_str(),
        shortcut_rows(ask),
        u32::try_from(ask.allowed).unwrap_or(u32::MAX),
    );
    let response: u32 = call(connection, "AskShortcuts", &body).await?;
    Ok(match response {
        SHORTCUTS_ALLOWED => global_shortcuts::Answer::Allowed,
        SHORTCUTS_DENIED => global_shortcuts::Answer::Denied,
        _ => global_shortcuts::Answer::Dismissed,
    })
}

/// Take the question of the call at `handle` down; its call answers as dismissed.
pub async fn close(connection: &Connection, handle: &ObjectPath<'_>) {
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

impl Prompts {
    /// The prompts asked on `connection`, whose executor runs each question.
    pub fn new(connection: Connection) -> Prompts {
        Prompts { connection }
    }

    fn spawn(&self, task: impl std::future::Future<Output = ()> + Send + 'static) {
        self.connection.executor().spawn(task, "prompts").detach();
    }

    /// Carry out an event of the screenshot interface.
    pub fn screenshot(&self, event: screenshot::Event) {
        let connection = self.connection.clone();
        match event {
            screenshot::Event::Ask { handle, ask, reply } => self.spawn(async move {
                let answer = ask_screenshot(&connection, &handle, &ask)
                    .await
                    .unwrap_or_else(|e| {
                        eprintln!("cupertino-portal: the screenshot question for {handle}: {e}");
                        screenshot::Answer::Cancelled
                    });
                reply.send(answer);
            }),
            screenshot::Event::Close { handle } => {
                self.spawn(async move { close(&connection, &handle).await })
            }
        }
    }

    /// Carry out an event of the global shortcuts interface.
    pub fn shortcuts(&self, event: global_shortcuts::Event) {
        let connection = self.connection.clone();
        match event {
            global_shortcuts::Event::Ask { handle, ask, reply } => self.spawn(async move {
                let answer = ask_shortcuts(&connection, &handle, &ask)
                    .await
                    .unwrap_or_else(|e| {
                        eprintln!("cupertino-portal: the shortcuts question for {handle}: {e}");
                        global_shortcuts::Answer::Dismissed
                    });
                reply.send(answer);
            }),
            global_shortcuts::Event::Close { handle } => {
                self.spawn(async move { close(&connection, &handle).await })
            }
        }
    }
}
