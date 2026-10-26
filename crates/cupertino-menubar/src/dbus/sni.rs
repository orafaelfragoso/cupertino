//! StatusNotifier support (the open freedesktop / KDE protocol for tray icons).
//!
//! * [`WatcherService`] serves `org.kde.StatusNotifierWatcher` (the compositor is also the host, so
//!   `IsStatusNotifierHostRegistered` is true), tracks registered items, drops items whose
//!   connection left the bus and reports changes on a channel.
//! * [`fetch_item`] reads an `org.kde.StatusNotifierItem` into a [`StatusItem`] snapshot, including
//!   icon pixmaps; [`load_menu`] fetches its DBusMenu; the `*_item` functions send `Activate`,
//!   `SecondaryActivate`, `ContextMenu` and `Scroll`.
//! * [`WatcherClient`] attaches to an existing watcher instead (when another process owns the name).
//!
//! Registered item strings are `service + path` (for example `:1.52/StatusNotifierItem`); a bare
//! service name means the default path `/StatusNotifierItem`, a bare path means the sender.

use std::sync::{mpsc, Arc, Mutex};

use futures_util::{Stream, StreamExt};
use zbus::message::Header;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::OwnedObjectPath;
use zbus::{fdo, proxy, Connection};

use crate::item::MenuItem;
use crate::layout::StatusEntry;

pub const WATCHER_NAME: &str = "org.kde.StatusNotifierWatcher";
pub const WATCHER_PATH: &str = "/StatusNotifierWatcher";
pub const DEFAULT_ITEM_PATH: &str = "/StatusNotifierItem";

/// Split a registered item string into `(service, path)`.
pub fn parse_item_id(registered: &str, sender: Option<&str>) -> Option<(String, String)> {
    if registered.starts_with('/') {
        return sender.map(|s| (s.to_string(), registered.to_string()));
    }
    if registered.is_empty() {
        return None;
    }
    match registered.find('/') {
        Some(i) => Some((registered[..i].to_string(), registered[i..].to_string())),
        None => Some((registered.to_string(), DEFAULT_ITEM_PATH.to_string())),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WatcherEvent {
    ItemRegistered { id: String, service: String, path: String },
    ItemUnregistered { id: String },
    HostRegistered,
}

struct WatcherState {
    items: Vec<String>,
    hosts: Vec<String>,
    tx: mpsc::Sender<WatcherEvent>,
}

struct WatcherObject {
    shared: Arc<Mutex<WatcherState>>,
}

#[zbus::interface(name = "org.kde.StatusNotifierWatcher")]
impl WatcherObject {
    async fn register_status_notifier_item(
        &self,
        service: &str,
        #[zbus(header)] header: Header<'_>,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> fdo::Result<()> {
        let sender = header.sender().map(|s| s.to_string());
        let (svc, path) = parse_item_id(service, sender.as_deref()).ok_or_else(|| fdo::Error::InvalidArgs(format!("bad item `{service}`")))?;
        let id = format!("{svc}{path}");
        let added = {
            let mut s = self.shared.lock().expect("watcher state");
            if s.items.contains(&id) {
                false
            } else {
                s.items.push(id.clone());
                let _ = s.tx.send(WatcherEvent::ItemRegistered { id: id.clone(), service: svc, path });
                true
            }
        };
        if added {
            Self::status_notifier_item_registered(&emitter, &id).await?;
            self.registered_status_notifier_items_changed(&emitter).await?;
        }
        Ok(())
    }

    async fn register_status_notifier_host(&self, service: &str, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>) -> fdo::Result<()> {
        {
            let mut s = self.shared.lock().expect("watcher state");
            if !s.hosts.iter().any(|h| h == service) {
                s.hosts.push(service.to_string());
            }
        }
        Self::status_notifier_host_registered(&emitter).await?;
        Ok(())
    }

    #[zbus(property)]
    fn registered_status_notifier_items(&self) -> Vec<String> {
        self.shared.lock().expect("watcher state").items.clone()
    }

    #[zbus(property)]
    fn is_status_notifier_host_registered(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn protocol_version(&self) -> i32 {
        0
    }

    #[zbus(signal)]
    async fn status_notifier_item_registered(emitter: &SignalEmitter<'_>, service: &str) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn status_notifier_item_unregistered(emitter: &SignalEmitter<'_>, service: &str) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn status_notifier_host_registered(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
}

#[derive(Clone)]
pub struct WatcherService {
    shared: Arc<Mutex<WatcherState>>,
}

impl WatcherService {
    /// Serve the watcher on `conn` and claim `org.kde.StatusNotifierWatcher`.
    pub async fn start(conn: &Connection) -> zbus::Result<(WatcherService, mpsc::Receiver<WatcherEvent>)> {
        let (tx, rx) = mpsc::channel();
        let shared = Arc::new(Mutex::new(WatcherState { items: Vec::new(), hosts: Vec::new(), tx }));
        conn.object_server().at(WATCHER_PATH, WatcherObject { shared: shared.clone() }).await?;
        super::claim_name(conn, WATCHER_NAME).await?;

        let dbus = fdo::DBusProxy::new(conn).await?;
        let mut owner_changes = dbus.receive_name_owner_changed().await?;
        let watched = shared.clone();
        let emit_conn = conn.clone();
        conn.executor()
            .spawn(
                async move {
                    while let Some(sig) = owner_changes.next().await {
                        let Ok(args) = sig.args() else { continue };
                        if args.new_owner().is_some() {
                            continue;
                        }
                        let gone = args.name().to_string();
                        let removed: Vec<String> = {
                            let mut s = watched.lock().expect("watcher state");
                            let removed: Vec<String> = s.items.iter().filter(|i| i.starts_with(&format!("{gone}/"))).cloned().collect();
                            s.items.retain(|i| !removed.contains(i));
                            for id in &removed {
                                let _ = s.tx.send(WatcherEvent::ItemUnregistered { id: id.clone() });
                            }
                            removed
                        };
                        if removed.is_empty() {
                            continue;
                        }
                        if let Ok(emitter) = SignalEmitter::new(&emit_conn, WATCHER_PATH) {
                            for id in removed {
                                let _ = WatcherObject::status_notifier_item_unregistered(&emitter, &id).await;
                            }
                            if let Ok(iface) = emit_conn.object_server().interface::<_, WatcherObject>(WATCHER_PATH).await {
                                let _ = iface.get().await.registered_status_notifier_items_changed(&emitter).await;
                            }
                        }
                    }
                },
                "menubar-sni-watch",
            )
            .detach();
        Ok((WatcherService { shared }, rx))
    }

    pub fn items(&self) -> Vec<String> {
        self.shared.lock().expect("watcher state").items.clone()
    }
}

/// `(icon name, icon pixmaps, title, description)` of a status item tooltip.
pub type ToolTip = (String, Vec<(i32, i32, Vec<u8>)>, String, String);

#[proxy(interface = "org.kde.StatusNotifierWatcher", default_service = "org.kde.StatusNotifierWatcher", default_path = "/StatusNotifierWatcher")]
pub trait Watcher {
    fn register_status_notifier_item(&self, service: &str) -> zbus::Result<()>;

    fn register_status_notifier_host(&self, service: &str) -> zbus::Result<()>;

    #[zbus(property)]
    fn registered_status_notifier_items(&self) -> zbus::Result<Vec<String>>;

    #[zbus(property)]
    fn is_status_notifier_host_registered(&self) -> zbus::Result<bool>;

    #[zbus(signal)]
    fn status_notifier_item_registered(&self, service: String) -> zbus::Result<()>;

    #[zbus(signal)]
    fn status_notifier_item_unregistered(&self, service: String) -> zbus::Result<()>;
}

/// Host attached to a watcher owned by another process.
pub struct WatcherClient {
    proxy: WatcherProxy<'static>,
}

impl WatcherClient {
    pub async fn connect(conn: &Connection) -> zbus::Result<Self> {
        Ok(Self { proxy: WatcherProxy::builder(conn).cache_properties(zbus::proxy::CacheProperties::No).build().await? })
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
        let reg = self.proxy.receive_status_notifier_item_registered().await?.filter_map(|s| async move {
            let id = s.args().ok()?.service;
            let (service, path) = parse_item_id(&id, None)?;
            Some(WatcherEvent::ItemRegistered { id, service, path })
        });
        let unreg = self
            .proxy
            .receive_status_notifier_item_unregistered()
            .await?
            .filter_map(|s| async move { Some(WatcherEvent::ItemUnregistered { id: s.args().ok()?.service }) });
        Ok(futures_util::stream::select(reg, unreg).boxed())
    }
}

#[proxy(interface = "org.kde.StatusNotifierItem", assume_defaults = false)]
pub trait StatusNotifierItem {
    fn activate(&self, x: i32, y: i32) -> zbus::Result<()>;

    fn secondary_activate(&self, x: i32, y: i32) -> zbus::Result<()>;

    fn context_menu(&self, x: i32, y: i32) -> zbus::Result<()>;

    fn scroll(&self, delta: i32, orientation: &str) -> zbus::Result<()>;

    #[zbus(property)]
    fn category(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn id(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn title(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn status(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn icon_name(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn icon_pixmap(&self) -> zbus::Result<Vec<(i32, i32, Vec<u8>)>>;

    #[zbus(property)]
    fn overlay_icon_name(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn attention_icon_name(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn attention_icon_pixmap(&self) -> zbus::Result<Vec<(i32, i32, Vec<u8>)>>;

    #[zbus(property)]
    fn icon_theme_path(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn tool_tip(&self) -> zbus::Result<ToolTip>;

    #[zbus(property)]
    fn item_is_menu(&self) -> zbus::Result<bool>;

    #[zbus(property)]
    fn menu(&self) -> zbus::Result<OwnedObjectPath>;

    #[zbus(signal)]
    fn new_title(&self) -> zbus::Result<()>;

    #[zbus(signal)]
    fn new_icon(&self) -> zbus::Result<()>;

    #[zbus(signal)]
    fn new_attention_icon(&self) -> zbus::Result<()>;

    #[zbus(signal)]
    fn new_overlay_icon(&self) -> zbus::Result<()>;

    #[zbus(signal)]
    fn new_tool_tip(&self) -> zbus::Result<()>;

    #[zbus(signal)]
    fn new_status(&self, status: String) -> zbus::Result<()>;
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ItemStatus {
    Passive,
    #[default]
    Active,
    NeedsAttention,
}

impl ItemStatus {
    pub fn parse(s: &str) -> ItemStatus {
        match s {
            "Passive" => ItemStatus::Passive,
            "NeedsAttention" => ItemStatus::NeedsAttention,
            _ => ItemStatus::Active,
        }
    }
}

/// An ARGB32 icon as sent by items: network byte order, bytes `A R G B` per pixel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IconPixmap {
    pub width: i32,
    pub height: i32,
    pub argb: Vec<u8>,
}

impl IconPixmap {
    pub fn is_valid(&self) -> bool {
        self.width > 0 && self.height > 0 && self.argb.len() == (self.width as usize) * (self.height as usize) * 4
    }

    /// Convert to straight RGBA8; `None` for a malformed pixmap.
    pub fn to_rgba(&self) -> Option<Vec<u8>> {
        if !self.is_valid() {
            return None;
        }
        Some(self.argb.chunks(4).flat_map(|p| [p[1], p[2], p[3], p[0]]).collect())
    }
}

/// The pixmap to draw at `target` pixels: the smallest valid one that is at least as large, else
/// the largest valid one.
pub fn best_pixmap(pixmaps: &[IconPixmap], target: i32) -> Option<&IconPixmap> {
    let valid = || pixmaps.iter().filter(|p| p.is_valid());
    valid().filter(|p| p.width >= target && p.height >= target).min_by_key(|p| p.width * p.height).or_else(|| valid().max_by_key(|p| p.width * p.height))
}

fn pixmaps(raw: Vec<(i32, i32, Vec<u8>)>) -> Vec<IconPixmap> {
    raw.into_iter().map(|(width, height, argb)| IconPixmap { width, height, argb }).collect()
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StatusItem {
    /// `service + path`, unique per item.
    pub id: String,
    pub service: String,
    pub path: String,
    pub app_id: String,
    pub category: String,
    pub title: String,
    pub status: ItemStatus,
    pub icon_name: String,
    pub icon_pixmaps: Vec<IconPixmap>,
    pub attention_icon_name: String,
    pub attention_pixmaps: Vec<IconPixmap>,
    pub overlay_icon_name: String,
    pub icon_theme_path: String,
    pub tooltip_title: String,
    /// Left click should open the menu instead of calling `Activate`.
    pub item_is_menu: bool,
    /// Object path of the item's DBusMenu, if it has one.
    pub menu_path: Option<String>,
}

impl StatusItem {
    /// The icon name to show for the current status.
    pub fn current_icon_name(&self) -> &str {
        if self.status == ItemStatus::NeedsAttention && !self.attention_icon_name.is_empty() { &self.attention_icon_name } else { &self.icon_name }
    }

    pub fn current_pixmaps(&self) -> &[IconPixmap] {
        if self.status == ItemStatus::NeedsAttention && !self.attention_pixmaps.is_empty() { &self.attention_pixmaps } else { &self.icon_pixmaps }
    }

    /// Passive items are normally tucked away by hosts.
    pub fn is_passive(&self) -> bool {
        self.status == ItemStatus::Passive
    }

    /// What a primary click should do.
    pub fn primary_action(&self) -> PrimaryAction {
        if self.menu_path.is_some() && self.item_is_menu {
            PrimaryAction::ShowMenu
        } else {
            PrimaryAction::Activate
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrimaryAction {
    Activate,
    ShowMenu,
}

async fn item_proxy(conn: &Connection, service: &str, path: &str) -> zbus::Result<StatusNotifierItemProxy<'static>> {
    StatusNotifierItemProxy::builder(conn)
        .destination(service.to_string())?
        .path(path.to_string())?
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()
        .await
}

/// Read an item. Missing properties keep their defaults (items implement different subsets).
pub async fn fetch_item(conn: &Connection, service: &str, path: &str) -> zbus::Result<StatusItem> {
    let p = item_proxy(conn, service, path).await?;
    // The Id property doubles as the existence check: an item without it is not an item.
    let app_id = p.id().await?;
    let tooltip = p.tool_tip().await.ok().map(|t| t.2).unwrap_or_default();
    let menu_path = p.menu().await.ok().map(|m| m.to_string()).filter(|m| m != "/" && !m.is_empty());
    Ok(StatusItem {
        id: format!("{service}{path}"),
        service: service.to_string(),
        path: path.to_string(),
        app_id,
        category: p.category().await.unwrap_or_default(),
        title: p.title().await.unwrap_or_default(),
        status: ItemStatus::parse(&p.status().await.unwrap_or_default()),
        icon_name: p.icon_name().await.unwrap_or_default(),
        icon_pixmaps: pixmaps(p.icon_pixmap().await.unwrap_or_default()),
        attention_icon_name: p.attention_icon_name().await.unwrap_or_default(),
        attention_pixmaps: pixmaps(p.attention_icon_pixmap().await.unwrap_or_default()),
        overlay_icon_name: p.overlay_icon_name().await.unwrap_or_default(),
        icon_theme_path: p.icon_theme_path().await.unwrap_or_default(),
        tooltip_title: tooltip,
        item_is_menu: p.item_is_menu().await.unwrap_or(false),
        menu_path,
    })
}

/// Fetch the DBusMenu of an item (the whole tree).
pub async fn load_menu(conn: &Connection, item: &StatusItem) -> Option<MenuItem> {
    let path = item.menu_path.as_deref()?;
    let client = super::dbusmenu::DBusMenuClient::connect(conn, &item.service, path).await.ok()?;
    let _ = client.about_to_show(crate::item::ItemId(0)).await;
    client.fetch(crate::item::ItemId(0), -1).await.ok().map(|(_, m)| m)
}

/// The bar entry for an item: `width` is the icon width in points.
pub fn status_entry(item: &StatusItem, menu: Option<MenuItem>, width: f64) -> StatusEntry {
    StatusEntry { id: item.id.clone(), width, menu }
}

pub async fn activate_item(conn: &Connection, item: &StatusItem, x: i32, y: i32) -> zbus::Result<()> {
    item_proxy(conn, &item.service, &item.path).await?.activate(x, y).await
}

pub async fn secondary_activate_item(conn: &Connection, item: &StatusItem, x: i32, y: i32) -> zbus::Result<()> {
    item_proxy(conn, &item.service, &item.path).await?.secondary_activate(x, y).await
}

/// Ask the item to show its own context menu (for items without a DBusMenu).
pub async fn context_menu_item(conn: &Connection, item: &StatusItem, x: i32, y: i32) -> zbus::Result<()> {
    item_proxy(conn, &item.service, &item.path).await?.context_menu(x, y).await
}

/// `vertical` or `horizontal`.
pub async fn scroll_item(conn: &Connection, item: &StatusItem, delta: i32, orientation: &str) -> zbus::Result<()> {
    item_proxy(conn, &item.service, &item.path).await?.scroll(delta, orientation).await
}

/// Change notifications of one item (`NewIcon`, `NewTitle`, `NewStatus`, ...): refetch with
/// [`fetch_item`] when one arrives.
pub async fn item_changes(conn: &Connection, service: &str, path: &str) -> zbus::Result<impl Stream<Item = ()> + Unpin> {
    let p = item_proxy(conn, service, path).await?;
    let a = p.receive_new_icon().await?.map(|_| ());
    let b = p.receive_new_title().await?.map(|_| ());
    let c = p.receive_new_status().await?.map(|_| ());
    let d = p.receive_new_attention_icon().await?.map(|_| ());
    let e = p.receive_new_tool_tip().await?.map(|_| ());
    let s = futures_util::stream::select(futures_util::stream::select(a, b), futures_util::stream::select(c, futures_util::stream::select(d, e)));
    Ok(s.boxed())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn item_ids_are_split_like_the_watcher_does() {
        assert_eq!(parse_item_id(":1.52/StatusNotifierItem", None), Some((":1.52".into(), "/StatusNotifierItem".into())));
        assert_eq!(parse_item_id("org.kde.StatusNotifierItem-4-1", None), Some(("org.kde.StatusNotifierItem-4-1".into(), "/StatusNotifierItem".into())));
        assert_eq!(parse_item_id("/org/ayatana/item", Some(":1.7")), Some((":1.7".into(), "/org/ayatana/item".into())));
        assert_eq!(parse_item_id("/org/ayatana/item", None), None);
        assert_eq!(parse_item_id("", Some(":1.7")), None);
        assert_eq!(parse_item_id(":1.9/a/b/c", None), Some((":1.9".into(), "/a/b/c".into())));
    }

    #[test]
    fn pixmaps_convert_and_are_picked_by_size() {
        let px = |w: i32, h: i32| IconPixmap { width: w, height: h, argb: (0..(w * h * 4)).map(|i| i as u8).collect() };
        let rgba = IconPixmap { width: 1, height: 1, argb: vec![255, 10, 20, 30] }.to_rgba().unwrap();
        assert_eq!(rgba, vec![10, 20, 30, 255]);
        assert!(IconPixmap { width: 2, height: 2, argb: vec![0; 3] }.to_rgba().is_none());
        assert!(IconPixmap { width: 0, height: 0, argb: vec![] }.to_rgba().is_none());
        let list = vec![px(16, 16), px(32, 32), px(64, 64), IconPixmap { width: 128, height: 128, argb: vec![] }];
        assert_eq!(best_pixmap(&list, 24).unwrap().width, 32);
        assert_eq!(best_pixmap(&list, 16).unwrap().width, 16);
        assert_eq!(best_pixmap(&list, 200).unwrap().width, 64, "falls back to the largest valid pixmap");
        assert!(best_pixmap(&[], 16).is_none());
    }

    #[test]
    fn status_parsing_and_icon_choice() {
        assert_eq!(ItemStatus::parse("Passive"), ItemStatus::Passive);
        assert_eq!(ItemStatus::parse("NeedsAttention"), ItemStatus::NeedsAttention);
        assert_eq!(ItemStatus::parse("whatever"), ItemStatus::Active);
        let mut item = StatusItem { icon_name: "net".into(), attention_icon_name: "net-alert".into(), ..Default::default() };
        assert_eq!(item.current_icon_name(), "net");
        item.status = ItemStatus::NeedsAttention;
        assert_eq!(item.current_icon_name(), "net-alert");
        item.attention_icon_name.clear();
        assert_eq!(item.current_icon_name(), "net");
        assert!(!item.is_passive());
    }

    #[test]
    fn primary_action_depends_on_menu_hints() {
        let mut item = StatusItem::default();
        assert_eq!(item.primary_action(), PrimaryAction::Activate);
        item.menu_path = Some("/MenuBar".into());
        assert_eq!(item.primary_action(), PrimaryAction::Activate);
        item.item_is_menu = true;
        assert_eq!(item.primary_action(), PrimaryAction::ShowMenu);
        item.menu_path = None;
        assert_eq!(item.primary_action(), PrimaryAction::Activate);
    }
}
