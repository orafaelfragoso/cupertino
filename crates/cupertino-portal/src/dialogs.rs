//! The shell's dialogs (`cupertino-shell-dialogs`) as the portal's file chooser and app chooser. The portal shows
//! neither itself: [`Dialogs`] asks the dialogs component over `org.cupertino.Dialogs1`, which only the portal may call,
//! and hands the answer to the call's [`file_chooser::Reply`] or [`app_chooser::Reply`]. Each dialog is a task on the
//! session bus connection's executor; a call that fails, or a dialog that ends without an answer (the component
//! restarted), answers as ended, so the app's request ends with nothing chosen.

use std::collections::HashMap;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;

use zbus::zvariant::{ObjectPath, Value};
use zbus::Connection;

use crate::app_chooser;
use crate::file_chooser::{self, Kind, Pattern, WireChoice, WireFilter};

/// Name, path and interface of the dialogs' service.
pub const BUS_NAME: &str = "org.cupertino.Dialogs1";
pub const OBJECT_PATH: &str = "/org/cupertino/Dialogs1";
const INTERFACE: &str = BUS_NAME;
/// The responses of `ChooseFile` and `ChooseApplication`: chosen and cancelled; any other ended the dialog without
/// an answer.
const CHOSEN: u32 = 0;
const CANCELLED: u32 = 1;

/// `ChooseFile`'s reply: the response, the chosen paths, the filter shown (an index, -1 for none) and the choices.
pub type FileReply = (u32, Vec<Vec<u8>>, i32, Vec<(String, String)>);

/// The dialogs, asked on the portal's session bus connection.
#[derive(Clone)]
pub struct Dialogs {
    connection: Connection,
}

/// How a dialog the portal asked for ended, before it is the portal's answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ended<T> {
    Answered(T),
    /// The dialog went without an answer.
    Gone,
}

/// The name `ChooseFile` gives a kind of file chooser.
pub fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::Open => "open",
        Kind::Save => "save",
        Kind::SaveFiles => "save-files",
    }
}

fn bytes(path: &std::path::Path) -> Vec<u8> {
    path.as_os_str().as_bytes().to_vec()
}

/// The options of `ChooseFile` for `ask`: its checked values as the dialogs take them, the filter shown first as an
/// index into its filters (-1 for one that is not among them).
pub fn file_options(ask: &file_chooser::Ask) -> HashMap<&'static str, Value<'static>> {
    let filters: Vec<WireFilter> = (ask.filters.iter())
        .map(|f| {
            let patterns = (f.patterns.iter())
                .map(|p| match p {
                    Pattern::Glob(glob) => (0, glob.clone()),
                    Pattern::Mime(mime) => (1, mime.clone()),
                })
                .collect();
            (f.name.clone(), patterns)
        })
        .collect();
    let current = (ask.current_filter.as_ref())
        .and_then(|current| ask.filters.iter().position(|f| f == current))
        .and_then(|i| i32::try_from(i).ok())
        .unwrap_or(-1);
    let choices: Vec<WireChoice> = (ask.choices.iter())
        .map(|c| {
            (
                c.id.clone(),
                c.label.clone(),
                c.options.clone(),
                c.initial.clone(),
            )
        })
        .collect();
    let mut options = HashMap::from([
        ("modal", Value::from(ask.modal)),
        ("multiple", Value::from(ask.multiple)),
        ("directory", Value::from(ask.directory)),
        ("filters", Value::from(filters)),
        ("current_filter", Value::from(current)),
        ("choices", Value::from(choices)),
        (
            "files",
            Value::from(ask.files.iter().map(|f| bytes(f)).collect::<Vec<_>>()),
        ),
    ]);
    if let Some(label) = &ask.accept_label {
        options.insert("accept_label", Value::from(label.clone()));
    }
    if let Some(name) = &ask.current_name {
        options.insert("current_name", Value::from(name.clone()));
    }
    if let Some(folder) = &ask.current_folder {
        options.insert("current_folder", Value::from(bytes(folder)));
    }
    if let Some(file) = &ask.current_file {
        options.insert("current_file", Value::from(bytes(file)));
    }
    options
}

/// The portal's answer to a file chooser: `ChooseFile`'s reply, the filter it names taken back from `ask`.
pub fn file_answer(
    ask: &file_chooser::Ask,
    (response, paths, filter, choices): FileReply,
) -> Ended<file_chooser::Answer> {
    match response {
        CHOSEN => Ended::Answered(file_chooser::Answer::Chosen {
            paths: paths
                .into_iter()
                .map(|p| PathBuf::from(std::ffi::OsString::from_vec(p)))
                .collect(),
            filter: usize::try_from(filter)
                .ok()
                .and_then(|i| ask.filters.get(i))
                .cloned(),
            choices,
        }),
        CANCELLED => Ended::Answered(file_chooser::Answer::Cancelled),
        _ => Ended::Gone,
    }
}

/// The applications of an app chooser as `ChooseApplication` and `UpdateChoices` take them: `(id, name, icon)`.
pub fn app_choices(choices: &[app_chooser::Choice]) -> Vec<(String, String, String)> {
    (choices.iter())
        .map(|c| {
            (
                c.id.clone(),
                c.name.clone(),
                c.icon.clone().unwrap_or_default(),
            )
        })
        .collect()
}

/// The portal's answer to an app chooser: `ChooseApplication`'s reply.
pub fn app_answer((response, choice, token): (u32, String, String)) -> Ended<app_chooser::Answer> {
    match response {
        CHOSEN => Ended::Answered(app_chooser::Answer::Chosen {
            id: choice,
            token: (!token.is_empty()).then_some(token),
        }),
        CANCELLED => Ended::Answered(app_chooser::Answer::Cancelled),
        _ => Ended::Gone,
    }
}

/// Call `method` with `body` on the dialogs.
async fn call<B, R>(connection: &Connection, method: &str, body: &B) -> Result<R, String>
where
    B: zbus::export::serde::Serialize + zbus::zvariant::DynamicType,
    R: for<'d> zbus::export::serde::Deserialize<'d> + zbus::zvariant::Type,
{
    connection
        .call_method(Some(BUS_NAME), OBJECT_PATH, Some(INTERFACE), method, body)
        .await
        .map_err(|e| format!("the dialogs' {method}: {e}"))?
        .body()
        .deserialize()
        .map_err(|e| format!("the dialogs' answer to {method}: {e}"))
}

/// The file chooser `ask` asks for, for the call at `handle`.
pub async fn choose_file(
    connection: &Connection,
    handle: &ObjectPath<'_>,
    ask: &file_chooser::Ask,
) -> Result<Ended<file_chooser::Answer>, String> {
    let body = (
        handle,
        kind_name(ask.kind),
        ask.app_id.as_str(),
        ask.parent_window.as_str(),
        ask.title.as_str(),
        file_options(ask),
    );
    let reply = call(connection, "ChooseFile", &body).await?;
    Ok(file_answer(ask, reply))
}

/// The app chooser `ask` asks for, for the call at `handle`.
pub async fn choose_application(
    connection: &Connection,
    handle: &ObjectPath<'_>,
    ask: &app_chooser::Ask,
) -> Result<Ended<app_chooser::Answer>, String> {
    let body = (
        handle,
        ask.app_id.as_str(),
        ask.parent_window.as_str(),
        ask.target.as_deref().unwrap_or(""),
        app_choices(&ask.choices),
        ask.last_choice.as_deref().unwrap_or(""),
        ask.wants_token,
    );
    let reply = call(connection, "ChooseApplication", &body).await?;
    Ok(app_answer(reply))
}

/// Take the dialog of the call at `handle` down; its call answers as ended.
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

impl Dialogs {
    /// The dialogs asked on `connection`, whose executor runs each dialog.
    pub fn new(connection: Connection) -> Dialogs {
        Dialogs { connection }
    }

    fn spawn(&self, task: impl std::future::Future<Output = ()> + Send + 'static) {
        self.connection.executor().spawn(task, "dialogs").detach();
    }

    /// Carry out an event of the file chooser interface. A dialog that went without an answer drops its reply, which
    /// ends the call as the interface's `Other`.
    pub fn file(&self, event: file_chooser::Event) {
        let connection = self.connection.clone();
        match event {
            file_chooser::Event::Ask { handle, ask, reply } => self.spawn(async move {
                match choose_file(&connection, &handle, &ask).await {
                    Ok(Ended::Answered(answer)) => reply.send(answer),
                    Ok(Ended::Gone) => {}
                    Err(e) => eprintln!("cupertino-portal: the file chooser for {handle}: {e}"),
                }
            }),
            file_chooser::Event::Close { handle } => {
                self.spawn(async move { close(&connection, &handle).await })
            }
        }
    }

    /// Carry out an event of the app chooser interface.
    pub fn app(&self, event: app_chooser::Event) {
        let connection = self.connection.clone();
        match event {
            app_chooser::Event::Ask { handle, ask, reply } => self.spawn(async move {
                match choose_application(&connection, &handle, &ask).await {
                    Ok(Ended::Answered(answer)) => reply.send(answer),
                    Ok(Ended::Gone) => {}
                    Err(e) => eprintln!("cupertino-portal: the app chooser for {handle}: {e}"),
                }
            }),
            app_chooser::Event::Update { handle, choices } => self.spawn(async move {
                let body = (&handle, app_choices(&choices));
                if let Err(e) = call::<_, ()>(&connection, "UpdateChoices", &body).await {
                    eprintln!("cupertino-portal: the app chooser for {handle}: {e}");
                }
            }),
            app_chooser::Event::Close { handle } => {
                self.spawn(async move { close(&connection, &handle).await })
            }
        }
    }
}
