//! `org.cupertino.Prompts1` at `/org/cupertino/Prompts1`: what the portal asks the prompts. The program serves it on
//! its main session bus connection, which owns the name, and Kit's loop ticks it.
//!
//! - `AskScreenshot(handle o, app_id s, app_name s, app_icon s, parent_window s) -> u`: 0 when the person lets the
//!   app take a screenshot, 1 when they do not.
//! - `AskShortcuts(handle o, app_id s, app_name s, app_icon s, parent_window s, shortcuts a(sss), allowed u) -> u`:
//!   each shortcut is `(description, keys, note)` as the portal words it; 0 Allow, 1 Don't Allow, 2 dismissed.
//! - `Close(handle o)`: the portal's request at `handle` was closed; its question goes and its call answers as
//!   dismissed.
//!
//! The reply to an `Ask` call waits until the person answers. An empty `app_name` or `app_icon` names none;
//! `parent_window` is taken and bounded, but an alert on the prompt layer has no parent.
//!
//! Only the portal may call: the program the server started as `portal` (D20, `Session1.ProgramOf`, asked through
//! `cupertino-peer`'s [`Lookup`] on a connection of its own, never by process relations). Any other caller gets
//! `org.freedesktop.DBus.Error.AccessDenied` and nothing is shown.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use cupertino_peer::lookup::Lookup;
use cupertino_peer::ComponentCaller;
use zbus::fdo::{self, RequestNameFlags, RequestNameReply};
use zbus::message::Header;
use zbus::zvariant::OwnedObjectPath;
use zbus::{interface, Connection};

use crate::consent::{App, Question, Request, Row, SCREENSHOT_CANCELLED, SHORTCUTS_DISMISSED};
use crate::text::shown;

/// Name, path and interface of the service.
pub const BUS_NAME: &str = "org.cupertino.Prompts1";
pub const OBJECT_PATH: &str = "/org/cupertino/Prompts1";
pub const INTERFACE: &str = "org.cupertino.Prompts1";
/// The server's name for the one program that may call (`Session1.ProgramOf`).
pub const PORTAL: &str = "portal";
/// The most questions that may wait at once, the one on screen included; more are refused at once, so a flood of
/// calls can't stack alerts. As many as the portal lets screenshots wait.
pub const MAX_PENDING: usize = 16;
/// Bytes of a handle, an app id, name or icon, a parent window and each text of a shortcut.
pub const MAX_TEXT: usize = 1024;
/// Shortcuts of one `AskShortcuts` call, as many as the portal takes in one call.
pub const MAX_SHORTCUTS: usize = 64;

/// The object on the bus.
struct Prompts1 {
    /// Hands a request to the program; false when it is not taken.
    events: Box<dyn Fn(Request) -> bool + Send + Sync>,
    lookup: Arc<Lookup>,
    pending: Arc<AtomicUsize>,
}

/// Counts one waiting question for as long as it lives.
struct Pending(Arc<AtomicUsize>);

impl Pending {
    fn take(count: &Arc<AtomicUsize>) -> Option<Pending> {
        count
            .try_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < MAX_PENDING).then_some(n + 1)
            })
            .ok()
            .map(|_| Pending(Arc::clone(count)))
    }
}

impl Drop for Pending {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

fn text(what: &str, value: &str) -> fdo::Result<String> {
    if value.len() > MAX_TEXT {
        return Err(fdo::Error::InvalidArgs(format!(
            "{what} is longer than {MAX_TEXT} bytes"
        )));
    }
    Ok(shown(value))
}

/// An icon name: one with no `/` and no control character.
fn icon(value: &str) -> fdo::Result<Option<String>> {
    if value.len() > MAX_TEXT {
        return Err(fdo::Error::InvalidArgs(format!(
            "app_icon is longer than {MAX_TEXT} bytes"
        )));
    }
    let name = !value.is_empty() && !value.contains('/') && !value.chars().any(char::is_control);
    Ok(name.then(|| value.to_owned()))
}

fn app(app_id: &str, app_name: &str, app_icon: &str, parent_window: &str) -> fdo::Result<App> {
    text("parent_window", parent_window)?;
    let name = text("app_name", app_name)?;
    Ok(App {
        id: text("app_id", app_id)?,
        name: (!name.trim().is_empty()).then_some(name),
        icon: icon(app_icon)?,
    })
}

impl Prompts1 {
    /// Refuses every caller but the portal.
    async fn check(&self, header: &Header<'_>) -> fdo::Result<()> {
        let portal = match header.sender() {
            Some(sender) => {
                self.lookup.component_caller(sender).await
                    == ComponentCaller::Program(PORTAL.into())
            }
            None => false,
        };
        if portal {
            Ok(())
        } else {
            Err(fdo::Error::AccessDenied(
                "only the portal may ask the prompts".into(),
            ))
        }
    }

    /// Hands `question` to the program and waits for the person's answer; `dismissed` when the program drops it.
    async fn ask(
        &self,
        handle: OwnedObjectPath,
        question: Question,
        dismissed: u32,
    ) -> fdo::Result<u32> {
        let Some(_pending) = Pending::take(&self.pending) else {
            return Err(fdo::Error::LimitsExceeded(
                "too many questions are waiting".into(),
            ));
        };
        let (reply, answer) = async_channel::bounded(1);
        let request = Request::Ask {
            handle: handle.to_string(),
            question,
            reply,
        };
        if !(self.events)(request) {
            return Err(fdo::Error::Failed("the prompts are not shown".into()));
        }
        Ok(answer.recv().await.unwrap_or(dismissed))
    }
}

#[interface(name = "org.cupertino.Prompts1")]
impl Prompts1 {
    /// Whether the app may take a screenshot of the screen: 0 granted, 1 cancelled.
    async fn ask_screenshot(
        &self,
        handle: OwnedObjectPath,
        app_id: String,
        app_name: String,
        app_icon: String,
        parent_window: String,
        #[zbus(header)] header: Header<'_>,
    ) -> fdo::Result<u32> {
        self.check(&header).await?;
        let app = app(&app_id, &app_name, &app_icon, &parent_window)?;
        self.ask(handle, Question::Screenshot { app }, SCREENSHOT_CANCELLED)
            .await
    }

    /// Whether the app may use `shortcuts` while another app has the keyboard: 0 Allow, 1 Don't Allow, 2 dismissed.
    #[allow(clippy::too_many_arguments)]
    async fn ask_shortcuts(
        &self,
        handle: OwnedObjectPath,
        app_id: String,
        app_name: String,
        app_icon: String,
        parent_window: String,
        shortcuts: Vec<(String, String, String)>,
        allowed: u32,
        #[zbus(header)] header: Header<'_>,
    ) -> fdo::Result<u32> {
        self.check(&header).await?;
        if shortcuts.len() > MAX_SHORTCUTS {
            return Err(fdo::Error::InvalidArgs(format!(
                "more than {MAX_SHORTCUTS} shortcuts"
            )));
        }
        let app = app(&app_id, &app_name, &app_icon, &parent_window)?;
        let rows = shortcuts
            .iter()
            .map(|(description, keys, note)| {
                Ok(Row {
                    description: text("a description", description)?,
                    keys: text("the keys", keys)?,
                    note: text("a note", note)?,
                })
            })
            .collect::<fdo::Result<Vec<Row>>>()?;
        let question = Question::Shortcuts { app, rows, allowed };
        self.ask(handle, question, SHORTCUTS_DISMISSED).await
    }

    /// Take the question of the portal's request `handle` down without an answer.
    async fn close(
        &self,
        handle: OwnedObjectPath,
        #[zbus(header)] header: Header<'_>,
    ) -> fdo::Result<()> {
        self.check(&header).await?;
        (self.events)(Request::Close {
            handle: handle.to_string(),
        });
        Ok(())
    }
}

/// Exports the interface at [`OBJECT_PATH`] on `connection` and claims [`BUS_NAME`] on it; `lookup` asks who
/// calls, on a connection of its own, and every accepted call goes to `events`. Fails with `zbus::Error::NameTaken`
/// when another program owns the name.
pub async fn serve(
    connection: &Connection,
    lookup: Connection,
    events: impl Fn(Request) -> bool + Send + Sync + 'static,
) -> zbus::Result<()> {
    connection
        .object_server()
        .at(
            OBJECT_PATH,
            Prompts1 {
                events: Box::new(events),
                lookup: Arc::new(Lookup::on(lookup)),
                pending: Arc::new(AtomicUsize::new(0)),
            },
        )
        .await?;
    // after the object exists, so no call finds the name without it; never queue behind the owner
    match connection
        .request_name_with_flags(BUS_NAME, RequestNameFlags::DoNotQueue.into())
        .await?
    {
        RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner => Ok(()),
        RequestNameReply::Exists | RequestNameReply::InQueue => Err(zbus::Error::NameTaken),
    }
}
