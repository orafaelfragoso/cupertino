//! `org.freedesktop.impl.portal.ScreenCast`: `CreateSession`, `SelectSources` and `Start`, and the
//! `org.freedesktop.impl.portal.Session` object of each session. `Start` becomes an [`Ask`] for the dialog, which answers
//! with the source the person chose through its [`Reply`]; the capture side then shares that source as a PipeWire
//! stream ([`Capture::Start`]) until the session is closed ([`Capture::Stop`]) or the share ends on its own, which closes
//! the session.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use futures_lite::StreamExt;
use zbus::message::Header;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};
use zbus::{interface, Connection};

use crate::file_chooser::plain;
use crate::request::{from_frontend, Exported, PortalError, Response, FRONTEND};
use crate::PORTAL_PATH;

/// The interface version served.
pub const VERSION: u32 = 5;
/// At most this many sessions exist at once; another is refused.
pub const MAX_SESSIONS: usize = 16;
/// At most this many of them for one app id.
pub const MAX_SESSIONS_PER_APP: usize = 4;
/// Bytes of an app id, a parent window and a shown name.
pub const MAX_TEXT: usize = 1024;

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
}

/// What the chooser shows: the sources of the requested types, as they were when `Start` was called.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ask {
    pub app_id: String,
    /// `wayland:<handle>`, `x11:<xid>` or empty.
    pub parent_window: String,
    pub sources: Vec<Source>,
}

/// How the chooser ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    Chosen(Source),
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Created,
    Selected {
        types: SourceTypes,
        cursor: CursorMode,
    },
    /// The chooser is open, or the share is being set up.
    Starting,
    Started,
}

struct Entry {
    app_id: String,
    phase: Phase,
    closed: async_channel::Receiver<()>,
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
    drop(entry);
    if sharing && ending == Ending::Closed {
        shares(Capture::Stop {
            session: path.clone(),
        });
    }
    if ending == Ending::ShareEnded {
        if let Ok(emitter) = SignalEmitter::new(connection, path.as_ref()) {
            let _ = Session::closed(&emitter).await;
        }
    }
    let _ = connection.object_server().remove::<Session, _>(path).await;
}

/// The object on the bus.
pub struct ScreenCast {
    sources: Box<dyn Fn() -> Vec<Source> + Send + Sync>,
    events: Box<dyn Fn(Event) + Send + Sync>,
    shares: Shares,
    sessions: Sessions,
}

impl ScreenCast {
    /// `sources` lists what can be shared now; `events` reaches the dialog side and `shares` the capture side.
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
        }
    }

    /// The phase of the app's session at `session`, which must be `expected`.
    fn phase(&self, session: &OwnedObjectPath, app_id: &str) -> Result<Phase, PortalError> {
        lock(&self.sessions)
            .get(session)
            .filter(|e| e.app_id == app_id)
            .map(|e| e.phase)
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

    async fn share(
        &self,
        connection: &Connection,
        handle: OwnedObjectPath,
        session: OwnedObjectPath,
        ask: Ask,
        cursor: CursorMode,
    ) -> Result<(u32, HashMap<String, OwnedValue>), PortalError> {
        let closed = self
            .closed(&session)
            .ok_or_else(|| invalid(format!("{session} is closed")))?;
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
        let source = match answer {
            Some(Some(Answer::Chosen(source))) => source,
            Some(Some(Answer::Cancelled)) => {
                self.set_phase(&session, Phase::Created);
                return Ok((Response::Cancelled as u32, HashMap::new()));
            }
            Some(None) => {
                self.set_phase(&session, Phase::Created);
                return Ok((Response::Other as u32, HashMap::new()));
            }
            None => {
                (self.events)(Event::Close { handle });
                return Ok((Response::Other as u32, HashMap::new()));
            }
        };
        let (started, stream) = async_channel::bounded(1);
        let (running, ended) = async_channel::bounded::<()>(1);
        (self.shares)(Capture::Start {
            session: session.clone(),
            source,
            cursor,
            started: Started(started),
            running: Running(running),
        });
        let stream = futures_lite::future::or(async { stream.recv().await.ok() }, async {
            let _ = closed.recv().await;
            None
        })
        .await;
        let stream = match stream {
            Some(Ok(stream)) => stream,
            Some(Err(reason)) => {
                eprintln!("cupertino-portal: the share of {session} did not start: {reason}");
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
        self.set_phase(&session, Phase::Started);
        let (sessions, shares, ended_session) = (
            Arc::clone(&self.sessions),
            Arc::clone(&self.shares),
            session.clone(),
        );
        let ended_connection = connection.clone();
        connection
            .executor()
            .spawn(
                async move {
                    let _ = ended.recv().await;
                    end(
                        &ended_connection,
                        &sessions,
                        &shares,
                        &ended_session,
                        Ending::ShareEnded,
                    )
                    .await;
                },
                "cupertino-portal-share",
            )
            .detach();
        Ok((Response::Success as u32, results(&stream)?))
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

    async fn select_sources(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
        _handle: OwnedObjectPath,
        session_handle: OwnedObjectPath,
        app_id: String,
        options: Options,
    ) -> Result<(u32, HashMap<String, OwnedValue>), PortalError> {
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
        let cursor =
            match option::<u32>(&options, "cursor_mode")?.unwrap_or(CursorMode::Hidden as u32) {
                1 => CursorMode::Hidden,
                2 => CursorMode::Embedded,
                other => return Err(invalid(format!("cursor mode {other} is not served"))),
            };
        // one source for each Start, and nothing is kept for a later session
        option::<bool>(&options, "multiple")?;
        option::<u32>(&options, "persist_mode")?;
        self.set_phase(
            &session_handle,
            Phase::Selected {
                types: SourceTypes(types),
                cursor,
            },
        );
        Ok((Response::Success as u32, HashMap::new()))
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
        from_frontend(&header, connection).await?;
        let Phase::Selected { types, cursor } = self.phase(&session_handle, &app_id)? else {
            return Err(invalid("Start comes once, after SelectSources"));
        };
        self.set_phase(&session_handle, Phase::Starting);
        let ask = Ask {
            app_id,
            parent_window: text("parent_window", parent_window)?,
            sources: (self.sources)()
                .into_iter()
                .filter(|s| types.contains(s.source_type()))
                .map(Source::shown)
                .collect(),
        };
        let started = self
            .share(connection, handle, session_handle.clone(), ask, cursor)
            .await;
        if started.is_err() {
            self.set_phase(&session_handle, Phase::Created);
        }
        started
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

/// The `results` of a started share: its one stream, and a persist mode of 0, since nothing is restored later.
fn results(stream: &Stream) -> Result<HashMap<String, OwnedValue>, PortalError> {
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
    Ok(HashMap::from([
        ("streams".to_owned(), owned(Value::from(streams))?),
        ("persist_mode".to_owned(), owned(Value::from(0u32))?),
    ]))
}
