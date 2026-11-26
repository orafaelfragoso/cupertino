//! A status notifier item as the host reads it: its properties in one snapshot, its change signals, and the calls a
//! click or a scroll on it sends.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};

use futures_util::{Stream, StreamExt};
use zbus::message::Type;
use zbus::names::InterfaceName;
use zbus::zvariant::{OwnedValue, Value};
use zbus::{Connection, MatchRule, MessageStream};

use crate::dbus::{prop, prop_bool, prop_i32, prop_str};
use crate::item::MenuItem;
use crate::layout::StatusEntry;

/// Largest width or height of an icon pixmap kept; a larger one is dropped, so an item cannot make the bar decode and
/// hold an image of any size.
const MAX_PIXMAP_EDGE: i32 = 1024;
/// Most pixmaps of one icon kept (items send one per size).
const MAX_PIXMAPS: usize = 16;
/// Signals of one item queued on the bus connection before they are taken; the follower takes them at once and keeps
/// one of each kind ([`Changes`]).
const SIGNAL_QUEUE: usize = 64;

/// The interface an item serves: the name everyone uses, or the freedesktop draft's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ItemInterface {
    #[default]
    Kde,
    Freedesktop,
}

impl ItemInterface {
    /// Both, in the order an item is tried.
    pub const ALL: [ItemInterface; 2] = [ItemInterface::Kde, ItemInterface::Freedesktop];

    pub fn name(self) -> &'static str {
        match self {
            ItemInterface::Kde => "org.kde.StatusNotifierItem",
            ItemInterface::Freedesktop => "org.freedesktop.StatusNotifierItem",
        }
    }

    fn of(name: &str) -> Option<ItemInterface> {
        Self::ALL.into_iter().find(|i| i.name() == name)
    }
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
        self.width > 0
            && self.height > 0
            && self.argb.len() == (self.width as usize) * (self.height as usize) * 4
    }

    /// Convert to straight RGBA8; `None` for a malformed pixmap.
    pub fn to_rgba(&self) -> Option<Vec<u8>> {
        if !self.is_valid() {
            return None;
        }
        Some(
            self.argb
                .chunks(4)
                .flat_map(|p| [p[1], p[2], p[3], p[0]])
                .collect(),
        )
    }
}

/// The pixmap to draw at `target` pixels: the smallest valid one that is at least as large, else
/// the largest valid one.
pub fn best_pixmap(pixmaps: &[IconPixmap], target: i32) -> Option<&IconPixmap> {
    let valid = || pixmaps.iter().filter(|p| p.is_valid());
    valid()
        .filter(|p| p.width >= target && p.height >= target)
        .min_by_key(|p| p.width * p.height)
        .or_else(|| valid().max_by_key(|p| p.width * p.height))
}

/// An item's `ToolTip`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StatusToolTip {
    pub icon_name: String,
    pub pixmaps: Vec<IconPixmap>,
    pub title: String,
    /// May carry the specification's basic markup (`b`, `i`, `u`, `br`, `p`, `a`, `img` and entities).
    pub description: String,
}

impl StatusToolTip {
    /// The text to show: the title, then the description without its markup on the lines below.
    pub fn text(&self) -> String {
        let description = plain_text(&self.description);
        match (self.title.trim(), description.trim()) {
            ("", d) => d.to_owned(),
            (t, "") => t.to_owned(),
            (t, d) => format!("{t}\n{d}"),
        }
    }
}

/// `markup` as plain text: tags dropped, `br` and the ends of paragraphs as line breaks, entities decoded.
pub fn plain_text(markup: &str) -> String {
    let mut out = String::with_capacity(markup.len());
    let mut rest = markup;
    while let Some(c) = rest.chars().next() {
        match c {
            '<' => {
                let Some(end) = rest.find('>') else {
                    out.push_str(rest);
                    break;
                };
                let tag = rest[1..end].trim().to_ascii_lowercase();
                let name = tag.trim_start_matches('/').trim_end_matches('/').trim();
                let name = name.split_whitespace().next().unwrap_or("");
                if name == "br" || (name == "p" && tag.starts_with('/')) {
                    out.push('\n');
                }
                rest = &rest[end + 1..];
            }
            '&' => {
                let decoded = rest.find(';').and_then(|end| {
                    let text = match &rest[1..end] {
                        "amp" => "&",
                        "lt" => "<",
                        "gt" => ">",
                        "quot" => "\"",
                        "apos" => "'",
                        "nbsp" => "\u{a0}",
                        _ => return None,
                    };
                    Some((text, end))
                });
                match decoded {
                    Some((text, end)) => {
                        out.push_str(text);
                        rest = &rest[end + 1..];
                    }
                    None => {
                        out.push('&');
                        rest = &rest[1..];
                    }
                }
            }
            c => {
                out.push(c);
                rest = &rest[c.len_utf8()..];
            }
        }
    }
    out
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StatusItem {
    /// `service + path`, unique per item.
    pub id: String,
    pub service: String,
    pub path: String,
    /// The interface the item answered on.
    pub interface: ItemInterface,
    pub app_id: String,
    pub category: String,
    pub title: String,
    pub status: ItemStatus,
    /// The window the item stands for, `0` for none.
    pub window_id: i32,
    pub icon_name: String,
    pub icon_pixmaps: Vec<IconPixmap>,
    pub attention_icon_name: String,
    pub attention_pixmaps: Vec<IconPixmap>,
    /// A movie (a name or a path) to show instead of the icon while the item needs attention; drawn as a still.
    pub attention_movie_name: String,
    pub overlay_icon_name: String,
    pub overlay_pixmaps: Vec<IconPixmap>,
    /// A directory of the item's own icons, searched before the system's icon themes.
    pub icon_theme_path: String,
    pub tooltip: StatusToolTip,
    /// Left click should open the menu instead of calling `Activate`.
    pub item_is_menu: bool,
    /// Object path of the item's DBusMenu, if it has one.
    pub menu_path: Option<String>,
}

impl StatusItem {
    /// Whether the item needs attention and says how to show it.
    fn shows_attention(&self) -> bool {
        self.status == ItemStatus::NeedsAttention
            && !(self.attention_icon_name.is_empty()
                && self.attention_pixmaps.is_empty()
                && self.attention_movie_name.is_empty())
    }

    /// The icon name to show for the current status: while the item needs attention its attention icon, else its
    /// attention movie.
    pub fn current_icon_name(&self) -> &str {
        if !self.shows_attention() {
            &self.icon_name
        } else if self.attention_icon_name.is_empty() {
            &self.attention_movie_name
        } else {
            &self.attention_icon_name
        }
    }

    /// The pixmaps to show for the current status: while the item needs attention only its attention pixmaps, so an
    /// attention icon it names wins over its usual pixmaps.
    pub fn current_pixmaps(&self) -> &[IconPixmap] {
        if self.shows_attention() {
            &self.attention_pixmaps
        } else {
            &self.icon_pixmaps
        }
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

    /// Set the property `name` from `props`; one `props` lacks goes back to its default.
    fn set(&mut self, name: &str, props: &HashMap<String, OwnedValue>) {
        let text = || prop_str(props, name).unwrap_or_default();
        match name {
            "Id" => self.app_id = text(),
            "Category" => self.category = text(),
            "Title" => self.title = text(),
            "Status" => self.status = ItemStatus::parse(&text()),
            "WindowId" => self.window_id = prop_i32(props, name).unwrap_or(0),
            "IconName" => self.icon_name = text(),
            "IconPixmap" => self.icon_pixmaps = pixmaps(prop(props, name)),
            "AttentionIconName" => self.attention_icon_name = text(),
            "AttentionIconPixmap" => self.attention_pixmaps = pixmaps(prop(props, name)),
            "AttentionMovieName" => self.attention_movie_name = text(),
            "OverlayIconName" => self.overlay_icon_name = text(),
            "OverlayIconPixmap" => self.overlay_pixmaps = pixmaps(prop(props, name)),
            "IconThemePath" => self.icon_theme_path = text(),
            "ToolTip" => self.tooltip = tooltip(prop(props, name)),
            "ItemIsMenu" => self.item_is_menu = prop_bool(props, name).unwrap_or(false),
            "Menu" => self.menu_path = Some(text()).filter(|m| m != "/" && !m.is_empty()),
            _ => {}
        }
    }
}

/// Every property the host reads.
const PROPERTIES: [&str; 16] = [
    "Id",
    "Category",
    "Title",
    "Status",
    "WindowId",
    "IconName",
    "IconPixmap",
    "AttentionIconName",
    "AttentionIconPixmap",
    "AttentionMovieName",
    "OverlayIconName",
    "OverlayIconPixmap",
    "IconThemePath",
    "ToolTip",
    "ItemIsMenu",
    "Menu",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrimaryAction {
    Activate,
    ShowMenu,
}

/// An array of `(width, height, ARGB32 bytes)`, the valid ones of a size the bar keeps.
fn pixmaps(value: Option<&Value<'_>>) -> Vec<IconPixmap> {
    let Some(Value::Array(array)) = value.map(super::super::unwrap_variant) else {
        return Vec::new();
    };
    array
        .iter()
        .filter_map(|entry| {
            let Value::Structure(fields) = super::super::unwrap_variant(entry) else {
                return None;
            };
            let [Value::I32(width), Value::I32(height), Value::Array(bytes)] = fields.fields()
            else {
                return None;
            };
            if !(1..=MAX_PIXMAP_EDGE).contains(width) || !(1..=MAX_PIXMAP_EDGE).contains(height) {
                return None;
            }
            let argb: Vec<u8> = bytes
                .iter()
                .map(|b| match b {
                    Value::U8(b) => Some(*b),
                    _ => None,
                })
                .collect::<Option<_>>()?;
            let pixmap = IconPixmap {
                width: *width,
                height: *height,
                argb,
            };
            pixmap.is_valid().then_some(pixmap)
        })
        .take(MAX_PIXMAPS)
        .collect()
}

/// A `(icon name, pixmaps, title, description)` structure.
fn tooltip(value: Option<&Value<'_>>) -> StatusToolTip {
    let Some(Value::Structure(fields)) = value.map(super::super::unwrap_variant) else {
        return StatusToolTip::default();
    };
    let text = |v: &Value<'_>| match super::super::unwrap_variant(v) {
        Value::Str(s) => s.as_str().to_owned(),
        _ => String::new(),
    };
    match fields.fields() {
        [icon, pixmap, title, description] => StatusToolTip {
            icon_name: text(icon),
            pixmaps: pixmaps(Some(pixmap)),
            title: text(title),
            description: text(description),
        },
        _ => StatusToolTip::default(),
    }
}

async fn properties_proxy(
    conn: &Connection,
    service: &str,
    path: &str,
) -> zbus::Result<zbus::fdo::PropertiesProxy<'static>> {
    zbus::fdo::PropertiesProxy::builder(conn)
        .destination(service.to_owned())?
        .path(path.to_owned())?
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()
        .await
}

/// The properties `names` of `interface`, read one by one; those the item does not have are left out. An error when
/// none could be read.
async fn get_each(
    props: &zbus::fdo::PropertiesProxy<'_>,
    interface: ItemInterface,
    names: &[&str],
) -> zbus::Result<HashMap<String, OwnedValue>> {
    let iface = InterfaceName::from_static_str_unchecked(interface.name());
    let mut read = HashMap::new();
    let mut failed = None;
    for name in names {
        match props.get(iface.clone(), name).await {
            Ok(value) => {
                read.insert((*name).to_owned(), value);
            }
            Err(e) => failed = Some(e),
        }
    }
    match failed {
        Some(e) if read.is_empty() && !names.is_empty() => Err(e.into()),
        _ => Ok(read),
    }
}

/// Read an item: every property in one `GetAll` of the interface it serves, else one by one for an item that does not
/// answer `GetAll`. Missing properties keep their defaults (items implement different subsets); an item without `Id`
/// is not an item.
pub async fn fetch_item(conn: &Connection, service: &str, path: &str) -> zbus::Result<StatusItem> {
    let props = properties_proxy(conn, service, path).await?;
    let mut failed = zbus::Error::Failure(format!("{service}{path} has no status item"));
    let item = |interface, read: &HashMap<String, OwnedValue>| {
        let mut item = StatusItem {
            id: format!("{service}{path}"),
            service: service.to_owned(),
            path: path.to_owned(),
            interface,
            ..StatusItem::default()
        };
        for name in PROPERTIES {
            item.set(name, read);
        }
        item
    };
    for interface in ItemInterface::ALL {
        let iface = InterfaceName::from_static_str_unchecked(interface.name());
        match props.get_all(iface).await {
            Ok(read) if read.contains_key("Id") => return Ok(item(interface, &read)),
            Ok(_) => {}
            Err(e) => failed = e.into(),
        }
    }
    for interface in ItemInterface::ALL {
        match get_each(&props, interface, &PROPERTIES).await {
            Ok(read) if read.contains_key("Id") => return Ok(item(interface, &read)),
            Ok(_) => {}
            Err(e) => failed = e,
        }
    }
    Err(failed)
}

/// What one of an item's signals says changed.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ItemChange {
    Title,
    Icon,
    AttentionIcon,
    OverlayIcon,
    ToolTip,
    /// `NewStatus` with the status it carries; `None` when the signal carried none to read.
    Status(Option<String>),
    /// `NewIconThemePath`, with the path it carries.
    IconThemePath(Option<String>),
    /// The item's `Menu` names another object, or one for the first time.
    Menu,
}

impl ItemChange {
    /// The change a signal `member` of `msg` tells of.
    fn of(member: &str, msg: &zbus::Message) -> Option<ItemChange> {
        let text = || msg.body().deserialize::<String>().ok();
        Some(match member {
            "NewTitle" => ItemChange::Title,
            "NewIcon" => ItemChange::Icon,
            "NewAttentionIcon" => ItemChange::AttentionIcon,
            "NewOverlayIcon" => ItemChange::OverlayIcon,
            "NewToolTip" => ItemChange::ToolTip,
            "NewStatus" => ItemChange::Status(text()),
            "NewIconThemePath" => ItemChange::IconThemePath(text()),
            "NewMenu" => ItemChange::Menu,
            _ => return None,
        })
    }

    /// The properties the change asks to read again.
    fn properties(&self) -> &'static [&'static str] {
        match self {
            ItemChange::Title => &["Title"],
            // an icon of the item's own may come with the directory it is in
            ItemChange::Icon => &["IconName", "IconPixmap", "IconThemePath"],
            ItemChange::AttentionIcon => &[
                "AttentionIconName",
                "AttentionIconPixmap",
                "AttentionMovieName",
            ],
            ItemChange::OverlayIcon => &["OverlayIconName", "OverlayIconPixmap"],
            ItemChange::ToolTip => &["ToolTip"],
            ItemChange::Status(Some(_)) | ItemChange::IconThemePath(Some(_)) => &[],
            ItemChange::Status(None) => &["Status"],
            ItemChange::IconThemePath(None) => &["IconThemePath"],
            ItemChange::Menu => &["Menu", "ItemIsMenu"],
        }
    }

    /// Whether `other` is the same kind of change, which one reading answers for both.
    fn same_kind(&self, other: &ItemChange) -> bool {
        std::mem::discriminant(self) == std::mem::discriminant(other)
    }
}

/// Read again the properties of `item` that `change` names; what a signal carries is taken from it.
pub async fn refresh_item(
    conn: &Connection,
    item: &mut StatusItem,
    change: &ItemChange,
) -> zbus::Result<()> {
    match change {
        ItemChange::Status(Some(status)) => item.status = ItemStatus::parse(status),
        ItemChange::IconThemePath(Some(path)) => item.icon_theme_path = path.clone(),
        _ => {}
    }
    let names = change.properties();
    if names.is_empty() {
        return Ok(());
    }
    let props = properties_proxy(conn, &item.service, &item.path).await?;
    let read = get_each(&props, item.interface, names).await?;
    for name in names {
        item.set(name, &read);
    }
    Ok(())
}

/// Change notifications of one item, on either interface. The signals are taken off the connection as they come, by a
/// task the stream holds, and wait in the stream one of each kind, the latest, so an item that signals without pause
/// neither fills the connection's queue nor makes the host read it again for every signal.
pub async fn item_changes(
    conn: &Connection,
    service: &str,
    path: &str,
) -> zbus::Result<impl Stream<Item = ItemChange> + Unpin> {
    let rule = MatchRule::builder()
        .msg_type(Type::Signal)
        .sender(service)?
        .path(path)?
        .build();
    let mut signals = MessageStream::for_match_rule(rule, conn, Some(SIGNAL_QUEUE)).await?;
    let pending: Arc<Mutex<Vec<ItemChange>>> = Arc::default();
    let (wake, woken) = async_channel::bounded(1);
    let queue = pending.clone();
    let task = conn.executor().spawn(
        async move {
            while let Some(Ok(msg)) = signals.next().await {
                let header = msg.header();
                let ours = header
                    .interface()
                    .and_then(|i| ItemInterface::of(i.as_str()))
                    .is_some();
                let Some(change) = header
                    .member()
                    .filter(|_| ours)
                    .and_then(|m| ItemChange::of(m.as_str(), &msg))
                else {
                    continue;
                };
                {
                    let mut queue = queue.lock().unwrap_or_else(PoisonError::into_inner);
                    queue.retain(|c| !c.same_kind(&change));
                    queue.push(change);
                }
                let _ = wake.try_send(());
            }
        },
        "cupertino-status-item-signals",
    );
    Ok(Changes {
        pending,
        woken: Box::pin(woken),
        _task: task,
    })
}

/// The changes of one item waiting to be taken ([`item_changes`]).
struct Changes {
    pending: Arc<Mutex<Vec<ItemChange>>>,
    woken: Pin<Box<async_channel::Receiver<()>>>,
    _task: zbus::Task<()>,
}

impl Stream for Changes {
    type Item = ItemChange;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<ItemChange>> {
        loop {
            {
                let mut pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
                if !pending.is_empty() {
                    return Poll::Ready(Some(pending.remove(0)));
                }
            }
            match self.woken.poll_next_unpin(cx) {
                Poll::Ready(Some(())) => continue,
                // the item's signals ended with the connection
                Poll::Ready(None) => return Poll::Ready(None),
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

/// Fetch the DBusMenu of an item (the whole tree).
pub async fn load_menu(conn: &Connection, item: &StatusItem) -> Option<MenuItem> {
    let path = item.menu_path.as_deref()?;
    let client = super::super::dbusmenu::DBusMenuClient::connect(conn, &item.service, path)
        .await
        .ok()?;
    let _ = client.about_to_show(crate::item::ItemId(0)).await;
    client
        .fetch(crate::item::ItemId(0), -1)
        .await
        .ok()
        .map(|(_, m)| m)
}

/// The bar entry for an item: `width` is the icon width in points.
pub fn status_entry(item: &StatusItem, menu: Option<MenuItem>, width: f64) -> StatusEntry {
    StatusEntry {
        id: item.id.clone(),
        width,
        menu,
    }
}

/// The direction of a scroll over an item.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Orientation {
    Horizontal,
    Vertical,
}

impl Orientation {
    pub fn as_str(self) -> &'static str {
        match self {
            Orientation::Horizontal => "horizontal",
            Orientation::Vertical => "vertical",
        }
    }
}

/// What the bar asks of an item, at a point of the compositor space where one is given: a primary click
/// (`Activate`), a middle click (`SecondaryActivate`), a secondary click on an item without a menu (`ContextMenu`) or
/// a scroll by `delta` steps (`Scroll`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StatusAction {
    Activate {
        x: i32,
        y: i32,
    },
    SecondaryActivate {
        x: i32,
        y: i32,
    },
    ContextMenu {
        x: i32,
        y: i32,
    },
    Scroll {
        delta: i32,
        orientation: Orientation,
    },
}

/// Whether `e` says the object has no such interface or method.
fn is_unknown(e: &zbus::Error) -> bool {
    const UNKNOWN: [&str; 2] = [
        "org.freedesktop.DBus.Error.UnknownInterface",
        "org.freedesktop.DBus.Error.UnknownMethod",
    ];
    match e {
        zbus::Error::MethodError(name, _, _) => UNKNOWN.contains(&name.as_str()),
        zbus::Error::FDO(e) => matches!(
            **e,
            zbus::fdo::Error::UnknownInterface(_) | zbus::fdo::Error::UnknownMethod(_)
        ),
        _ => false,
    }
}

/// Send `action` to the item at `service` and `path`, on the interface everyone uses, else on the freedesktop draft's
/// for an item that has only that one.
pub async fn send_action(
    conn: &Connection,
    service: &str,
    path: &str,
    action: StatusAction,
) -> zbus::Result<()> {
    let mut result = Ok(());
    for interface in ItemInterface::ALL {
        let iface = Some(interface.name());
        let reply = match action {
            StatusAction::Activate { x, y } => {
                conn.call_method(Some(service), path, iface, "Activate", &(x, y))
                    .await
            }
            StatusAction::SecondaryActivate { x, y } => {
                conn.call_method(Some(service), path, iface, "SecondaryActivate", &(x, y))
                    .await
            }
            StatusAction::ContextMenu { x, y } => {
                conn.call_method(Some(service), path, iface, "ContextMenu", &(x, y))
                    .await
            }
            StatusAction::Scroll { delta, orientation } => {
                let body = (delta, orientation.as_str());
                conn.call_method(Some(service), path, iface, "Scroll", &body)
                    .await
            }
        };
        match reply {
            Err(e) if is_unknown(&e) => result = Err(e),
            reply => return reply.map(drop),
        }
    }
    result
}

pub async fn activate_item(
    conn: &Connection,
    item: &StatusItem,
    x: i32,
    y: i32,
) -> zbus::Result<()> {
    send_action(
        conn,
        &item.service,
        &item.path,
        StatusAction::Activate { x, y },
    )
    .await
}

pub async fn secondary_activate_item(
    conn: &Connection,
    item: &StatusItem,
    x: i32,
    y: i32,
) -> zbus::Result<()> {
    let action = StatusAction::SecondaryActivate { x, y };
    send_action(conn, &item.service, &item.path, action).await
}

/// Ask the item to show its own context menu (for items without a DBusMenu).
pub async fn context_menu_item(
    conn: &Connection,
    item: &StatusItem,
    x: i32,
    y: i32,
) -> zbus::Result<()> {
    let action = StatusAction::ContextMenu { x, y };
    send_action(conn, &item.service, &item.path, action).await
}

pub async fn scroll_item(
    conn: &Connection,
    item: &StatusItem,
    delta: i32,
    orientation: Orientation,
) -> zbus::Result<()> {
    let action = StatusAction::Scroll { delta, orientation };
    send_action(conn, &item.service, &item.path, action).await
}
