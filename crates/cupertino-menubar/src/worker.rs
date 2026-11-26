//! Session bus worker of the menu bar: serves the status notifier watcher, loads menu trees (DBusMenu and GMenuModel
//! exporters) and follows their changes, reads status items and relays activations.
//!
//! Everything runs as tasks of the session connection's executor, which Kit runs on the component's UI thread between
//! its frames; nothing in them waits but for the bus. The component talks to the worker through [`Worker::send`] and
//! hears its [`Event`]s on a channel. A bus call that fails is reported on stderr with what it was for; the bars carry
//! on.

use crate::dbus::dbusmenu::{DBusMenuClient, MenuEvent};
use crate::dbus::gtk::{GtkChange, GtkMenuClient, GtkStarted};
use crate::dbus::registrar;
use crate::dbus::sni::{self, StatusItem, WatcherEvent, WatcherService};
use crate::follow::{Command, Follower};
use crate::item::{ItemId, MenuItem};
use crate::registered::Registration;
use crate::source::{GtkMenuProps, KdeAppMenu, MenuSource};
use crate::spawned::{Spawned, Spawner};
use futures_util::StreamExt;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::Arc;
use std::time::Duration;
use zbus::Connection;

mod exporters;

use exporters::Exporters;

/// The first wait before a GTK menu load refused with `AccessDenied` is made again; it doubles up to
/// [`GTK_RETRY_LAST`], then the refusal is reported. A CupertinoKit application learns that the bar's connection reads
/// its menus over its own Wayland connection, which can come after the bar's first load.
const GTK_RETRY_FIRST: Duration = Duration::from_millis(100);
const GTK_RETRY_LAST: Duration = Duration::from_millis(1600);
/// The changes of a GTK exporter reach the component at most this often: those that come in between are applied to its
/// cache and sent as one tree, so an exporter that changes without pause cannot flood the bar. The groups those changes
/// link to are subscribed as the tree is sent, so they are asked for no more often either.
const GTK_CHANGE_INTERVAL: Duration = Duration::from_millis(50);
/// A change that came while an exporter's menus loaded may or may not be in what the load read, so they are loaded
/// again, this many times at most.
const GTK_RELOADS: u8 = 2;

pub use crate::follow::Waits;

/// What the component asks of the worker.
#[derive(Clone, Debug, PartialEq)]
pub enum Request {
    /// Load the whole tree of a DBusMenu exporter and follow its changes until it is released: every change is
    /// reported as a tree.
    FetchDbusMenu(KdeAppMenu),
    /// No bar shows the menus of a DBusMenu exporter any more: stop following it.
    ReleaseDbusMenu(KdeAppMenu),
    /// Load the menu bar of a GMenuModel exporter and follow its changes until it is released.
    FetchGtk(GtkMenuProps),
    /// No bar shows the menus of a GMenuModel exporter any more: stop following it and end its subscriptions.
    ReleaseGtk(GtkMenuProps),
    /// The menu of an item (0 for the whole menu) of a followed DBusMenu exporter, an application's or a status
    /// item's, is about to be shown: tell the exporter, then read again what it says has changed. The answer is waited
    /// for by the worker, never by the bar, and no longer than [`Waits::about_to_show`].
    AboutToShow { menu: KdeAppMenu, item: i32 },
    /// Send `clicked` for an item of a DBusMenu, at `time` (milliseconds of the input clock).
    ActivateDbusMenu {
        menu: KdeAppMenu,
        item: i32,
        time: u32,
    },
    /// Tell a followed DBusMenu exporter that the menu of one of its items (0 for the whole menu) opened or closed, at
    /// `time` (milliseconds of the input clock).
    MenuEvent {
        menu: KdeAppMenu,
        item: i32,
        event: MenuEvent,
        time: u32,
    },
    /// Activate the action of an item of a GMenuModel exporter, handing it `token` (an activation token of the bar's)
    /// so a window the action opens takes the focus.
    ActivateGtk {
        props: GtkMenuProps,
        item: i32,
        token: Option<String>,
    },
    /// An item of a GMenuModel exporter was chosen and its activation waits for the bar's activation token: take what
    /// it activates now (its action's name and target, and the object that owns it) under `click`, so an exporter that
    /// rebuilds its tree meanwhile, numbering its items afresh, still has the chosen item's action run
    /// ([`Request::ActivateChosen`]).
    ChooseGtk {
        props: GtkMenuProps,
        item: i32,
        click: u64,
    },
    /// Run the activation taken under `click` ([`Request::ChooseGtk`]), handing it `token`.
    ActivateChosen { click: u64, token: Option<String> },
    /// Send a status item a click or a scroll (`Activate`, `SecondaryActivate`, `ContextMenu`, `Scroll`), at a point
    /// of the compositor space.
    StatusAction {
        service: String,
        path: String,
        action: sni::StatusAction,
    },
    /// Own `com.canonical.AppMenu.Registrar` while a bar exists, so toolkits keep their own menu bar otherwise.
    AppMenuRegistrar(bool),
    /// Forget registered menus, by connection and window: their X11 windows are gone.
    ForgetRegistrations(Vec<(String, u32)>),
}

/// What the worker reports.
#[derive(Clone, Debug)]
pub enum Event {
    /// A menu tree arrived from an exporter: the first, or the tree after a change. A status item's menu comes this
    /// way too, under the item's service and the path of its menu.
    Layout {
        source: MenuSource,
        root: Box<MenuItem>,
    },
    /// A status item was read. Its menu follows as an [`Event::Layout`], and again whenever it changes.
    StatusUpdated {
        item: Box<StatusItem>,
    },
    StatusRemoved {
        id: String,
    },
    /// A menu tree could not be loaded (reported on stderr); it is asked for again when the server announces it again.
    LayoutFailed {
        source: MenuSource,
    },
    /// The menus registered with `com.canonical.AppMenu.Registrar` changed: all of them now.
    Registrations(Vec<Registration>),
    /// A DBusMenu exporter asks for the menu of its item `item` to be opened (`ItemActivationRequested`).
    ActivationRequested {
        source: MenuSource,
        item: i32,
    },
}

/// Why the worker did not start.
#[derive(Debug)]
pub enum StartError {
    /// The status notifier watcher could not be served; `NameTaken` means another process owns
    /// `org.kde.StatusNotifierWatcher`.
    Watcher(zbus::Error),
}

impl fmt::Display for StartError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Watcher(zbus::Error::NameTaken) => {
                write!(f, "another process owns {}", sni::WATCHER_NAME)
            }
            Self::Watcher(e) => write!(f, "cannot serve {}: {e}", sni::WATCHER_NAME),
        }
    }
}

impl std::error::Error for StartError {}

/// The worker's tasks are gone, so requests cannot be served any more.
#[derive(Debug)]
pub struct WorkerGone;

impl fmt::Display for WorkerGone {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the session bus worker ended")
    }
}

impl std::error::Error for WorkerGone {}

enum Work {
    Request(Request),
    Watcher(WatcherEvent),
    /// The status item `id` was read: `menu` is where its menu is now, `None` without one.
    StatusMenu {
        id: String,
        menu: Option<KdeAppMenu>,
    },
    /// A GTK exporter's menus loaded, with their tree: the task that forwards its `Changed` signals, when they could be
    /// followed, and how many loads came before this one.
    GtkLoaded {
        props: GtkMenuProps,
        client: Box<GtkMenuClient>,
        items: Vec<MenuItem>,
        follow: Option<zbus::Task<()>>,
        reloads: u8,
    },
    /// A change of a GTK exporter; `applied` is answered once it is, so an exporter has one change waiting at most.
    GtkChanged(GtkMenuProps, GtkChange, async_channel::Sender<()>),
    /// Send the tree of an exporter whose changes were applied since it was last sent.
    GtkFlush(GtkMenuProps),
    /// The replies of a subscription to an exporter's groups, made for its load `generation`.
    GtkStarted {
        props: GtkMenuProps,
        generation: u64,
        started: Box<GtkStarted>,
    },
}

/// Sends events to the component.
#[derive(Clone)]
pub(crate) struct Out {
    events: async_channel::Sender<Event>,
}

impl Out {
    pub(crate) fn emit(&self, event: Event) {
        // a closed channel means the component is ending and nobody needs the event; an unbounded one is never full
        let _ = self.events.try_send(event);
    }
}

/// Whether the exporter refused this connection, as a CupertinoKit application does until it knows the bar reads its
/// menus.
fn is_access_denied(e: &zbus::Error) -> bool {
    const ACCESS_DENIED: &str = "org.freedesktop.DBus.Error.AccessDenied";
    match e {
        zbus::Error::MethodError(name, _, _) => name.as_str() == ACCESS_DENIED,
        zbus::Error::FDO(e) => matches!(**e, zbus::fdo::Error::AccessDenied(_)),
        _ => false,
    }
}

pub(crate) fn report(context: &str, error: impl fmt::Display) {
    eprintln!("cupertino-menubar: {context}: {error}");
}

/// Handle on the worker: its loop and every task it started run as long as this lives.
pub struct Worker {
    work: async_channel::Sender<Work>,
    _tasks: Spawned,
}

impl Worker {
    /// Start the worker on `conn`, once it serves the watcher; its events come on the channel returned.
    pub async fn start(
        conn: Connection,
    ) -> Result<(Worker, async_channel::Receiver<Event>), StartError> {
        Self::start_with(conn, Waits::default()).await
    }

    /// [`Worker::start`] with other waits for the answers of menu exporters.
    pub async fn start_with(
        conn: Connection,
        waits: Waits,
    ) -> Result<(Worker, async_channel::Receiver<Event>), StartError> {
        let (work_tx, work_rx) = async_channel::unbounded();
        let (events_tx, events_rx) = async_channel::unbounded();
        let out = Out { events: events_tx };
        let tasks = Spawned::default();
        let spawner = tasks.spawner(conn.executor());
        let registrar = serve(&conn, &spawner, &work_tx, &out).await?;
        spawner.spawn(
            run(
                conn,
                spawner.clone(),
                work_rx,
                work_tx.clone(),
                out,
                waits,
                registrar,
            ),
            "cupertino-menubar-worker",
        );
        Ok((
            Worker {
                work: work_tx,
                _tasks: tasks,
            },
            events_rx,
        ))
    }

    pub fn send(&self, request: Request) -> Result<(), WorkerGone> {
        self.work
            .try_send(Work::Request(request))
            .map_err(|_| WorkerGone)
    }
}

/// Move the events of a channel into the worker's queue, as a task of the worker's.
fn forward<T: Send + 'static>(
    spawner: &Spawner,
    rx: async_channel::Receiver<T>,
    tx: async_channel::Sender<Work>,
    wrap: fn(T) -> Work,
) {
    spawner.spawn(
        async move {
            while let Ok(event) = rx.recv().await {
                if tx.send(wrap(event)).await.is_err() {
                    break;
                }
            }
        },
        "cupertino-menubar-forward",
    );
}

async fn run(
    conn: Connection,
    spawner: Spawner,
    rx: async_channel::Receiver<Work>,
    tx: async_channel::Sender<Work>,
    out: Out,
    waits: Waits,
    registrar: Option<registrar::Registrar>,
) {
    let mut gtk = Exporters::new(&conn, &spawner, &tx, &out);
    let mut menus = Followed {
        conn: conn.clone(),
        out: out.clone(),
        waits,
        menus: HashMap::new(),
    };
    // the task that reads each status item and follows its changes; dropping it ends the following
    let mut items: HashMap<String, zbus::Task<()>> = HashMap::new();
    while let Ok(work) = rx.recv().await {
        match work {
            Work::Watcher(WatcherEvent::ItemRegistered { id, service, path }) => {
                let follow = follow_status_item(
                    conn.clone(),
                    out.clone(),
                    tx.clone(),
                    id.clone(),
                    service,
                    path,
                );
                let task = conn
                    .executor()
                    .clone()
                    .spawn(follow, "cupertino-status-item");
                items.insert(id, task);
            }
            Work::Watcher(WatcherEvent::ItemUnregistered { id }) => {
                items.remove(&id);
                menus.hold(&Holder::Status(id.clone()), None);
                out.emit(Event::StatusRemoved { id })
            }
            // read before its item went: nothing shows it
            Work::StatusMenu { id, .. } if !items.contains_key(&id) => {}
            Work::StatusMenu { id, menu } => menus.hold(&Holder::Status(id), menu),
            Work::Request(Request::FetchDbusMenu(menu)) => menus.hold(&Holder::Bars, Some(menu)),
            Work::Request(Request::ReleaseDbusMenu(menu)) => menus.release(&Holder::Bars, &menu),
            Work::Request(Request::AboutToShow { menu, item }) => menus.show(&menu, item),
            Work::Request(Request::MenuEvent {
                menu,
                item,
                event,
                time,
            }) => menus.send(&menu, Command::Event { item, event, time }),
            // hosts need no action: this component is the host
            Work::Watcher(WatcherEvent::HostRegistered) => {}
            Work::GtkLoaded {
                props,
                client,
                items,
                follow,
                reloads,
            } => gtk.loaded(props, *client, items, follow, reloads),
            Work::GtkChanged(props, change, applied) => gtk.changed(props, change, &applied),
            Work::GtkFlush(props) => gtk.flush(props),
            Work::GtkStarted {
                props,
                generation,
                started,
            } => gtk.started(props, generation, *started),
            // awaited in order: a bar that goes and comes back leaves the name owned
            Work::Request(Request::AppMenuRegistrar(owned)) => {
                app_menu_registrar(&conn, owned).await
            }
            Work::Request(Request::ForgetRegistrations(gone)) => {
                if let Some(registrar) = &registrar {
                    registrar.forget(&gone);
                }
            }
            Work::Request(request) => {
                if let Some(request) = gtk.handle(request) {
                    handle(&conn, &spawner, request)
                }
            }
        }
    }
}

/// Who shows the menu of a followed DBusMenu exporter.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Holder {
    /// The bars, as the menus of an application.
    Bars,
    /// The status item of this id.
    Status(String),
}

/// The DBusMenu exporters followed, each for as long as something shows its menu.
struct Followed {
    conn: Connection,
    out: Out,
    waits: Waits,
    menus: HashMap<KdeAppMenu, (Follower, HashSet<Holder>)>,
}

impl Followed {
    /// `holder` shows `menu` from now on. A status item shows one menu: the one it showed before is released, so
    /// `None` releases it.
    fn hold(&mut self, holder: &Holder, menu: Option<KdeAppMenu>) {
        if matches!(holder, Holder::Status(_)) {
            let held: Vec<KdeAppMenu> = self
                .menus
                .iter()
                .filter(|(key, (_, holders))| {
                    holders.contains(holder) && Some(*key) != menu.as_ref()
                })
                .map(|(key, _)| key.clone())
                .collect();
            for key in held {
                self.release(holder, &key);
            }
        }
        let Some(menu) = menu else {
            return;
        };
        match self.menus.get_mut(&menu) {
            // its holder has no tree yet, or lost it: the follower reports it again
            Some((follower, holders)) => {
                holders.insert(holder.clone());
                follower.send(Command::Publish);
            }
            None => {
                let patient = matches!(holder, Holder::Status(_));
                let follower = Follower::start(
                    &self.conn,
                    self.out.clone(),
                    menu.clone(),
                    patient,
                    self.waits,
                );
                self.menus
                    .insert(menu, (follower, HashSet::from([holder.clone()])));
            }
        }
    }

    /// `holder` shows `menu` no more; a menu nothing shows is followed no more.
    fn release(&mut self, holder: &Holder, menu: &KdeAppMenu) {
        if let Some((_, holders)) = self.menus.get_mut(menu) {
            holders.remove(holder);
            if holders.is_empty() {
                if let Some((follower, _)) = self.menus.remove(menu) {
                    follower.finish();
                }
            }
        }
    }

    /// The menu of `item` of `menu` is about to show. A menu that is not followed is shown by nothing.
    fn show(&self, menu: &KdeAppMenu, item: i32) {
        self.send(menu, Command::Show(item));
    }

    /// Hand `command` to the follower of `menu`; a menu that is not followed is shown by nothing.
    fn send(&self, menu: &KdeAppMenu, command: Command) {
        if let Some((follower, _)) = self.menus.get(menu) {
            follower.send(command);
        }
    }
}

/// Claim the watcher and forward its events, then serve the registrar (`None` when it cannot be served).
async fn serve(
    conn: &Connection,
    spawner: &Spawner,
    tx: &async_channel::Sender<Work>,
    out: &Out,
) -> Result<Option<registrar::Registrar>, StartError> {
    let (_service, events) = WatcherService::start(conn, spawner)
        .await
        .map_err(StartError::Watcher)?;
    forward(spawner, events, tx.clone(), Work::Watcher);
    let out = out.clone();
    let changed: registrar::Changed = Arc::new(move |list| out.emit(Event::Registrations(list)));
    match registrar::serve(conn, changed, spawner).await {
        Ok(registrar) => Ok(Some(registrar)),
        Err(e) => {
            report(&format!("cannot serve {}", registrar::REGISTRAR_NAME), e);
            Ok(None)
        }
    }
}

async fn app_menu_registrar(conn: &Connection, owned: bool) {
    let result = if owned {
        registrar::own(conn).await
    } else {
        registrar::release(conn).await
    };
    // another registrar makes toolkits export their menus just the same
    if let Err(e) = result {
        let verb = if owned { "own" } else { "release" };
        report(&format!("cannot {verb} {}", registrar::REGISTRAR_NAME), e);
    }
}

/// Publish the status item `id`, then again every time it changes (the place of its menu included). The item's
/// changes are followed before it is first read, so none between the two is missed.
async fn follow_status_item(
    conn: Connection,
    out: Out,
    tx: async_channel::Sender<Work>,
    id: String,
    service: String,
    path: String,
) {
    let mut changes = match sni::item_changes(&conn, &service, &path).await {
        Ok(changes) => Some(changes),
        Err(e) => {
            report(&format!("cannot follow the status item {id}"), e);
            None
        }
    };
    // what a change names is read again of the item last read; the whole item when none was, or that reading fails
    let mut last: Option<StatusItem> = None;
    let mut change: Option<sni::ItemChange> = None;
    loop {
        let read = match (last.take(), &change) {
            (Some(mut item), Some(change)) => {
                match sni::refresh_item(&conn, &mut item, change).await {
                    Ok(()) => Ok(item),
                    Err(_) => sni::fetch_item(&conn, &service, &path).await,
                }
            }
            _ => sni::fetch_item(&conn, &service, &path).await,
        };
        match read {
            Ok(item) => {
                last = Some(item.clone());
                let menu = item.menu_path.clone().map(|path| KdeAppMenu {
                    service: item.service.clone(),
                    path,
                });
                // the item first, then its menu: the tree the follower reports finds the item it belongs to
                out.emit(Event::StatusUpdated {
                    item: Box::new(item),
                });
                let work = Work::StatusMenu {
                    id: id.clone(),
                    menu,
                };
                // the worker drops its receiver only when it ends
                if tx.send(work).await.is_err() {
                    return;
                }
            }
            Err(e) => report(&format!("status item {id} is not readable"), e),
        }
        let Some(changes) = &mut changes else {
            return;
        };
        change = changes.next().await;
        if change.is_none() {
            return;
        }
    }
}

fn handle(conn: &Connection, spawner: &Spawner, request: Request) {
    let conn = conn.clone();
    let executor = spawner;
    match request {
        Request::ActivateDbusMenu { menu, item, time } => executor.spawn(
            async move {
                let result = match DBusMenuClient::connect(&conn, &menu.service, &menu.path).await {
                    Ok(client) => client.activate(ItemId(item), time).await,
                    Err(e) => Err(e),
                };
                if let Err(e) = result {
                    report(
                        &format!(
                            "cannot activate item {item} of {}{}",
                            menu.service, menu.path
                        ),
                        e,
                    );
                }
            },
            "cupertino-activate-menu",
        ),
        Request::StatusAction {
            service,
            path,
            action,
        } => executor.spawn(
            async move {
                if let Err(e) = sni::send_action(&conn, &service, &path, action).await {
                    report(&format!("cannot send {action:?} to {service}{path}"), e);
                }
            },
            "cupertino-status-action",
        ),
        Request::AppMenuRegistrar(_)
        | Request::ForgetRegistrations(_)
        | Request::FetchDbusMenu(_)
        | Request::ReleaseDbusMenu(_)
        | Request::AboutToShow { .. }
        | Request::FetchGtk(_)
        | Request::ReleaseGtk(_)
        | Request::ActivateGtk { .. }
        | Request::ChooseGtk { .. }
        | Request::ActivateChosen { .. }
        | Request::MenuEvent { .. } => {
            unreachable!("the worker's loop handles the registrar, the followed menus and the GMenuModel exporters")
        }
    }
}
