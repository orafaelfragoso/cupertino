//! `org.cupertino.Dialogs1` at `/org/cupertino/Dialogs1`: what the portal asks the dialogs. The component serves it
//! on its own session bus connection, which owns the name, and Kit's loop ticks it.
//!
//! - `ChooseFile(handle o, kind s, app_id s, parent_window s, title s, options a{sv}) -> (u, aay, i, a(ss))`: a file
//!   chooser (`kind` `open`, `save` or `save-files`); the answer is the response (0 chosen, 1 cancelled, 2 ended),
//!   the chosen paths, the filter shown (an index into `filters`, -1 for none) and each choice's id and value.
//! - `ChooseApplication(handle o, app_id s, parent_window s, target s, choices a(sss), last_choice s, wants_token b)
//!   -> (u, s, s)`: an app chooser; the answer is the response, the chosen application's id and the activation token
//!   made for it (empty without one).
//! - `UpdateChoices(handle o, choices a(sss))`: the applications of the app chooser of `handle` changed.
//! - `Close(handle o)`: the portal's request at `handle` was closed; its dialog goes and its call answers 2.
//!
//! The reply to a `Choose` call waits until the person answers. Only the portal may call: the program the server
//! started as `portal` (D20, `Session1.ProgramOf`, asked through `cupertino-peer`'s [`Lookup`] on a connection of
//! its own, never by process relations). Any other caller gets `org.freedesktop.DBus.Error.AccessDenied` and nothing
//! is shown.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use cupertino_peer::lookup::Lookup;
use cupertino_peer::ComponentCaller;
use zbus::fdo::{self, RequestNameFlags, RequestNameReply};
use zbus::message::Header;
use zbus::zvariant::{OwnedObjectPath, OwnedValue};
use zbus::{interface, Connection};

use crate::ask::{
    app_choices, AppAnswer, AppAsk, AppChoice, FileAnswer, FileAsk, WireApp, MAX_PENDING,
    MAX_PENDING_PER_APP,
};

/// Name, path and interface of the service.
pub const BUS_NAME: &str = "org.cupertino.Dialogs1";
pub const OBJECT_PATH: &str = "/org/cupertino/Dialogs1";
pub const INTERFACE: &str = "org.cupertino.Dialogs1";
/// The server's name for the one program that may call (`Session1.ProgramOf`).
pub const PORTAL: &str = "portal";
/// The responses of a `Choose` call.
pub const CHOSEN: u32 = 0;
pub const CANCELLED: u32 = 1;
pub const ENDED: u32 = 2;

/// What the service hands the component.
#[derive(Clone, Debug)]
pub enum Request {
    /// Show a file chooser for the portal's request at `handle`, and send how it ended to `reply`.
    File {
        handle: String,
        ask: Box<FileAsk>,
        reply: async_channel::Sender<FileAnswer>,
    },
    /// Show an app chooser for the request at `handle`.
    App {
        handle: String,
        ask: Box<AppAsk>,
        reply: async_channel::Sender<AppAnswer>,
    },
    /// The applications of the app chooser of `handle` changed.
    Update {
        handle: String,
        choices: Vec<AppChoice>,
    },
    /// Take the dialog of `handle` down without an answer.
    Close { handle: String },
}

/// Counts one open dialog of an app for as long as it lives.
struct Pending {
    counts: Arc<Mutex<HashMap<String, usize>>>,
    app_id: String,
}

impl Pending {
    fn take(counts: &Arc<Mutex<HashMap<String, usize>>>, app_id: &str) -> Option<Pending> {
        let mut open = counts.lock().unwrap_or_else(PoisonError::into_inner);
        let mine = open.get(app_id).copied().unwrap_or(0);
        if open.values().sum::<usize>() >= MAX_PENDING || mine >= MAX_PENDING_PER_APP {
            return None;
        }
        open.insert(app_id.to_owned(), mine + 1);
        Some(Pending {
            counts: Arc::clone(counts),
            app_id: app_id.to_owned(),
        })
    }
}

impl Drop for Pending {
    fn drop(&mut self) {
        let mut open = self.counts.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(n) = open.get_mut(&self.app_id) {
            *n -= 1;
            if *n == 0 {
                open.remove(&self.app_id);
            }
        }
    }
}

/// The object on the bus.
struct Dialogs1 {
    /// Hands a request to the component; false when it is not taken.
    requests: Box<dyn Fn(Request) -> bool + Send + Sync>,
    lookup: Arc<Lookup>,
    pending: Arc<Mutex<HashMap<String, usize>>>,
}

fn invalid(why: String) -> fdo::Error {
    fdo::Error::InvalidArgs(why)
}

impl Dialogs1 {
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
                "only the portal may ask the dialogs".into(),
            ))
        }
    }

    fn pending(&self, app_id: &str) -> fdo::Result<Pending> {
        Pending::take(&self.pending, app_id)
            .ok_or_else(|| fdo::Error::LimitsExceeded("too many dialogs are open".into()))
    }

    fn hand(&self, request: Request) -> fdo::Result<()> {
        if (self.requests)(request) {
            Ok(())
        } else {
            Err(fdo::Error::Failed("the dialogs are not shown".into()))
        }
    }
}

#[interface(name = "org.cupertino.Dialogs1")]
impl Dialogs1 {
    /// A file chooser: the response, the chosen paths, the filter shown (-1 for none) and the choices' values.
    #[allow(clippy::too_many_arguments)]
    async fn choose_file(
        &self,
        handle: OwnedObjectPath,
        kind: String,
        app_id: String,
        parent_window: String,
        title: String,
        options: HashMap<String, OwnedValue>,
        #[zbus(header)] header: Header<'_>,
    ) -> fdo::Result<(u32, Vec<Vec<u8>>, i32, Vec<(String, String)>)> {
        self.check(&header).await?;
        let ask =
            FileAsk::from_wire(&kind, app_id, parent_window, title, &options).map_err(invalid)?;
        let _pending = self.pending(&ask.app_id)?;
        let (reply, answer) = async_channel::bounded(1);
        self.hand(Request::File {
            handle: handle.to_string(),
            ask: Box::new(ask),
            reply,
        })?;
        Ok(match answer.recv().await {
            Ok(FileAnswer::Chosen {
                paths,
                filter,
                choices,
            }) => (
                CHOSEN,
                paths
                    .into_iter()
                    .map(|p| p.into_os_string().into_encoded_bytes())
                    .collect(),
                filter.and_then(|i| i32::try_from(i).ok()).unwrap_or(-1),
                choices,
            ),
            Ok(FileAnswer::Cancelled) => (CANCELLED, Vec::new(), -1, Vec::new()),
            // the dialog went without an answer: closed, or the component ended
            Err(_) => (ENDED, Vec::new(), -1, Vec::new()),
        })
    }

    /// An app chooser: the response, the chosen application's id and its activation token (empty for none).
    #[allow(clippy::too_many_arguments)]
    async fn choose_application(
        &self,
        handle: OwnedObjectPath,
        app_id: String,
        parent_window: String,
        target: String,
        choices: Vec<WireApp>,
        last_choice: String,
        wants_token: bool,
        #[zbus(header)] header: Header<'_>,
    ) -> fdo::Result<(u32, String, String)> {
        self.check(&header).await?;
        let ask = AppAsk::from_wire(
            app_id,
            parent_window,
            target,
            choices,
            last_choice,
            wants_token,
        )
        .map_err(invalid)?;
        let _pending = self.pending(&ask.app_id)?;
        let (reply, answer) = async_channel::bounded(1);
        self.hand(Request::App {
            handle: handle.to_string(),
            ask: Box::new(ask),
            reply,
        })?;
        Ok(match answer.recv().await {
            Ok(AppAnswer::Chosen { id, token }) => (CHOSEN, id, token.unwrap_or_default()),
            Ok(AppAnswer::Cancelled) => (CANCELLED, String::new(), String::new()),
            Err(_) => (ENDED, String::new(), String::new()),
        })
    }

    /// The applications the app chooser of `handle` offers changed.
    async fn update_choices(
        &self,
        handle: OwnedObjectPath,
        choices: Vec<WireApp>,
        #[zbus(header)] header: Header<'_>,
    ) -> fdo::Result<()> {
        self.check(&header).await?;
        let choices = app_choices(choices).map_err(invalid)?;
        self.hand(Request::Update {
            handle: handle.to_string(),
            choices,
        })
    }

    /// Take the dialog of the portal's request `handle` down without an answer.
    async fn close(
        &self,
        handle: OwnedObjectPath,
        #[zbus(header)] header: Header<'_>,
    ) -> fdo::Result<()> {
        self.check(&header).await?;
        self.hand(Request::Close {
            handle: handle.to_string(),
        })
    }
}

/// Exports the interface at [`OBJECT_PATH`] on `connection` and claims [`BUS_NAME`] on it; `lookup` asks who
/// calls, on a connection of its own, and every accepted call goes to `requests`. Fails with `zbus::Error::NameTaken`
/// when another program owns the name.
pub async fn serve(
    connection: &Connection,
    lookup: Connection,
    requests: impl Fn(Request) -> bool + Send + Sync + 'static,
) -> zbus::Result<()> {
    connection
        .object_server()
        .at(
            OBJECT_PATH,
            Dialogs1 {
                requests: Box::new(requests),
                lookup: Arc::new(Lookup::on(lookup)),
                pending: Arc::default(),
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
