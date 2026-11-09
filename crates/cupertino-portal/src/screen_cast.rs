//! `org.freedesktop.impl.portal.ScreenCast`: `CreateSession`, `SelectSources` and `Start`, and the
//! `org.freedesktop.impl.portal.Session` object of each session. `Start` becomes an [`Ask`] for the dialog, which answers
//! with the source the person chose through its [`Reply`]; the capture side then shares that source as a PipeWire
//! stream ([`Capture::Start`]) until the session is closed ([`Capture::Stop`]) or the share ends on its own, which closes
//! the session. While a share runs, a menu-bar item ([`crate::sharing`]) names who shares what and stops it.
//!
//! A share the person let the app restore ([`Persist`]) comes back without the chooser when the frontend hands its
//! `restore_data` to a later `SelectSources`: a display from `screen-sharing.conf` (the person forgets it in Settings),
//! a window or a display shared until the app quits from memory.

use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use cupertino_defaults::screen_sharing::{is_id, Grant, ScreenSharingConf};
use cupertino_diagnostic_events::{self as diag, Severity, Value as Field};

use futures_lite::StreamExt;
use zbus::message::Header;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, StructureBuilder, Value};
use zbus::{interface, Connection};

use crate::consent::identify;
use crate::file_chooser::plain;
use crate::request::{answered, from_frontend, Exported, PortalError, Response, FRONTEND};
use crate::sharing::{Indicator, Shown};
use crate::PORTAL_PATH;

/// The interface version served.
pub const VERSION: u32 = 5;
/// At most this many sessions exist at once; another is refused.
pub const MAX_SESSIONS: usize = 16;
/// At most this many of them for one app id.
pub const MAX_SESSIONS_PER_APP: usize = 4;
/// Bytes of an app id, a parent window and a shown name.
pub const MAX_TEXT: usize = 1024;
/// The vendor of the `restore_data` this backend hands the frontend to keep.
pub const RESTORE_VENDOR: &str = "cupertino";
/// The version of that data: a grant id.
pub const RESTORE_VERSION: u32 = 1;
/// At most this many shares are kept in memory to restore until their app quits; the oldest goes first.
pub const MAX_TRANSIENT: usize = 64;

/// How long a share may be restored without asking, as `persist_mode` names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Persist {
    /// Not at all: the chooser shows each time.
    None = 0,
    /// While the app runs; kept in memory.
    Transient = 1,
    /// Until the person forgets it in Settings; only a display, kept in `screen-sharing.conf`.
    Persistent = 2,
}

impl Persist {
    fn from_u32(value: u32) -> Option<Persist> {
        match value {
            0 => Some(Persist::None),
            1 => Some(Persist::Transient),
            2 => Some(Persist::Persistent),
            _ => None,
        }
    }
}

/// What a session may share, as the `types` bitmask names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceTypes(u32);

impl SourceTypes {
    pub const MONITOR: SourceTypes = SourceTypes(1);
    pub const WINDOW: SourceTypes = SourceTypes(2);
    /// Every type this backend shares.
    pub const AVAILABLE: SourceTypes = SourceTypes(1 | 2);

    pub fn bits(self) -> u32 {
        self.0
    }

    pub fn contains(self, other: SourceTypes) -> bool {
        self.0 & other.0 == other.0
    }
}

/// How the pointer shows in a stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorMode {
    Hidden = 1,
    Embedded = 2,
}

/// Every cursor mode served, as `AvailableCursorModes` names them.
pub const AVAILABLE_CURSOR_MODES: u32 = CursorMode::Hidden as u32 | CursorMode::Embedded as u32;

/// What a stream shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// An output, by the registry name of its `wl_output` global.
    Display {
        output: u32,
        /// The connector name, such as `DP-1`.
        name: String,
        description: String,
    },
    /// A window, by its foreign-toplevel identifier.
    Window {
        identifier: String,
        title: String,
        app_id: String,
    },
}

impl Source {
    /// The source with its names as the chooser shows them: other clients name windows, so they lose control and
    /// bidirectional-override characters.
    fn shown(self) -> Source {
        match self {
            Source::Display {
                output,
                name,
                description,
            } => Source::Display {
                output,
                name: plain(&name),
                description: plain(&description),
            },
            Source::Window {
                identifier,
                title,
                app_id,
            } => Source::Window {
                identifier,
                title: plain(&title),
                app_id: plain(&app_id),
            },
        }
    }

    pub fn source_type(&self) -> SourceTypes {
        match self {
            Source::Display { .. } => SourceTypes::MONITOR,
            Source::Window { .. } => SourceTypes::WINDOW,
        }
    }

    /// The source as the menu-bar item names it: the display's description or connector, the window's title or app.
    pub fn label(&self) -> String {
        let label = match self {
            Source::Display {
                name, description, ..
            } => {
                if description.is_empty() {
                    name
                } else {
                    description
                }
            }
            Source::Window { title, app_id, .. } => {
                if title.is_empty() {
                    app_id
                } else {
                    title
                }
            }
        };
        plain(label)
    }

    /// Whether `self`, as remembered, is `other` as listed now: a display by its connector, a window by its identifier.
    fn same(&self, other: &Source) -> bool {
        match (self, other) {
            (Source::Display { name: a, .. }, Source::Display { name: b, .. }) => a == b,
            (Source::Window { identifier: a, .. }, Source::Window { identifier: b, .. }) => a == b,
            _ => false,
        }
    }
}

/// What the chooser shows: the app that asks and the sources of the requested types, as they were when `Start` was
/// called.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ask {
    pub app_id: String,
    /// The name of the app's desktop entry, else its id; `None` when the id is empty.
    pub app_name: Option<String>,
    /// The icon name of the app's desktop entry, if it has one.
    pub app_icon: Option<String>,
    /// `wayland:<handle>`, `x11:<xid>` or empty.
    pub parent_window: String,
    pub sources: Vec<Source>,
    /// The app asked to restore the share later, so the chooser offers to remember it (off until ticked).
    pub offer_remember: bool,
}

/// How the chooser ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    /// Share `source`; `remember` when the person ticked to let the app restore it.
    Chosen {
        source: Source,
        remember: bool,
    },
    Cancelled,
}

/// The chooser's answer to one [`Ask`].
pub struct Reply(pub(crate) async_channel::Sender<Answer>);

impl Reply {
    pub fn send(self, answer: Answer) {
        let _ = self.0.try_send(answer);
    }
}

/// A shared source as the frontend reads it from `Start`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stream {
    /// The PipeWire node.
    pub node: u32,
    pub source_type: SourceTypes,
    /// The source's size in the compositor's space.
    pub size: (i32, i32),
    /// Where a display sits in the compositor's space; `None` for a window.
    pub position: Option<(i32, i32)>,
}

/// Held by the capture side for as long as the share runs; dropping it closes the session.
pub struct Running(#[allow(dead_code)] async_channel::Sender<()>);

/// What the capture side answers to [`Capture::Start`].
pub struct Started(pub(crate) async_channel::Sender<Result<Stream, String>>);

impl Started {
    pub fn send(self, started: Result<Stream, String>) {
        let _ = self.0.try_send(started);
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
    /// The frontend closed the call at `handle`, or its session: take its chooser down without answering.
    Close { handle: OwnedObjectPath },
}

/// What the capture side is told.
pub enum Capture {
    /// Share `source` for the session at `session`, while `running` is held.
    Start {
        session: OwnedObjectPath,
        source: Source,
        cursor: CursorMode,
        started: Started,
        running: Running,
    },
    /// The session at `session` was closed: stop its share.
    Stop { session: OwnedObjectPath },
}

/// Where a session is.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Phase {
    Created,
    Selected {
        types: SourceTypes,
        cursor: CursorMode,
        /// What the app asked for.
        persist: Persist,
        /// The grant id of the frontend's `restore_data`, when it is ours.
        restore: Option<String>,
    },
    /// The chooser is open, or the share is being set up.
    Starting,
    Started,
}

struct Entry {
    app_id: String,
    phase: Phase,
    closed: async_channel::Receiver<()>,
    /// The menu-bar item while the share runs.
    indicator: Option<Indicator>,
    /// The transient grant the share runs under, forgotten when the person stops it.
    grant: Option<String>,
}

/// A share kept in memory to restore until the app quits.
struct Transient {
    id: String,
    app_id: String,
    source: Source,
}

/// The shares an app may restore: displays in `screen-sharing.conf`, and shares kept in memory.
struct Grants {
    file: Option<PathBuf>,
    transient: Mutex<Vec<Transient>>,
}

impl Grants {
    /// The source grant `id` of `app_id` restores among `sources`, and for how long; `None` when it is not known, was
    /// forgotten, or its source is not there now.
    fn restore(&self, id: &str, app_id: &str, sources: &[Source]) -> Option<(Source, Persist)> {
        if app_id.is_empty() {
            return None;
        }
        let kept = self
            .transient
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .find(|t| t.id == id && t.app_id == app_id)
            .map(|t| t.source.clone());
        if let Some(kept) = kept {
            let now = sources.iter().find(|s| kept.same(s))?;
            return Some((now.clone(), Persist::Transient));
        }
        let path = self.file.as_ref()?;
        let (conf, _) = ScreenSharingConf::load(path)
            .inspect_err(|e| {
                eprintln!("cupertino-portal: {} restores nothing: {e}", path.display());
                store_failed("load", app_id, &e.to_string());
            })
            .ok()?;
        let grant = conf.find(id, app_id)?;
        let now = sources
            .iter()
            .find(|s| matches!(s, Source::Display { name, .. } if *name == grant.display))?;
        Some((now.clone(), Persist::Persistent))
    }

    /// Remember that `app_id` may restore `source` for at most `wanted`: a window only while the app runs, a display
    /// until forgotten when the file takes it. The grant id and how long it lasts; `None` when nothing is kept.
    fn give(&self, app_id: &str, source: &Source, wanted: Persist) -> Option<(String, Persist)> {
        if app_id.is_empty() || wanted == Persist::None {
            return None;
        }
        let id = grant_id()?;
        if let (Persist::Persistent, Source::Display { name, .. }, Some(path)) =
            (wanted, source, &self.file)
        {
            let grant = Grant {
                id: id.clone(),
                app_id: app_id.to_owned(),
                display: name.clone(),
            };
            // under the file's lock, so a Forget in Settings at the same time is neither lost nor undone
            let stored = ScreenSharingConf::update(path, |conf| {
                grant
                    .check()
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
                conf.grants.push(grant);
                Ok(())
            });
            match stored {
                Ok(()) => return Some((id, Persist::Persistent)),
                Err(e) => {
                    eprintln!(
                        "cupertino-portal: the share is kept only while the app runs: {}: {e}",
                        path.display()
                    );
                    store_failed("save", app_id, &e.to_string());
                }
            }
        }
        let mut transient = self.transient.lock().unwrap_or_else(|e| e.into_inner());
        if transient.len() >= MAX_TRANSIENT {
            transient.remove(0);
        }
        transient.push(Transient {
            id: id.clone(),
            app_id: app_id.to_owned(),
            source: source.clone(),
        });
        Some((id, Persist::Transient))
    }

    /// Forget the share kept in memory as `id`.
    fn forget(&self, id: &str) {
        self.transient
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|t| t.id != id);
    }
}

/// Records `portal.permission.store_failed`: the remembered shares of `screen-sharing.conf` could not be read
/// (`load`) or written (`save`) for `app_id`.
fn store_failed(operation: &'static str, app_id: &str, error: &str) {
    diag::event(
        Severity::Warn,
        "portal.permission.store_failed",
        "the remembered screen shares could not be read or written",
    )
    .field("operation", Field::Public(operation))
    .field("app_id", Field::Public(app_id))
    .field("error", Field::Public(error))
    .emit();
}

/// Records `portal.screencast.failed`: the share of `app_id` (when known) failed at `stage`, `start` before its stream
/// reached the app or `running` after.
pub fn screencast_failed(app_id: Option<&str>, stage: &'static str, error: &str) {
    let mut event = diag::event(
        Severity::Error,
        "portal.screencast.failed",
        "a screen share failed",
    )
    .field("stage", Field::Public(stage));
    if let Some(app_id) = app_id {
        event = event.field("app_id", Field::Public(app_id));
    }
    event.field("error", Field::Public(error)).emit();
}

/// A new grant id: 128 random bits in hexadecimal; `None` when the system has no randomness to give.
fn grant_id() -> Option<String> {
    let mut bytes = [0u8; 16];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut bytes))
        .ok()?;
    let id: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    is_id(&id).then_some(id)
}

/// The grant id in the frontend's `restore_data`, when the data is this backend's.
fn restore_id(value: &Value<'_>) -> Option<String> {
    let Value::Structure(data) = value else {
        return None;
    };
    let [Value::Str(vendor), Value::U32(version), Value::Value(inner)] = data.fields() else {
        return None;
    };
    let Value::Str(id) = inner.as_ref() else {
        return None;
    };
    (vendor.as_str() == RESTORE_VENDOR && *version == RESTORE_VERSION && is_id(id.as_str()))
        .then(|| id.as_str().to_owned())
}

/// The `restore_data` of grant `id`, for the frontend to keep and hand back.
fn restore_data(id: &str) -> Result<OwnedValue, PortalError> {
    let data = StructureBuilder::new()
        .add_field(RESTORE_VENDOR)
        .add_field(RESTORE_VERSION)
        .append_field(Value::Value(Box::new(Value::from(id))))
        .build()
        .map_err(|e| PortalError::ZBus(e.into()))?;
    owned(Value::Structure(data))
}

type Sessions = Arc<Mutex<HashMap<OwnedObjectPath, Entry>>>;
type Shares = Arc<dyn Fn(Capture) + Send + Sync>;

fn lock(sessions: &Sessions) -> std::sync::MutexGuard<'_, HashMap<OwnedObjectPath, Entry>> {
    sessions.lock().unwrap_or_else(|e| e.into_inner())
}

/// The `org.freedesktop.impl.portal.Session` object of one session, at its handle while the session lives.
struct Session {
    path: OwnedObjectPath,
    closed: async_channel::Sender<()>,
    sessions: Sessions,
    shares: Shares,
}

#[interface(name = "org.freedesktop.impl.portal.Session")]
impl Session {
    async fn close(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> Result<(), PortalError> {
        let result = async {
            from_frontend(&header, connection).await?;
            // a Start still waiting for the chooser or the share returns now
            let _ = self.closed.try_send(());
            let (connection, sessions, shares, path) = (
                connection.clone(),
                Arc::clone(&self.sessions),
                Arc::clone(&self.shares),
                self.path.clone(),
            );
            // the object is taken off the bus after this call returns, not from inside it
            connection
                .executor()
                .clone()
                .spawn(
                    async move { end(&connection, &sessions, &shares, &path, Ending::Closed).await },
                    "cupertino-portal-session-close",
                )
                .detach();
            Ok(())
        }
        .await;
        answered("org.freedesktop.impl.portal.Session", "Close", None, result)
    }

    /// The share ended without the frontend asking.
    #[zbus(signal)]
    async fn closed(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        1
    }
}

/// Why a session ends.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Ending {
    /// The frontend closed it: its share stops.
    Closed,
    /// Its share ended on its own: the frontend is told with `Closed`.
    ShareEnded,
    /// Its share could not start.
    NotStarted,
    /// The person stopped it from the menu bar: the share stops and the frontend is told with `Closed`.
    Stopped,
}

/// End the session at `path` and take its object off the bus. A session that already ended is left alone.
async fn end(
    connection: &Connection,
    sessions: &Sessions,
    shares: &Shares,
    path: &OwnedObjectPath,
    ending: Ending,
) {
    let Some(entry) = lock(sessions).remove(path) else {
        return;
    };
    let sharing = matches!(entry.phase, Phase::Starting | Phase::Started);
    if sharing && matches!(ending, Ending::Closed | Ending::Stopped) {
        shares(Capture::Stop {
            session: path.clone(),
        });
    }
    if let Some(indicator) = entry.indicator {
        indicator.remove().await;
    }
    if matches!(ending, Ending::ShareEnded | Ending::Stopped) {
        if let Ok(emitter) = SignalEmitter::new(connection, path.as_ref()) {
            let _ = Session::closed(&emitter).await;
        }
    }
    let _ = connection.object_server().remove::<Session, _>(path).await;
}

/// Hand a [`Capture::Start`] to the capture side unless its session has ended. It runs under the lock [`end`] takes the
/// session out with, so a Close that wins the race with the chooser's answer either finds no share or stops this one.
fn start_unless_ended(
    sessions: &Sessions,
    shares: &Shares,
    session: &OwnedObjectPath,
    start: Capture,
) -> bool {
    let sessions = lock(sessions);
    if !sessions.contains_key(session) {
        return false;
    }
    shares(start);
    true
}

/// The object on the bus.
pub struct ScreenCast {
    sources: Box<dyn Fn() -> Vec<Source> + Send + Sync>,
    events: Box<dyn Fn(Event) + Send + Sync>,
    shares: Shares,
    sessions: Sessions,
    applications: Vec<PathBuf>,
    grants: Arc<Grants>,
    /// Numbers the menu-bar items of the shares.
    shared: Arc<AtomicU64>,
}

impl ScreenCast {
    /// `sources` lists what can be shared now; `events` reaches the dialog side and `shares` the capture side. Nothing
    /// is read from or written to a file until [`ScreenCast::with_grants`] names one, and apps are shown by their ids
    /// until [`ScreenCast::with_applications`] names where their desktop entries are.
    pub fn new(
        sources: impl Fn() -> Vec<Source> + Send + Sync + 'static,
        events: impl Fn(Event) + Send + Sync + 'static,
        shares: impl Fn(Capture) + Send + Sync + 'static,
    ) -> Self {
        Self {
            sources: Box::new(sources),
            events: Box::new(events),
            shares: Arc::new(shares),
            sessions: Arc::default(),
            applications: Vec::new(),
            grants: Arc::new(Grants {
                file: None,
                transient: Mutex::default(),
            }),
            shared: Arc::default(),
        }
    }

    /// Read the desktop entries that name and draw the apps from `applications`, in precedence order.
    pub fn with_applications(mut self, applications: Vec<PathBuf>) -> Self {
        self.applications = applications;
        self
    }

    /// Keep the displays the person lets an app restore until forgotten in `file` (`screen-sharing.conf`); `None`
    /// keeps them only while the app runs.
    pub fn with_grants(mut self, file: Option<PathBuf>) -> Self {
        self.grants = Arc::new(Grants {
            file,
            transient: Mutex::default(),
        });
        self
    }

    /// The phase of the app's session at `session`.
    fn phase(&self, session: &OwnedObjectPath, app_id: &str) -> Result<Phase, PortalError> {
        lock(&self.sessions)
            .get(session)
            .filter(|e| e.app_id == app_id)
            .map(|e| e.phase.clone())
            .ok_or_else(|| invalid(format!("{session} is no session of {app_id}")))
    }

    fn set_phase(&self, session: &OwnedObjectPath, phase: Phase) {
        if let Some(entry) = lock(&self.sessions).get_mut(session) {
            entry.phase = phase;
        }
    }

    fn closed(&self, session: &OwnedObjectPath) -> Option<async_channel::Receiver<()>> {
        lock(&self.sessions).get(session).map(|e| e.closed.clone())
    }

    /// Serve the interface at [`PORTAL_PATH`]. When the frontend leaves the bus its sessions end as if it closed them,
    /// so no share outlives the frontend that could stop it.
    pub fn serve(self, connection: &zbus::blocking::Connection) -> zbus::Result<()> {
        let connection = connection.inner().clone();
        let (sessions, shares) = (Arc::clone(&self.sessions), Arc::clone(&self.shares));
        zbus::block_on(async {
            let mut owners = zbus::fdo::DBusProxy::new(&connection)
                .await?
                .receive_name_owner_changed_with_args(&[(0, FRONTEND)])
                .await?;
            connection.object_server().at(PORTAL_PATH, self).await?;
            let watcher = connection.clone();
            connection
                .executor()
                .spawn(
                    async move {
                        while let Some(change) = owners.next().await {
                            if change.args().is_ok_and(|a| a.old_owner().is_some()) {
                                let open: Vec<OwnedObjectPath> =
                                    lock(&sessions).keys().cloned().collect();
                                for path in open {
                                    end(&watcher, &sessions, &shares, &path, Ending::Closed).await;
                                }
                            }
                        }
                    },
                    "cupertino-portal-frontend",
                )
                .detach();
            Ok(())
        })
    }

    /// Ask the chooser what to share: the source and whether the person ticked to remember it, else how the call
    /// answers when it ends without a source.
    async fn choose(
        &self,
        connection: &Connection,
        handle: &OwnedObjectPath,
        session: &OwnedObjectPath,
        closed: &async_channel::Receiver<()>,
        ask: Ask,
    ) -> Result<Result<(Source, bool), u32>, PortalError> {
        let request = Exported::at(connection, handle.as_ref()).await?;
        let (reply, answer) = async_channel::bounded(1);
        (self.events)(Event::Ask {
            handle: handle.clone(),
            ask: Box::new(ask),
            reply: Reply(reply),
        });
        let answer = futures_lite::future::or(async { Some(answer.recv().await.ok()) }, async {
            futures_lite::future::or(request.closed(), async {
                let _ = closed.recv().await;
            })
            .await;
            None
        })
        .await;
        request.remove().await;
        Ok(match answer {
            Some(Some(Answer::Chosen { source, remember })) => Ok((source, remember)),
            Some(Some(Answer::Cancelled)) => {
                self.set_phase(session, Phase::Created);
                Err(Response::Cancelled as u32)
            }
            Some(None) => {
                self.set_phase(session, Phase::Created);
                Err(Response::Other as u32)
            }
            None => {
                (self.events)(Event::Close {
                    handle: handle.clone(),
                });
                Err(Response::Other as u32)
            }
        })
    }

    #[allow(clippy::too_many_arguments)]
    async fn share(
        &self,
        connection: &Connection,
        handle: OwnedObjectPath,
        session: OwnedObjectPath,
        ask: Ask,
        cursor: CursorMode,
        persist: Persist,
        restore: Option<String>,
    ) -> Result<(u32, HashMap<String, OwnedValue>), PortalError> {
        let closed = self
            .closed(&session)
            .ok_or_else(|| invalid(format!("{session} is closed")))?;
        let app_id = ask.app_id.clone();
        let app = ask
            .app_name
            .clone()
            .unwrap_or_else(|| "An application".to_owned());
        // a share the app may restore needs no chooser while the person has not forgotten it and its source is there
        let restored = restore
            .as_deref()
            .filter(|_| persist != Persist::None)
            .and_then(|id| Some((id, self.grants.restore(id, &app_id, &ask.sources)?)));
        let (source, kept) = match restored {
            Some((id, (source, lasts))) => (source, Some((id.to_owned(), lasts.min(persist)))),
            None => {
                let (source, remember) = match self
                    .choose(connection, &handle, &session, &closed, ask)
                    .await?
                {
                    Ok(chosen) => chosen,
                    Err(response) => return Ok((response, HashMap::new())),
                };
                let kept = remember
                    .then(|| self.grants.give(&app_id, &source, persist))
                    .flatten();
                (source, kept)
            }
        };
        let (started, stream) = async_channel::bounded(1);
        let (running, ended) = async_channel::bounded::<()>(1);
        let start = Capture::Start {
            session: session.clone(),
            source: source.clone(),
            cursor,
            started: Started(started),
            running: Running(running),
        };
        if !start_unless_ended(&self.sessions, &self.shares, &session, start) {
            return Ok((Response::Other as u32, HashMap::new()));
        }
        let stream = futures_lite::future::or(async { stream.recv().await.ok() }, async {
            let _ = closed.recv().await;
            None
        })
        .await;
        let stream = match stream {
            Some(Ok(stream)) => stream,
            Some(Err(reason)) => {
                eprintln!("cupertino-portal: the share of {session} did not start: {reason}");
                screencast_failed(Some(&app_id), "start", &reason);
                end(
                    connection,
                    &self.sessions,
                    &self.shares,
                    &session,
                    Ending::NotStarted,
                )
                .await;
                return Ok((Response::Other as u32, HashMap::new()));
            }
            None => return Ok((Response::Other as u32, HashMap::new())),
        };
        let (stop, stopped) = async_channel::bounded::<()>(1);
        let indicator = Indicator::show(
            connection,
            self.shared.fetch_add(1, Ordering::Relaxed),
            Shown {
                app,
                source: source.label(),
            },
            stop,
        )
        .await;
        let transient = kept
            .as_ref()
            .filter(|(_, lasts)| *lasts == Persist::Transient)
            .map(|(id, _)| id.clone());
        let orphan = {
            let mut sessions = lock(&self.sessions);
            match sessions.get_mut(&session) {
                Some(entry) => {
                    entry.phase = Phase::Started;
                    entry.indicator = indicator;
                    entry.grant = transient;
                    None
                }
                // closed while the item was put up: it goes with the session
                None => indicator,
            }
        };
        if let Some(indicator) = orphan {
            indicator.remove().await;
            return Ok((Response::Other as u32, HashMap::new()));
        }
        let (sessions, shares, grants, ended_session) = (
            Arc::clone(&self.sessions),
            Arc::clone(&self.shares),
            Arc::clone(&self.grants),
            session.clone(),
        );
        let ended_connection = connection.clone();
        connection
            .executor()
            .spawn(
                async move {
                    let stopped_by_person = futures_lite::future::or(
                        async {
                            let _ = ended.recv().await;
                            false
                        },
                        async { stopped.recv().await.is_ok() },
                    )
                    .await;
                    let ending = if stopped_by_person {
                        // stopping a share also takes back the leave to restore it until the app quits
                        let grant = lock(&sessions)
                            .get(&ended_session)
                            .and_then(|e| e.grant.clone());
                        if let Some(id) = grant {
                            grants.forget(&id);
                        }
                        Ending::Stopped
                    } else {
                        Ending::ShareEnded
                    };
                    end(
                        &ended_connection,
                        &sessions,
                        &shares,
                        &ended_session,
                        ending,
                    )
                    .await;
                },
                "cupertino-portal-share",
            )
            .detach();
        Ok((Response::Success as u32, results(&stream, kept.as_ref())?))
    }
}

type Options = HashMap<String, OwnedValue>;

#[interface(name = "org.freedesktop.impl.portal.ScreenCast")]
impl ScreenCast {
    async fn create_session(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
        _handle: OwnedObjectPath,
        session_handle: OwnedObjectPath,
        app_id: String,
        _options: Options,
    ) -> Result<(u32, HashMap<String, OwnedValue>), PortalError> {
        let app = app_id.clone();
        let result = async {
            from_frontend(&header, connection).await?;
            let app_id = text("app_id", app_id)?;
            let (closed, closed_receiver) = async_channel::bounded(1);
            {
                let mut sessions = lock(&self.sessions);
                if sessions.contains_key(&session_handle) {
                    return Err(invalid(format!("{session_handle} is already a session")));
                }
                let mine = sessions.values().filter(|e| e.app_id == app_id).count();
                if sessions.len() >= MAX_SESSIONS || mine >= MAX_SESSIONS_PER_APP {
                    return Err(PortalError::LimitsExceeded(
                        "too many screen-cast sessions are open".to_owned(),
                    ));
                }
                sessions.insert(
                    session_handle.clone(),
                    Entry {
                        app_id,
                        phase: Phase::Created,
                        closed: closed_receiver,
                        indicator: None,
                        grant: None,
                    },
                );
            }
            let session = Session {
                path: session_handle.clone(),
                closed,
                sessions: Arc::clone(&self.sessions),
                shares: Arc::clone(&self.shares),
            };
            let added = connection
                .object_server()
                .at(&session_handle, session)
                .await
                .unwrap_or(false);
            if !added {
                lock(&self.sessions).remove(&session_handle);
                return Err(invalid(format!("{session_handle} is already in use")));
            }
            Ok((Response::Success as u32, HashMap::new()))
        }
        .await;
        answered(
            "org.freedesktop.impl.portal.ScreenCast",
            "CreateSession",
            Some(&app),
            result,
        )
    }

    async fn select_sources(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
        _handle: OwnedObjectPath,
        session_handle: OwnedObjectPath,
        app_id: String,
        options: Options,
    ) -> Result<(u32, HashMap<String, OwnedValue>), PortalError> {
        let app = app_id.clone();
        let result = async {
            from_frontend(&header, connection).await?;
            match self.phase(&session_handle, &app_id)? {
                Phase::Created | Phase::Selected { .. } => {}
                Phase::Starting | Phase::Started => {
                    return Err(invalid("sources are selected before Start"));
                }
            }
            let types = option::<u32>(&options, "types")?.unwrap_or(SourceTypes::MONITOR.bits());
            if types == 0 || types & !SourceTypes::AVAILABLE.bits() != 0 {
                return Err(invalid(format!("source types {types} are not served")));
            }
            let cursor = match option::<u32>(&options, "cursor_mode")?
                .unwrap_or(CursorMode::Hidden as u32)
            {
                1 => CursorMode::Hidden,
                2 => CursorMode::Embedded,
                other => return Err(invalid(format!("cursor mode {other} is not served"))),
            };
            // one source for each Start
            option::<bool>(&options, "multiple")?;
            let persist = option::<u32>(&options, "persist_mode")?.unwrap_or(0);
            let persist = Persist::from_u32(persist)
                .ok_or_else(|| invalid(format!("persist mode {persist} is not served")))?;
            // restore data that is not this backend's, or names no grant, is ignored and the chooser shows
            let restore = options
                .get("restore_data")
                .and_then(|value| restore_id(value));
            self.set_phase(
                &session_handle,
                Phase::Selected {
                    types: SourceTypes(types),
                    cursor,
                    persist,
                    restore,
                },
            );
            Ok((Response::Success as u32, HashMap::new()))
        }
        .await;
        answered(
            "org.freedesktop.impl.portal.ScreenCast",
            "SelectSources",
            Some(&app),
            result,
        )
    }

    #[allow(clippy::too_many_arguments)]
    async fn start(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
        handle: OwnedObjectPath,
        session_handle: OwnedObjectPath,
        app_id: String,
        parent_window: String,
        _options: Options,
    ) -> Result<(u32, HashMap<String, OwnedValue>), PortalError> {
        let app = app_id.clone();
        let result = async {
            from_frontend(&header, connection).await?;
            let Phase::Selected {
                types,
                cursor,
                persist,
                restore,
            } = self.phase(&session_handle, &app_id)?
            else {
                return Err(invalid("Start comes once, after SelectSources"));
            };
            let parent_window = text("parent_window", parent_window)?;
            self.set_phase(&session_handle, Phase::Starting);
            let identity = identify(&self.applications, &app_id);
            let ask = Ask {
                offer_remember: persist != Persist::None && !app_id.is_empty(),
                app_id,
                app_name: identity.name,
                app_icon: identity.icon,
                parent_window,
                sources: (self.sources)()
                    .into_iter()
                    .filter(|s| types.contains(s.source_type()))
                    .map(Source::shown)
                    .collect(),
            };
            let started = self
                .share(
                    connection,
                    handle,
                    session_handle.clone(),
                    ask,
                    cursor,
                    persist,
                    restore,
                )
                .await;
            if started.is_err() {
                self.set_phase(&session_handle, Phase::Created);
            }
            started
        }
        .await;
        answered(
            "org.freedesktop.impl.portal.ScreenCast",
            "Start",
            Some(&app),
            result,
        )
    }

    #[zbus(property)]
    fn available_source_types(&self) -> u32 {
        SourceTypes::AVAILABLE.bits()
    }

    #[zbus(property)]
    fn available_cursor_modes(&self) -> u32 {
        AVAILABLE_CURSOR_MODES
    }

    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        VERSION
    }
}

fn invalid(why: impl Into<String>) -> PortalError {
    PortalError::InvalidArgs(why.into())
}

fn text(what: &str, value: String) -> Result<String, PortalError> {
    if value.len() > MAX_TEXT {
        return Err(invalid(format!("{what} is longer than {MAX_TEXT} bytes")));
    }
    Ok(value)
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

/// The `results` of a started share: its one stream, how long it may be restored, and the data to restore it with when
/// it may be.
fn results(
    stream: &Stream,
    kept: Option<&(String, Persist)>,
) -> Result<HashMap<String, OwnedValue>, PortalError> {
    let mut properties: HashMap<String, Value<'static>> = HashMap::from([
        (
            "source_type".to_owned(),
            Value::from(stream.source_type.bits()),
        ),
        ("size".to_owned(), Value::from(stream.size)),
    ]);
    if let Some(position) = stream.position {
        properties.insert("position".to_owned(), Value::from(position));
    }
    let streams = vec![(stream.node, properties)];
    let persist = kept.map_or(Persist::None, |(_, lasts)| *lasts);
    let mut results = HashMap::from([
        ("streams".to_owned(), owned(Value::from(streams))?),
        (
            "persist_mode".to_owned(),
            owned(Value::from(persist as u32))?,
        ),
    ]);
    if let Some((id, _)) = kept {
        results.insert("restore_data".to_owned(), restore_data(id)?);
    }
    Ok(results)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;

    use super::*;

    fn display(name: &str) -> Source {
        Source::Display {
            output: 1,
            name: name.to_owned(),
            description: String::new(),
        }
    }

    #[test]
    fn a_start_reaches_the_capture_side_only_while_its_session_lives() {
        let path =
            OwnedObjectPath::try_from("/org/freedesktop/portal/desktop/session/1_7/s").unwrap();
        let sessions: Sessions = Arc::default();
        let starts = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&starts);
        let shares: Shares = Arc::new(move |_| {
            counted.fetch_add(1, Ordering::SeqCst);
        });
        let start = || Capture::Start {
            session: path.clone(),
            source: display("DP-1"),
            cursor: CursorMode::Hidden,
            started: Started(async_channel::bounded(1).0),
            running: Running(async_channel::bounded(1).0),
        };

        assert!(
            !start_unless_ended(&sessions, &shares, &path, start()),
            "a Close that ended the session after the chooser answered"
        );
        assert_eq!(starts.load(Ordering::SeqCst), 0);

        lock(&sessions).insert(
            path.clone(),
            Entry {
                app_id: "org.example.App".to_owned(),
                phase: Phase::Starting,
                closed: async_channel::bounded(1).1,
                indicator: None,
                grant: None,
            },
        );
        assert!(start_unless_ended(&sessions, &shares, &path, start()));
        assert_eq!(starts.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn restore_data_is_read_back_only_when_it_is_this_backends_and_names_a_grant() {
        let id = grant_id().unwrap();
        let data = restore_data(&id).unwrap();
        assert_eq!(restore_id(&data), Some(id.clone()));
        let other = |vendor: &str, version: u32, inner: Value<'static>| {
            Value::Structure(
                StructureBuilder::new()
                    .add_field(vendor.to_owned())
                    .add_field(version)
                    .append_field(Value::Value(Box::new(inner)))
                    .build()
                    .unwrap(),
            )
        };
        assert_eq!(
            restore_id(&other("gnome", 1, Value::from(id.clone()))),
            None
        );
        assert_eq!(
            restore_id(&other(RESTORE_VENDOR, 2, Value::from(id.clone()))),
            None
        );
        assert_eq!(
            restore_id(&other(RESTORE_VENDOR, 1, Value::from("DP-1"))),
            None
        );
        assert_eq!(
            restore_id(&other(RESTORE_VENDOR, 1, Value::from(7u32))),
            None
        );
        assert_eq!(restore_id(&Value::from(id)), None);
    }

    #[test]
    fn a_window_is_kept_only_while_the_app_runs_and_a_display_until_forgotten() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("screen-sharing.conf");
        let grants = Grants {
            file: Some(file.clone()),
            transient: Mutex::default(),
        };
        let window = Source::Window {
            identifier: "toplevel-7".to_owned(),
            title: "Notes".to_owned(),
            app_id: "org.example.Notes".to_owned(),
        };
        let app = "org.example.Meeting";
        let (window_id, lasts) = grants.give(app, &window, Persist::Persistent).unwrap();
        assert_eq!(
            lasts,
            Persist::Transient,
            "a window has no name beyond the session"
        );
        let (display_id, lasts) = grants
            .give(app, &display("DP-1"), Persist::Persistent)
            .unwrap();
        assert_eq!(lasts, Persist::Persistent);
        let (conf, _) = ScreenSharingConf::load(&file).unwrap();
        assert_eq!(conf.grants.len(), 1, "only the display is written");
        assert!(
            grants
                .give("", &display("DP-1"), Persist::Persistent)
                .is_none(),
            "no app id"
        );
        assert!(grants.give(app, &display("DP-1"), Persist::None).is_none());

        let now = [display("DP-1"), window.clone()];
        assert_eq!(
            grants.restore(&display_id, app, &now),
            Some((display("DP-1"), Persist::Persistent))
        );
        assert_eq!(
            grants.restore(&window_id, app, &now),
            Some((window.clone(), Persist::Transient))
        );
        assert_eq!(
            grants.restore(&display_id, "org.example.Other", &now),
            None,
            "another app"
        );
        assert_eq!(
            grants.restore(&display_id, app, &[display("HDMI-A-1")]),
            None,
            "not plugged in"
        );
        assert_eq!(
            grants.restore(&window_id, app, &[display("DP-1")]),
            None,
            "the window closed"
        );

        let (mut conf, _) = ScreenSharingConf::load(&file).unwrap();
        assert!(conf.revoke(&display_id));
        conf.store(&file).unwrap();
        assert_eq!(
            grants.restore(&display_id, app, &now),
            None,
            "forgotten in Settings"
        );
        grants.forget(&window_id);
        assert_eq!(
            grants.restore(&window_id, app, &now),
            None,
            "stopped by the person"
        );
    }
}
