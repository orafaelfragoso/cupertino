//! `org.freedesktop.impl.portal.AppChooser`: `ChooseApplication` and `UpdateChoices`. A call becomes an [`Ask`] for
//! the dialog, which answers with the app the person chose through its [`Reply`]; nothing opens unless they choose
//! one. The call returns when the dialog answers, or when the frontend closes the request.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use cupertino_apps::entry::{parse_handler, Locale, Visibility};
use zbus::message::Header;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};
use zbus::{interface, Connection};

use crate::file_chooser::plain;
use crate::request::{from_frontend, Exported, PortalError, Response};

/// The interface version served.
pub const VERSION: u32 = 2;
/// At most this many choosers are open at once; another call is refused.
pub const MAX_PENDING: usize = 16;
/// At most this many of them for one app id.
pub const MAX_PENDING_PER_APP: usize = 4;
/// Bytes of an app id, a parent window, a desktop file id, a content type, a file name or a token.
pub const MAX_TEXT: usize = 1024;
/// Bytes of a URI.
pub const MAX_URI: usize = 32 * 1024;
/// Apps offered in one chooser.
pub const MAX_CHOICES: usize = 256;

/// An app the chooser offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    /// The desktop file id, without `.desktop`.
    pub id: String,
    /// The name of its desktop entry, or the id when no installed entry has one.
    pub name: String,
}

/// What the chooser shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ask {
    pub app_id: String,
    /// `wayland:<handle>`, `x11:<xid>` or empty.
    pub parent_window: String,
    /// What is opened as the chooser names it: the file name, else the URI, without control or
    /// bidirectional-override characters.
    pub target: Option<String>,
    pub choices: Vec<Choice>,
    /// The id of the choice the chooser opens with selected, when it is one of `choices`.
    pub last_choice: Option<String>,
}

/// How the chooser ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    /// The id of the chosen app.
    Chosen(String),
    Cancelled,
}

/// The chooser's answer to one [`Ask`].
pub struct Reply(pub(crate) async_channel::Sender<Answer>);

impl Reply {
    pub fn send(self, answer: Answer) {
        let _ = self.0.try_send(answer);
    }
}

/// What the dialog side is told.
pub enum Event {
    /// Show a chooser for the call at `handle`.
    Ask {
        handle: OwnedObjectPath,
        ask: Box<Ask>,
        reply: Reply,
    },
    /// The chooser of the call at `handle` offers `choices` from now on.
    Update {
        handle: OwnedObjectPath,
        choices: Vec<Choice>,
    },
    /// The frontend closed the call at `handle`: take its chooser down without answering.
    Close { handle: OwnedObjectPath },
}

/// An open call: whose it is and the ids it offers now.
struct Open {
    app_id: String,
    choices: Vec<String>,
}

type Calls = Arc<Mutex<HashMap<OwnedObjectPath, Open>>>;

fn lock(calls: &Calls) -> std::sync::MutexGuard<'_, HashMap<OwnedObjectPath, Open>> {
    calls.lock().unwrap_or_else(|e| e.into_inner())
}

/// Keeps one call in [`Calls`] for as long as it lives.
struct Pending {
    calls: Calls,
    handle: OwnedObjectPath,
}

impl Pending {
    fn take(calls: &Calls, handle: &OwnedObjectPath, open: Open) -> Result<Pending, PortalError> {
        let mut calls_now = lock(calls);
        if calls_now.contains_key(handle) {
            return Err(invalid(format!("{handle} is already in use")));
        }
        let mine = calls_now
            .values()
            .filter(|o| o.app_id == open.app_id)
            .count();
        if calls_now.len() >= MAX_PENDING || mine >= MAX_PENDING_PER_APP {
            return Err(PortalError::LimitsExceeded(
                "too many app choosers are open".to_owned(),
            ));
        }
        calls_now.insert(handle.clone(), open);
        Ok(Pending {
            calls: Arc::clone(calls),
            handle: handle.clone(),
        })
    }

    /// Whether `id` is offered now.
    fn offers(&self, id: &str) -> bool {
        lock(&self.calls)
            .get(&self.handle)
            .is_some_and(|o| o.choices.iter().any(|c| c == id))
    }
}

impl Drop for Pending {
    fn drop(&mut self) {
        lock(&self.calls).remove(&self.handle);
    }
}

/// The object on the bus.
pub struct AppChooser {
    applications: Vec<PathBuf>,
    events: Box<dyn Fn(Event) + Send + Sync>,
    calls: Calls,
}

impl AppChooser {
    /// `applications` are the directories desktop entries are read from, in precedence order; `events` reaches the
    /// dialog side.
    pub fn new(applications: Vec<PathBuf>, events: impl Fn(Event) + Send + Sync + 'static) -> Self {
        Self {
            applications,
            events: Box::new(events),
            calls: Arc::default(),
        }
    }

    /// The choices of `ids` as the chooser shows them.
    fn choices(&self, ids: &[String]) -> Vec<Choice> {
        let (locale, visibility) = (Locale::from_env(), Visibility::from_env());
        ids.iter()
            .map(|id| Choice {
                id: id.clone(),
                name: self
                    .name(id, &locale, &visibility)
                    .unwrap_or_else(|| plain(id)),
            })
            .collect()
    }

    /// The name of `<id>.desktop` in the first application directory that has it; `None` when it is hidden or not an
    /// application.
    fn name(&self, id: &str, locale: &Locale, visibility: &Visibility) -> Option<String> {
        let text = self
            .applications
            .iter()
            .find_map(|dir| std::fs::read_to_string(dir.join(format!("{id}.desktop"))).ok())?;
        let name = parse_handler(id, &text, locale, visibility)
            .ok()?
            .entry
            .name;
        Some(plain(&name)).filter(|n| !n.trim().is_empty())
    }
}

type Options = HashMap<String, OwnedValue>;

#[interface(name = "org.freedesktop.impl.portal.AppChooser")]
impl AppChooser {
    #[allow(clippy::too_many_arguments)]
    async fn choose_application(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
        handle: OwnedObjectPath,
        app_id: String,
        parent_window: String,
        choices: Vec<String>,
        options: Options,
    ) -> Result<(u32, HashMap<String, OwnedValue>), PortalError> {
        from_frontend(&header, connection).await?;
        let app_id = text("app_id", app_id)?;
        let parent_window = text("parent_window", parent_window)?;
        let choices = ids(choices)?;
        let last_choice =
            option::<String>(&options, "last_choice")?.filter(|id| choices.contains(id));
        // the chooser is a window of its own over its parent either way
        option::<bool>(&options, "modal")?;
        option::<String>(&options, "content_type")?
            .map(|t| text("content_type", t))
            .transpose()?;
        let uri = option::<String>(&options, "uri")?
            .map(|uri| bounded("uri", uri, MAX_URI))
            .transpose()?;
        let filename = option::<String>(&options, "filename")?
            .map(|name| text("filename", name))
            .transpose()?;
        let token = option::<String>(&options, "activation_token")?
            .map(|token| text("activation_token", token))
            .transpose()?;

        let pending = Pending::take(
            &self.calls,
            &handle,
            Open {
                app_id: app_id.clone(),
                choices: choices.clone(),
            },
        )?;
        let request = Exported::at(connection, handle.as_ref()).await?;
        let (reply, answer) = async_channel::bounded(1);
        (self.events)(Event::Ask {
            handle: handle.clone(),
            ask: Box::new(Ask {
                app_id,
                parent_window,
                target: filename.or(uri).map(|t| plain(&t)),
                choices: self.choices(&choices),
                last_choice,
            }),
            reply: Reply(reply),
        });
        let answer = futures_lite::future::or(async { Some(answer.recv().await.ok()) }, async {
            request.closed().await;
            None
        })
        .await;
        request.remove().await;
        Ok(match answer {
            Some(Some(Answer::Chosen(choice))) if pending.offers(&choice) => {
                (Response::Success as u32, results(choice, token)?)
            }
            Some(Some(Answer::Cancelled)) => (Response::Cancelled as u32, HashMap::new()),
            Some(_) => (Response::Other as u32, HashMap::new()),
            None => {
                (self.events)(Event::Close { handle });
                (Response::Other as u32, HashMap::new())
            }
        })
    }

    async fn update_choices(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
        handle: OwnedObjectPath,
        choices: Vec<String>,
    ) -> Result<(), PortalError> {
        from_frontend(&header, connection).await?;
        let choices = ids(choices)?;
        match lock(&self.calls).get_mut(&handle) {
            Some(open) => open.choices = choices.clone(),
            None => return Err(invalid(format!("{handle} is no open app chooser"))),
        }
        (self.events)(Event::Update {
            handle,
            choices: self.choices(&choices),
        });
        Ok(())
    }

    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        VERSION
    }
}

fn invalid(why: impl Into<String>) -> PortalError {
    PortalError::InvalidArgs(why.into())
}

fn bounded(what: &str, value: String, max: usize) -> Result<String, PortalError> {
    if value.len() > max {
        return Err(invalid(format!("{what} is longer than {max} bytes")));
    }
    Ok(value)
}

fn text(what: &str, value: String) -> Result<String, PortalError> {
    bounded(what, value, MAX_TEXT)
}

/// The desktop file ids of `choices`, each one a file name in an application directory.
fn ids(choices: Vec<String>) -> Result<Vec<String>, PortalError> {
    if choices.len() > MAX_CHOICES {
        return Err(invalid(format!("more than {MAX_CHOICES} choices")));
    }
    choices
        .into_iter()
        .map(|id| {
            let id = text("a choice", id)?;
            if id.is_empty() || id.contains('/') || id.chars().any(char::is_control) {
                return Err(invalid(format!("{id:?} is not a desktop file id")));
            }
            Ok(id)
        })
        .collect()
}

fn option<T>(options: &Options, key: &str) -> Result<Option<T>, PortalError>
where
    T: TryFrom<OwnedValue>,
{
    options
        .get(key)
        .map(|value| {
            value
                .try_clone()
                .ok()
                .and_then(|value| T::try_from(value).ok())
                .ok_or_else(|| invalid(format!("option {key} has the wrong type")))
        })
        .transpose()
}

fn owned(value: Value<'_>) -> Result<OwnedValue, PortalError> {
    OwnedValue::try_from(value).map_err(|e| PortalError::ZBus(e.into()))
}

/// The `results` of a chosen app. The dialog cannot ask the compositor for a token of its own, so the caller's token,
/// when it sent one, is handed on to the chosen app.
fn results(
    choice: String,
    token: Option<String>,
) -> Result<HashMap<String, OwnedValue>, PortalError> {
    let mut results = HashMap::from([("choice".to_owned(), owned(Value::from(choice))?)]);
    if let Some(token) = token {
        results.insert("activation_token".to_owned(), owned(Value::from(token))?);
    }
    Ok(results)
}
