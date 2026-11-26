//! The status notifier watcher this component serves, and a client of one another process serves.
//!
//! An item registers with a bus name (its service, the item at [`DEFAULT_ITEM_PATH`]), a `service/path` string, or a
//! bare object path (the caller's own unique name and that path). An item is kept until its service or the connection
//! that registered it loses its name: the connection leaves the bus, or the well-known name the item gave passes to
//! another owner; an item registered again by the owner of its service is that owner's from then on, so one another
//! connection registered first under a name it does not own stays while the owner does. A registration is announced (the watcher's signals, [`WatcherEvent::ItemRegistered`]) only once its
//! service is known to be on the bus, so one that left while it registered is never shown.

use std::sync::{Arc, Mutex, PoisonError};

use futures_util::{Stream, StreamExt};
use zbus::message::Header;
use zbus::names::BusName;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::ObjectPath;
use zbus::{fdo, proxy, Connection};

use super::{parse_item_id, FREEDESKTOP_WATCHER_NAME, WATCHER_NAME, WATCHER_PATH};

/// The `ProtocolVersion` the watcher reports: the specification's only version.
const PROTOCOL_VERSION: i32 = 0;
/// Most items one connection may have registered at once; a program registering more is refused, so it cannot fill
/// the bar.
const MAX_ITEMS_PER_CONNECTION: usize = 32;
/// Most items kept in all; past it registrations are refused.
const MAX_ITEMS: usize = 512;
/// Most other hosts kept; past it a host's registration is refused.
const MAX_HOSTS: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WatcherEvent {
    ItemRegistered {
        id: String,
        service: String,
        path: String,
    },
    ItemUnregistered {
        id: String,
    },
    HostRegistered,
}

/// One registered item.
struct Entry {
    /// `service + path`.
    id: String,
    service: String,
    /// The unique name of the connection that registered it.
    sender: String,
    /// Its registration was announced; until then nothing tells of its leaving either.
    announced: bool,
}

/// Another host, by the name it registered and the connection that did.
struct Host {
    name: String,
    sender: String,
}

struct WatcherState {
    items: Vec<Entry>,
    hosts: Vec<Host>,
    tx: async_channel::Sender<WatcherEvent>,
}

/// What a registration found.
enum Added {
    New,
    Known,
    Full,
}

impl WatcherState {
    fn announced(&self) -> Vec<String> {
        self.items
            .iter()
            .filter(|e| e.announced)
            .map(|e| e.id.clone())
            .collect()
    }

    fn add(&mut self, id: &str, service: &str, sender: &str) -> Added {
        if self.items.iter().any(|e| e.id == id) {
            return Added::Known;
        }
        let own = self.items.iter().filter(|e| e.sender == sender).count();
        if own >= MAX_ITEMS_PER_CONNECTION || self.items.len() >= MAX_ITEMS {
            return Added::Full;
        }
        self.items.push(Entry {
            id: id.to_owned(),
            service: service.to_owned(),
            sender: sender.to_owned(),
            announced: false,
        });
        Added::New
    }

    /// The item `id`, registered by another connection first, is registered again by `owner`, the owner of its
    /// service: it is the owner's from then on, so an item registered under a name its registrar does not own leaves
    /// with the name's owner, not with the registrar.
    fn adopt(&mut self, id: &str, owner: &str) {
        if let Some(entry) = self.items.iter_mut().find(|e| e.id == id) {
            entry.sender = owner.to_owned();
        }
    }

    /// Announce the item `id` when it is still kept and was not yet; whether it was.
    fn announce(&mut self, id: &str, path: &str) -> bool {
        let Some(entry) = self.items.iter_mut().find(|e| e.id == id && !e.announced) else {
            return false;
        };
        entry.announced = true;
        let _ = self.tx.try_send(WatcherEvent::ItemRegistered {
            id: id.to_owned(),
            service: entry.service.clone(),
            path: path.to_owned(),
        });
        true
    }

    /// Drop the items and hosts of `name`, which lost its owner: the ids of the announced items dropped, and whether
    /// a host went.
    fn name_lost(&mut self, name: &str) -> (Vec<String>, bool) {
        let mut gone = Vec::new();
        self.items.retain(|e| {
            let lost = e.service == name || e.sender == name;
            if lost && e.announced {
                gone.push(e.id.clone());
            }
            !lost
        });
        for id in &gone {
            let _ = self
                .tx
                .try_send(WatcherEvent::ItemUnregistered { id: id.clone() });
        }
        let hosts = self.hosts.len();
        self.hosts.retain(|h| h.name != name && h.sender != name);
        (gone, self.hosts.len() != hosts)
    }
}

#[derive(Clone)]
struct Shared(Arc<Mutex<WatcherState>>);

impl Shared {
    fn lock(&self) -> std::sync::MutexGuard<'_, WatcherState> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// A change the watcher's signals tell of.
enum Announce {
    ItemRegistered(String),
    ItemUnregistered(String),
    HostRegistered,
    HostUnregistered,
}

/// The watcher's interface under the name `$name`, sharing one state with the other.
macro_rules! watcher_interface {
    ($ty:ident, $name:tt) => {
        struct $ty(Shared);

        #[zbus::interface(name = $name)]
        impl $ty {
            async fn register_status_notifier_item(
                &self,
                service: &str,
                #[zbus(header)] header: Header<'_>,
                #[zbus(connection)] conn: &Connection,
            ) -> fdo::Result<()> {
                let sender = header.sender().map(|s| s.as_str());
                register_item(&self.0, conn, service, sender).await
            }

            async fn register_status_notifier_host(
                &self,
                service: &str,
                #[zbus(header)] header: Header<'_>,
                #[zbus(connection)] conn: &Connection,
            ) -> fdo::Result<()> {
                let sender = header.sender().map(|s| s.as_str());
                register_host(&self.0, conn, service, sender).await
            }

            #[zbus(property)]
            fn registered_status_notifier_items(&self) -> Vec<String> {
                self.0.lock().announced()
            }

            /// The component is the host.
            #[zbus(property)]
            fn is_status_notifier_host_registered(&self) -> bool {
                true
            }

            #[zbus(property)]
            fn protocol_version(&self) -> i32 {
                PROTOCOL_VERSION
            }

            #[zbus(signal)]
            async fn status_notifier_item_registered(
                emitter: &SignalEmitter<'_>,
                service: &str,
            ) -> zbus::Result<()>;

            #[zbus(signal)]
            async fn status_notifier_item_unregistered(
                emitter: &SignalEmitter<'_>,
                service: &str,
            ) -> zbus::Result<()>;

            #[zbus(signal)]
            async fn status_notifier_host_registered(
                emitter: &SignalEmitter<'_>,
            ) -> zbus::Result<()>;

            #[zbus(signal)]
            async fn status_notifier_host_unregistered(
                emitter: &SignalEmitter<'_>,
            ) -> zbus::Result<()>;
        }

        impl $ty {
            /// Tell of `changes` under this interface's name.
            async fn announce(conn: &Connection, changes: &[Announce]) -> zbus::Result<()> {
                let emitter = SignalEmitter::new(conn, WATCHER_PATH)?;
                let mut items = false;
                for change in changes {
                    match change {
                        Announce::ItemRegistered(id) => {
                            items = true;
                            Self::status_notifier_item_registered(&emitter, id).await?
                        }
                        Announce::ItemUnregistered(id) => {
                            items = true;
                            Self::status_notifier_item_unregistered(&emitter, id).await?
                        }
                        Announce::HostRegistered => {
                            Self::status_notifier_host_registered(&emitter).await?
                        }
                        Announce::HostUnregistered => {
                            Self::status_notifier_host_unregistered(&emitter).await?
                        }
                    }
                }
                if items {
                    let iface = conn
                        .object_server()
                        .interface::<_, $ty>(WATCHER_PATH)
                        .await?;
                    iface
                        .get()
                        .await
                        .registered_status_notifier_items_changed(&emitter)
                        .await?;
                }
                Ok(())
            }
        }
    };
}

watcher_interface!(KdeWatcher, "org.kde.StatusNotifierWatcher");
watcher_interface!(FreedesktopWatcher, "org.freedesktop.StatusNotifierWatcher");

/// Tell of `changes` under both interface names.
async fn announce(conn: &Connection, changes: &[Announce]) {
    if changes.is_empty() {
        return;
    }
    for result in [
        KdeWatcher::announce(conn, changes).await,
        FreedesktopWatcher::announce(conn, changes).await,
    ] {
        if let Err(e) = result {
            crate::worker::report("cannot signal a change of the status item watcher", e);
        }
    }
}

/// The unique name of the owner of `name` (itself, for a unique name on the bus); `None` when it has none or the bus
/// does not answer, so nothing is shown that may be gone.
async fn name_owner(conn: &Connection, name: &str) -> Option<String> {
    let name = BusName::try_from(name).ok()?;
    let bus = fdo::DBusProxy::new(conn).await.ok()?;
    let owner = bus.get_name_owner(name).await.ok()?;
    Some(owner.to_string())
}

/// `name` lost its owner: drop its items and hosts, and tell of the items that were announced, and of the hosts when
/// `hosts` (those of a host whose registration was not told of are not).
async fn lost(shared: &Shared, conn: &Connection, name: &str, hosts: bool) {
    let (items, hosts_went) = shared.lock().name_lost(name);
    let mut changes: Vec<Announce> = items.into_iter().map(Announce::ItemUnregistered).collect();
    if hosts && hosts_went {
        changes.push(Announce::HostUnregistered);
    }
    announce(conn, &changes).await;
}

async fn register_item(
    shared: &Shared,
    conn: &Connection,
    registered: &str,
    sender: Option<&str>,
) -> fdo::Result<()> {
    let sender = sender.ok_or_else(|| fdo::Error::InvalidArgs("the call has no sender".into()))?;
    let invalid = || fdo::Error::InvalidArgs(format!("`{registered}` names no status item"));
    let (service, path) = parse_item_id(registered, Some(sender)).ok_or_else(invalid)?;
    BusName::try_from(service.as_str()).map_err(|_| invalid())?;
    ObjectPath::try_from(path.as_str()).map_err(|_| invalid())?;
    let id = format!("{service}{path}");
    let known = match shared.lock().add(&id, &service, sender) {
        Added::Known => true,
        Added::Full => {
            return Err(fdo::Error::LimitsExceeded(format!(
                "too many status items to register `{registered}`"
            )))
        }
        Added::New => false,
    };
    // kept before the bus is asked, so a leaving told of meanwhile drops it; announced only once it is known to stay
    let owner = name_owner(conn, &service).await;
    if known {
        // the owner of the service registering an item another connection registered first takes it over
        if owner.as_deref() == Some(sender) {
            shared.lock().adopt(&id, sender);
        }
        return Ok(());
    }
    if owner.is_none() {
        lost(shared, conn, &service, true).await;
        return Err(fdo::Error::NameHasNoOwner(format!(
            "`{service}` is not on the bus"
        )));
    }
    if shared.lock().announce(&id, &path) {
        announce(conn, &[Announce::ItemRegistered(id)]).await;
    }
    Ok(())
}

async fn register_host(
    shared: &Shared,
    conn: &Connection,
    name: &str,
    sender: Option<&str>,
) -> fdo::Result<()> {
    let sender = sender.ok_or_else(|| fdo::Error::InvalidArgs("the call has no sender".into()))?;
    BusName::try_from(name)
        .map_err(|_| fdo::Error::InvalidArgs(format!("`{name}` is not a bus name")))?;
    {
        let mut state = shared.lock();
        if state.hosts.iter().any(|h| h.name == name) {
            return Ok(());
        }
        if state.hosts.len() >= MAX_HOSTS {
            return Err(fdo::Error::LimitsExceeded("too many hosts".into()));
        }
        state.hosts.push(Host {
            name: name.to_owned(),
            sender: sender.to_owned(),
        });
    }
    if name_owner(conn, sender).await.is_none() {
        // the caller left: its items go, told of, and the host whose registration was never told of goes silently
        lost(shared, conn, sender, false).await;
        return Ok(());
    }
    announce(conn, &[Announce::HostRegistered]).await;
    Ok(())
}

#[derive(Clone)]
pub struct WatcherService {
    shared: Shared,
}

impl WatcherService {
    /// Serve the watcher on `conn` and claim [`WATCHER_NAME`], then [`FREEDESKTOP_WATCHER_NAME`] when no other process
    /// owns it; the task that drops the items of a name that lost its owner is one of `spawner`'s.
    pub async fn start(
        conn: &Connection,
        spawner: &crate::spawned::Spawner,
    ) -> zbus::Result<(WatcherService, async_channel::Receiver<WatcherEvent>)> {
        let (tx, rx) = async_channel::unbounded();
        let shared = Shared(Arc::new(Mutex::new(WatcherState {
            items: Vec::new(),
            hosts: Vec::new(),
            tx,
        })));
        // subscribed before the object is served, so no item that registered can leave unseen
        let mut owner_changes = fdo::DBusProxy::new(conn)
            .await?
            .receive_name_owner_changed()
            .await?;
        conn.object_server()
            .at(WATCHER_PATH, KdeWatcher(shared.clone()))
            .await?;
        conn.object_server()
            .at(WATCHER_PATH, FreedesktopWatcher(shared.clone()))
            .await?;
        super::super::claim_name(conn, WATCHER_NAME).await?;
        if let Err(e) = super::super::claim_name(conn, FREEDESKTOP_WATCHER_NAME).await {
            crate::worker::report(&format!("cannot own {FREEDESKTOP_WATCHER_NAME}"), e);
        }

        let watched = shared.clone();
        let emit_conn = conn.clone();
        spawner.spawn(
            async move {
                while let Some(sig) = owner_changes.next().await {
                    let Ok(args) = sig.args() else { continue };
                    // a name that had an owner and lost it: its connection left, or it passed to another
                    if args.old_owner().is_none() {
                        continue;
                    }
                    lost(&watched, &emit_conn, args.name().as_str(), true).await;
                }
            },
            "menubar-sni-watch",
        );
        Ok((WatcherService { shared }, rx))
    }

    /// The items announced, as registered item strings.
    pub fn items(&self) -> Vec<String> {
        self.shared.lock().announced()
    }
}

#[proxy(
    gen_blocking = false,
    interface = "org.kde.StatusNotifierWatcher",
    default_service = "org.kde.StatusNotifierWatcher",
    default_path = "/StatusNotifierWatcher"
)]
pub trait Watcher {
    fn register_status_notifier_item(&self, service: &str) -> zbus::Result<()>;

    fn register_status_notifier_host(&self, service: &str) -> zbus::Result<()>;

    #[zbus(property)]
    fn registered_status_notifier_items(&self) -> zbus::Result<Vec<String>>;

    #[zbus(property)]
    fn is_status_notifier_host_registered(&self) -> zbus::Result<bool>;

    #[zbus(property)]
    fn protocol_version(&self) -> zbus::Result<i32>;

    #[zbus(signal)]
    fn status_notifier_item_registered(&self, service: String) -> zbus::Result<()>;

    #[zbus(signal)]
    fn status_notifier_item_unregistered(&self, service: String) -> zbus::Result<()>;

    #[zbus(signal)]
    fn status_notifier_host_registered(&self) -> zbus::Result<()>;

    #[zbus(signal)]
    fn status_notifier_host_unregistered(&self) -> zbus::Result<()>;
}

/// Host attached to a watcher owned by another process.
pub struct WatcherClient {
    proxy: WatcherProxy<'static>,
}

impl WatcherClient {
    pub async fn connect(conn: &Connection) -> zbus::Result<Self> {
        Ok(Self {
            proxy: WatcherProxy::builder(conn)
                .cache_properties(zbus::proxy::CacheProperties::No)
                .build()
                .await?,
        })
    }

    pub async fn items(&self) -> zbus::Result<Vec<String>> {
        self.proxy.registered_status_notifier_items().await
    }

    pub async fn register_host(&self, name: &str) -> zbus::Result<()> {
        self.proxy.register_status_notifier_host(name).await
    }

    /// Item side: announce an item (`service`, `service/path` or a bare object path).
    pub async fn register_item(&self, service: &str) -> zbus::Result<()> {
        self.proxy.register_status_notifier_item(service).await
    }

    pub async fn is_host_registered(&self) -> zbus::Result<bool> {
        self.proxy.is_status_notifier_host_registered().await
    }

    pub fn proxy(&self) -> &WatcherProxy<'static> {
        &self.proxy
    }

    pub async fn events(&self) -> zbus::Result<impl Stream<Item = WatcherEvent> + Unpin> {
        let reg = self
            .proxy
            .receive_status_notifier_item_registered()
            .await?
            .filter_map(|s| async move {
                let id = s.args().ok()?.service;
                let (service, path) = parse_item_id(&id, None)?;
                Some(WatcherEvent::ItemRegistered { id, service, path })
            });
        let unreg = self
            .proxy
            .receive_status_notifier_item_unregistered()
            .await?
            .filter_map(|s| async move {
                Some(WatcherEvent::ItemUnregistered {
                    id: s.args().ok()?.service,
                })
            });
        Ok(futures_util::stream::select(reg, unreg).boxed())
    }
}
