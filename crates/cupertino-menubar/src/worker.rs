//! Session bus worker of the `cupertino-menubar` process: serves the status notifier watcher, loads menu trees (DBusMenu and GMenuModel exporters), reads
//! status items and relays activations.
//!
//! Everything runs on one background thread that drives `zbus` futures. The process talks to it
//! through [`Worker::send`] and reads [`Event`]s from a channel; each event wakes the process's
//! loop. A bus call that fails is reported on stderr with what it was for; the bars carry on.

use crate::dbus::dbusmenu::DBusMenuClient;
use crate::dbus::gtk::{GtkChange, GtkMenuClient};
use crate::dbus::registrar;
use crate::dbus::sni::{self, StatusItem, WatcherEvent, WatcherService};
use crate::item::{ItemId, MenuItem};
use crate::source::{GtkMenuProps, KdeAppMenu, MenuSource};
use futures_util::stream::BoxStream;
use futures_util::StreamExt;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;
use zbus::Connection;

/// The first wait before a GTK menu load refused with `AccessDenied` is made again; it doubles up to
/// [`GTK_RETRY_LAST`], then the refusal is reported. A CupertinoKit application learns that the bar's connection reads
/// its menus over its own Wayland connection, which can come after the bar's first load.
const GTK_RETRY_FIRST: Duration = Duration::from_millis(100);
const GTK_RETRY_LAST: Duration = Duration::from_millis(1600);
/// The changes of a GTK exporter reach the process at most this often: those that come in between are applied to its
/// cache and sent as one tree, so an exporter that changes without pause cannot flood the bar.
const GTK_CHANGE_INTERVAL: Duration = Duration::from_millis(50);
/// A change that came while an exporter's menus loaded may or may not be in what the load read, so they are loaded
/// again, this many times at most.
const GTK_RELOADS: u8 = 2;

/// The interface of the notification process (`cupertino-notifications`, its `dbus` module).
const CENTRE_NAME: &str = "org.cupertino.NotificationCenter1";
const CENTRE_PATH: &str = "/org/cupertino/NotificationCenter1";
/// The interface of the control panel process (`cupertino-controlcenter`, its `control` module).
const PANEL_NAME: &str = "org.cupertino.ControlPanel1";
const PANEL_PATH: &str = "/org/cupertino/ControlPanel1";

/// What the process asks of the worker.
#[derive(Clone, Debug, PartialEq)]
pub enum Request {
    /// Load the whole tree of a DBusMenu exporter.
    FetchDbusMenu(KdeAppMenu),
    /// Load the menu bar of a GMenuModel exporter.
    FetchGtk(GtkMenuProps),
    /// A menu is about to be shown: tell the exporter, then reload its tree.
    AboutToShow {
        menu: KdeAppMenu,
        item: i32,
    },
    /// Send `clicked` for an item of a DBusMenu.
    ActivateDbusMenu {
        menu: KdeAppMenu,
        item: i32,
    },
    ActivateGtk {
        props: GtkMenuProps,
        item: i32,
    },
    /// Primary action of a status item at a point of the compositor space.
    ActivateStatus {
        service: String,
        path: String,
        x: i32,
        y: i32,
    },
    /// Open or close the list of the notification process.
    ToggleCentre,
    /// Open or close the control panel process.
    TogglePanel,
    /// Own `com.canonical.AppMenu.Registrar` while a bar exists, so toolkits keep their own menu bar otherwise.
    AppMenuRegistrar(bool),
}

/// What the worker reports.
#[derive(Debug)]
pub enum Event {
    /// A menu tree arrived from an exporter.
    Layout {
        source: MenuSource,
        root: MenuItem,
    },
    StatusUpdated {
        item: Box<StatusItem>,
        menu: Option<MenuItem>,
    },
    StatusRemoved {
        id: String,
    },
    /// A menu tree could not be loaded (reported on stderr); it is asked for again when the server announces it again.
    LayoutFailed {
        source: MenuSource,
    },
}

/// Why the worker did not start.
#[derive(Debug)]
pub enum StartError {
    /// The status notifier watcher could not be served; `NameTaken` means another process owns
    /// `org.kde.StatusNotifierWatcher`.
    Watcher(zbus::Error),
    Thread(std::io::Error),
    /// The worker thread ended before it reported.
    Died,
}

impl fmt::Display for StartError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Watcher(zbus::Error::NameTaken) => {
                write!(f, "another process owns {}", sni::WATCHER_NAME)
            }
            Self::Watcher(e) => write!(f, "cannot serve {}: {e}", sni::WATCHER_NAME),
            Self::Thread(e) => write!(f, "cannot start the session bus worker: {e}"),
            Self::Died => write!(f, "the session bus worker ended while starting"),
        }
    }
}

impl std::error::Error for StartError {}

/// The worker thread is gone, so requests cannot be served any more.
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
    /// A GTK exporter's menus loaded: the task that forwards its `Changed` signals, when they could be followed, and
    /// how many loads came before this one.
    GtkLoaded {
        props: GtkMenuProps,
        client: Box<GtkMenuClient>,
        follow: Option<zbus::Task<()>>,
        reloads: u8,
    },
    /// A change of a GTK exporter; `applied` is answered once it is, so an exporter has one change waiting at most.
    GtkChanged(GtkMenuProps, GtkChange, async_channel::Sender<()>),
    /// Send the tree of an exporter whose changes were applied since it was last sent.
    GtkFlush(GtkMenuProps),
}

/// Sends events to the process and wakes its loop.
#[derive(Clone)]
struct Out {
    events: mpsc::Sender<Event>,
    wake: Arc<dyn Fn() + Send + Sync>,
}

impl Out {
    fn emit(&self, event: Event) {
        // a closed channel means the process is shutting down and nobody needs the event
        if self.events.send(event).is_ok() {
            (self.wake)();
        }
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

fn report(context: &str, error: impl fmt::Display) {
    eprintln!("cupertino-menubar: {context}: {error}");
}

/// Handle on the worker thread.
pub struct Worker {
    work: async_channel::Sender<Work>,
}

impl Worker {
    /// Start the worker on `conn` and wait until it serves the watcher. `wake`
    /// is called after every event.
    pub fn start(
        conn: Connection,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Result<(Worker, mpsc::Receiver<Event>), StartError> {
        let (work_tx, work_rx) = async_channel::unbounded();
        let (events_tx, events_rx) = mpsc::channel();
        let (started_tx, started_rx) = mpsc::sync_channel(1);
        let out = Out {
            events: events_tx,
            wake: Arc::new(wake),
        };
        let tx = work_tx.clone();
        std::thread::Builder::new()
            .name("cupertino-dbus".into())
            .spawn(move || zbus::block_on(run(conn, work_rx, tx, out, started_tx)))
            .map_err(StartError::Thread)?;
        started_rx.recv().map_err(|_| StartError::Died)??;
        Ok((Worker { work: work_tx }, events_rx))
    }

    pub fn send(&self, request: Request) -> Result<(), WorkerGone> {
        self.work
            .try_send(Work::Request(request))
            .map_err(|_| WorkerGone)
    }
}

/// Move the events of a blocking channel into the worker's queue.
fn forward<T: Send + 'static>(
    rx: mpsc::Receiver<T>,
    tx: async_channel::Sender<Work>,
    wrap: fn(T) -> Work,
) {
    std::thread::spawn(move || {
        while let Ok(event) = rx.recv() {
            if tx.send_blocking(wrap(event)).is_err() {
                break;
            }
        }
    });
}

async fn run(
    conn: Connection,
    rx: async_channel::Receiver<Work>,
    tx: async_channel::Sender<Work>,
    out: Out,
    started: mpsc::SyncSender<Result<(), StartError>>,
) {
    let services = serve(&conn, &tx).await;
    let failed = services.is_err();
    // the caller holds the receiver until this is read
    started
        .send(services)
        .expect("the starting thread waits for the report");
    if failed {
        return;
    }
    let mut gtk: HashMap<GtkMenuProps, GtkMenuClient> = HashMap::new();
    // the task that forwards each exporter's changes; replacing or dropping it ends the subscription
    let mut follows: HashMap<GtkMenuProps, zbus::Task<()>> = HashMap::new();
    let mut unsent: HashSet<GtkMenuProps> = HashSet::new();
    // exporters whose menus are loaded again, and those a change came for while they loaded
    let mut reloading: HashSet<GtkMenuProps> = HashSet::new();
    let mut missed: HashSet<GtkMenuProps> = HashSet::new();
    while let Ok(work) = rx.recv().await {
        match work {
            Work::Watcher(WatcherEvent::ItemRegistered { service, path, .. }) => {
                let (conn, out) = (conn.clone(), out.clone());
                conn.executor()
                    .clone()
                    .spawn(
                        follow_status_item(conn, out, service, path),
                        "cupertino-status-item",
                    )
                    .detach();
            }
            Work::Watcher(WatcherEvent::ItemUnregistered { id }) => {
                out.emit(Event::StatusRemoved { id })
            }
            // hosts need no action: this process is the host
            Work::Watcher(WatcherEvent::HostRegistered) => {}
            Work::GtkLoaded {
                props,
                client,
                follow,
                reloads,
            } => {
                gtk.insert(props.clone(), *client);
                if reloads == 0 {
                    match follow {
                        Some(task) => {
                            follows.insert(props.clone(), task);
                        }
                        None => {
                            follows.remove(&props);
                        }
                    }
                }
                reloading.remove(&props);
                if missed.remove(&props) && reloads < GTK_RELOADS {
                    reloading.insert(props.clone());
                    reload_gtk(&conn, &out, &tx, props, reloads + 1);
                }
            }
            Work::GtkChanged(props, change, applied) => {
                // the forwarding task waits for the answer; it is gone only when its exporter was replaced
                let _ = applied.try_send(());
                if reloading.contains(&props) || !gtk.contains_key(&props) {
                    missed.insert(props.clone());
                }
                let Some(client) = gtk.get_mut(&props) else {
                    continue;
                };
                client.apply(change);
                if unsent.insert(props.clone()) {
                    let tx = tx.clone();
                    conn.executor()
                        .clone()
                        .spawn(
                            async move {
                                async_io::Timer::after(GTK_CHANGE_INTERVAL).await;
                                // the worker drops its receiver only when it ends
                                let _ = tx.send(Work::GtkFlush(props)).await;
                            },
                            "cupertino-gtk-flush",
                        )
                        .detach();
                }
            }
            Work::GtkFlush(props) => {
                unsent.remove(&props);
                if let Some(client) = gtk.get_mut(&props) {
                    out.emit(Event::Layout {
                        source: MenuSource::Gtk(props),
                        root: MenuItem::submenu(0, "", client.items()),
                    });
                }
            }
            // awaited in order: a bar that goes and comes back leaves the name owned
            Work::Request(Request::AppMenuRegistrar(owned)) => {
                app_menu_registrar(&conn, owned).await
            }
            Work::Request(request) => handle(&conn, &out, &tx, &mut gtk, request),
        }
    }
}

/// Load the menus of `props` again, after changes came while they loaded.
fn reload_gtk(
    conn: &Connection,
    out: &Out,
    tx: &async_channel::Sender<Work>,
    props: GtkMenuProps,
    reloads: u8,
) {
    let (conn, out, tx) = (conn.clone(), out.clone(), tx.clone());
    conn.executor()
        .clone()
        .spawn(
            async move {
                match GtkMenuClient::load(&conn, &props).await {
                    Ok((client, items)) => {
                        out.emit(Event::Layout {
                            source: MenuSource::Gtk(props.clone()),
                            root: MenuItem::submenu(0, "", items),
                        });
                        let loaded = Work::GtkLoaded {
                            props,
                            client: Box::new(client),
                            follow: None,
                            reloads,
                        };
                        // the worker drops its receiver only when it ends
                        let _ = tx.send(loaded).await;
                    }
                    Err(e) => report(&format!("cannot load the menu of {}", props.bus_name), e),
                }
            },
            "cupertino-reload-gtk",
        )
        .detach();
}

/// Forward the changes of the exporter of `props` to the worker for as long as the returned task lives.
fn follow_gtk(
    conn: &Connection,
    tx: &async_channel::Sender<Work>,
    props: GtkMenuProps,
    mut changes: BoxStream<'static, GtkChange>,
) -> zbus::Task<()> {
    let tx = tx.clone();
    conn.executor().clone().spawn(
        async move {
            let (applied, done) = async_channel::bounded(1);
            while let Some(change) = changes.next().await {
                let work = Work::GtkChanged(props.clone(), change, applied.clone());
                if tx.send(work).await.is_err() || done.recv().await.is_err() {
                    return;
                }
            }
        },
        "cupertino-follow-gtk",
    )
}

/// Claim the watcher and forward its events, then serve the registrar.
async fn serve(conn: &Connection, tx: &async_channel::Sender<Work>) -> Result<(), StartError> {
    let (_service, events) = WatcherService::start(conn)
        .await
        .map_err(StartError::Watcher)?;
    forward(events, tx.clone(), Work::Watcher);
    if let Err(e) = registrar::serve(conn).await {
        report(&format!("cannot serve {}", registrar::REGISTRAR_NAME), e);
    }
    Ok(())
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

/// Publish a status item, then again every time it changes.
async fn follow_status_item(conn: Connection, out: Out, service: String, path: String) {
    publish_status(&conn, &out, &service, &path).await;
    let mut changes = match sni::item_changes(&conn, &service, &path).await {
        Ok(changes) => changes,
        Err(e) => return report(&format!("cannot follow the status item {service}{path}"), e),
    };
    while changes.next().await.is_some() {
        publish_status(&conn, &out, &service, &path).await;
    }
}

async fn publish_status(conn: &Connection, out: &Out, service: &str, path: &str) {
    match sni::fetch_item(conn, service, path).await {
        Ok(item) => {
            let menu = sni::load_menu(conn, &item).await;
            out.emit(Event::StatusUpdated {
                item: Box::new(item),
                menu,
            });
        }
        Err(e) => report(&format!("status item {service}{path} is not readable"), e),
    }
}

/// The whole tree of a DBusMenu exporter; `about_to_show` first tells it that an item is about
/// to be shown.
async fn fetch_dbusmenu(
    conn: &Connection,
    menu: &KdeAppMenu,
    about_to_show: Option<i32>,
) -> zbus::Result<MenuItem> {
    let client = DBusMenuClient::connect(conn, &menu.service, &menu.path).await?;
    if let Some(item) = about_to_show {
        // many exporters do not implement it and the tree is worth loading anyway
        if let Err(e) = client.about_to_show(ItemId(item)).await {
            report(&format!("AboutToShow on {}{}", menu.service, menu.path), e);
        }
    }
    Ok(client.fetch(ItemId(0), -1).await?.1)
}

fn handle(
    conn: &Connection,
    out: &Out,
    tx: &async_channel::Sender<Work>,
    gtk: &mut HashMap<GtkMenuProps, GtkMenuClient>,
    request: Request,
) {
    let (conn, out) = (conn.clone(), out.clone());
    let executor = conn.executor().clone();
    let task = match request {
        Request::FetchDbusMenu(menu) => executor.spawn(
            async move {
                match fetch_dbusmenu(&conn, &menu, None).await {
                    Ok(root) => out.emit(Event::Layout {
                        source: MenuSource::Kde(menu),
                        root,
                    }),
                    Err(e) => {
                        report(
                            &format!("cannot load the menu of {}{}", menu.service, menu.path),
                            e,
                        );
                        out.emit(Event::LayoutFailed {
                            source: MenuSource::Kde(menu),
                        });
                    }
                }
            },
            "cupertino-fetch-menu",
        ),
        Request::AboutToShow { menu, item } => executor.spawn(
            async move {
                match fetch_dbusmenu(&conn, &menu, Some(item)).await {
                    Ok(root) => out.emit(Event::Layout {
                        source: MenuSource::Kde(menu),
                        root,
                    }),
                    Err(e) => report(
                        &format!("cannot reload the menu of {}{}", menu.service, menu.path),
                        e,
                    ),
                }
            },
            "cupertino-about-to-show",
        ),
        Request::FetchGtk(props) => {
            let tx = tx.clone();
            executor.spawn(
                async move {
                    // followed before the load and read from the start: a full signal queue would stop the connection,
                    // and with it the load's own reply
                    let follow = match GtkMenuClient::changes(&conn, &props).await {
                        Ok(changes) => Some(follow_gtk(&conn, &tx, props.clone(), changes)),
                        Err(e) => {
                            report(&format!("cannot follow the menus of {}", props.bus_name), e);
                            None
                        }
                    };
                    let mut wait = GTK_RETRY_FIRST;
                    let loaded = loop {
                        match GtkMenuClient::load(&conn, &props).await {
                            Err(e) if is_access_denied(&e) && wait <= GTK_RETRY_LAST => {
                                async_io::Timer::after(wait).await;
                                wait *= 2;
                            }
                            loaded => break loaded,
                        }
                    };
                    match loaded {
                        Ok((client, items)) => {
                            out.emit(Event::Layout {
                                source: MenuSource::Gtk(props.clone()),
                                root: MenuItem::submenu(0, "", items),
                            });
                            // the process drops its end only when it exits
                            let loaded = Work::GtkLoaded {
                                props,
                                client: Box::new(client),
                                follow,
                                reloads: 0,
                            };
                            if tx.send(loaded).await.is_err() {
                                report(
                                    "cannot keep the menu exporter",
                                    "the worker is shutting down",
                                );
                            }
                        }
                        Err(e) => {
                            report(&format!("cannot load the menu of {}", props.bus_name), e);
                            out.emit(Event::LayoutFailed {
                                source: MenuSource::Gtk(props),
                            });
                        }
                    }
                },
                "cupertino-fetch-gtk",
            )
        }
        Request::ActivateDbusMenu { menu, item } => executor.spawn(
            async move {
                let result = match DBusMenuClient::connect(&conn, &menu.service, &menu.path).await {
                    Ok(client) => client.activate(ItemId(item), 0).await,
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
        Request::ActivateGtk { props, item } => {
            // the client stays: its changes keep coming while the activation runs
            let Some(activation) = gtk
                .get(&props)
                .and_then(|client| client.activation(ItemId(item)))
            else {
                return report(
                    &format!("cannot activate item {item} of {}", props.bus_name),
                    "the menu is not loaded or the item has no action",
                );
            };
            executor.spawn(
                async move {
                    if let Err(e) = activation.run().await {
                        report(
                            &format!("cannot activate item {item} of {}", props.bus_name),
                            e,
                        );
                    }
                },
                "cupertino-activate-gtk",
            )
        }
        Request::ActivateStatus {
            service,
            path,
            x,
            y,
        } => executor.spawn(
            async move {
                let item = StatusItem {
                    service,
                    path,
                    ..StatusItem::default()
                };
                if let Err(e) = sni::activate_item(&conn, &item, x, y).await {
                    report(
                        &format!(
                            "cannot activate the status item {}{}",
                            item.service, item.path
                        ),
                        e,
                    );
                }
            },
            "cupertino-status-activate",
        ),
        Request::ToggleCentre => executor.spawn(
            async move {
                if let Err(e) = conn
                    .call_method(
                        Some(CENTRE_NAME),
                        CENTRE_PATH,
                        Some(CENTRE_NAME),
                        "ToggleCentre",
                        &(),
                    )
                    .await
                {
                    report("cannot toggle the notification centre", e);
                }
            },
            "cupertino-toggle-centre",
        ),
        Request::AppMenuRegistrar(_) => unreachable!("the worker's loop handles the registrar"),
        Request::TogglePanel => executor.spawn(
            async move {
                if let Err(e) = conn
                    .call_method(
                        Some(PANEL_NAME),
                        PANEL_PATH,
                        Some(PANEL_NAME),
                        "Toggle",
                        &(),
                    )
                    .await
                {
                    report("cannot toggle the control panel", e);
                }
            },
            "cupertino-toggle-panel",
        ),
    };
    task.detach();
}
