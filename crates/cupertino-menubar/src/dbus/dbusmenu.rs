//! Client for `com.canonical.dbusmenu`.
//!
//! Wire format (an open Canonical specification): `GetLayout(parentId i, recursionDepth i,
//! propertyNames as) → (revision u, layout (ia{sv}av))`, where every `av` element is a variant
//! holding a nested `(ia{sv}av)` node. Items carry the properties `type`, `label`, `enabled`,
//! `visible`, `icon-name`, `icon-data`, `shortcut`, `toggle-type`, `toggle-state`, `children-display`,
//! `disposition` and `accessible-desc`. Events (`clicked`, `hovered`, `opened`, `closed`) go through `Event`, or
//! several at once through `EventGroup`; `AboutToShow` (`AboutToShowGroup` for several) lets a service refresh a
//! submenu lazily; `LayoutUpdated` and `ItemsPropertiesUpdated` signal changes, `ItemActivationRequested` asks for a
//! menu to be opened. The menu as a whole has the properties `Version`, `TextDirection`, `Status` and
//! `IconThemePath` ([`MenuProperties`]).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use futures_util::{Stream, StreamExt};
use zbus::fdo::PropertiesProxy;
use zbus::proxy;
use zbus::proxy::CacheProperties;
use zbus::zvariant::{OwnedValue, Value};
use zbus::Connection;

use crate::fallback;
use crate::item::{Disposition, ItemId, MenuItem, Origin, Shortcut, Toggle};

/// One node of the wire layout: id, properties, children (variants holding nested nodes).
pub type RawNode = (i32, HashMap<String, OwnedValue>, Vec<OwnedValue>);

/// The interface's name, as `PropertiesChanged` names it.
pub const INTERFACE: &str = "com.canonical.dbusmenu";

/// Properties requested from `GetLayout`.
pub const LAYOUT_PROPERTIES: &[&str] = &[
    "type",
    "label",
    "enabled",
    "visible",
    "icon-name",
    "icon-data",
    "shortcut",
    "toggle-type",
    "toggle-state",
    "children-display",
    "disposition",
    "accessible-desc",
];

#[proxy(
    gen_blocking = false,
    interface = "com.canonical.dbusmenu",
    assume_defaults = false
)]
pub trait DBusMenu {
    fn get_layout(
        &self,
        parent_id: i32,
        recursion_depth: i32,
        property_names: &[&str],
    ) -> zbus::Result<(u32, RawNode)>;

    fn get_group_properties(
        &self,
        ids: &[i32],
        property_names: &[&str],
    ) -> zbus::Result<Vec<(i32, HashMap<String, OwnedValue>)>>;

    fn get_property(&self, id: i32, name: &str) -> zbus::Result<OwnedValue>;

    fn event(&self, id: i32, event_id: &str, data: &Value<'_>, timestamp: u32) -> zbus::Result<()>;

    /// `(id, eventId, data, timestamp)` for each event; answers the ids that were not found.
    fn event_group(&self, events: &[(i32, &str, Value<'_>, u32)]) -> zbus::Result<Vec<i32>>;

    fn about_to_show(&self, id: i32) -> zbus::Result<bool>;

    /// Answers the ids whose layouts are to be read again, then the ids that were not found.
    fn about_to_show_group(&self, ids: &[i32]) -> zbus::Result<(Vec<i32>, Vec<i32>)>;

    #[zbus(signal)]
    fn items_properties_updated(
        &self,
        updated_props: Vec<(i32, HashMap<String, OwnedValue>)>,
        removed_props: Vec<(i32, Vec<String>)>,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    fn layout_updated(&self, revision: u32, parent: i32) -> zbus::Result<()>;

    #[zbus(signal)]
    fn item_activation_requested(&self, id: i32, timestamp: u32) -> zbus::Result<()>;

    #[zbus(property)]
    fn version(&self) -> zbus::Result<u32>;

    #[zbus(property)]
    fn status(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn text_direction(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn icon_theme_path(&self) -> zbus::Result<Vec<String>>;
}

/// The first protocol version with `AboutToShowGroup` and `EventGroup`. An exporter of an earlier version is sent the
/// single calls; one that does not say its version is tried, and sent the single calls once it answers that it has no
/// such method.
pub const GROUP_VERSION: u32 = 3;
/// The most directories of an exporter's `IconThemePath` searched (the list comes from the app).
const MAX_ICON_DIRS: usize = 8;

/// The menu as a whole, as the exporter's properties describe it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MenuProperties {
    /// The protocol version the exporter speaks; 0 when it does not say.
    pub version: u32,
    /// `TextDirection` is `rtl`: the menus' rows are laid out right to left.
    pub rtl: bool,
    /// `Status` is `notice`: the menus ask to be noticed.
    pub notice: bool,
    /// `IconThemePath`: directories searched for the items' icon names, absolute paths only.
    pub icon_dirs: Arc<[PathBuf]>,
}

impl MenuProperties {
    /// Apply the properties `GetAll` answered or `PropertiesChanged` changed, and reset those it `invalidated` to their
    /// defaults; `true` when something changed.
    pub fn apply(&mut self, changed: &HashMap<String, OwnedValue>, invalidated: &[String]) -> bool {
        use super::{prop, prop_i32, prop_str};
        let before = self.clone();
        for name in invalidated {
            match name.as_str() {
                "Version" => self.version = 0,
                "TextDirection" => self.rtl = false,
                "Status" => self.notice = false,
                "IconThemePath" => self.icon_dirs = Arc::from([]),
                _ => {}
            }
        }
        if let Some(v) = prop_i32(changed, "Version") {
            self.version = u32::try_from(v).unwrap_or(0);
        }
        if let Some(d) = prop_str(changed, "TextDirection") {
            self.rtl = d == "rtl";
        }
        if let Some(s) = prop_str(changed, "Status") {
            self.notice = s == "notice";
        }
        if let Some(v) = prop(changed, "IconThemePath") {
            let dirs: Vec<PathBuf> = strings(v)
                .into_iter()
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .take(MAX_ICON_DIRS)
                .collect();
            self.icon_dirs = Arc::from(dirs);
        }
        *self != before
    }

    /// Whether to send the group calls rather than the single ones.
    pub fn has_groups(&self) -> bool {
        self.version == 0 || self.version >= GROUP_VERSION
    }

    /// Lay the menu's properties on its tree: the direction and the icon directories on every item, the status on
    /// the root.
    pub fn decorate(&self, root: &mut MenuItem) {
        root.notice = self.notice;
        root.for_each_mut(&mut |item| {
            item.rtl = self.rtl;
            item.icon_dirs = Arc::clone(&self.icon_dirs);
        });
    }
}

/// An event the bar sends an exporter about one of its menus.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MenuEvent {
    /// The item's menu shows.
    Opened,
    /// The item's menu no longer shows.
    Closed,
    /// The person chose the item.
    Clicked,
}

impl MenuEvent {
    /// The event's id on the wire.
    pub fn name(self) -> &'static str {
        match self {
            MenuEvent::Opened => "opened",
            MenuEvent::Closed => "closed",
            MenuEvent::Clicked => "clicked",
        }
    }
}

/// The data every event carries: none is defined for the events the bar sends, and the protocol's reference client
/// sends an `i32` 0 in the variant.
fn event_data() -> Value<'static> {
    Value::from(0i32)
}

/// Whether the exporter answered that it has no such method (an earlier version of the protocol).
pub fn is_unknown_method(e: &zbus::Error) -> bool {
    const UNKNOWN_METHOD: &str = "org.freedesktop.DBus.Error.UnknownMethod";
    match e {
        zbus::Error::MethodError(name, _, _) => name.as_str() == UNKNOWN_METHOD,
        zbus::Error::FDO(e) => matches!(**e, zbus::fdo::Error::UnknownMethod(_)),
        _ => false,
    }
}

#[derive(Debug)]
pub enum ParseError {
    NotAStructure,
    BadNode(&'static str),
    TooDeep,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::NotAStructure => write!(f, "layout child is not a structure"),
            ParseError::BadNode(w) => write!(f, "malformed layout node: {w}"),
            ParseError::TooDeep => write!(f, "layout nesting too deep"),
        }
    }
}

impl std::error::Error for ParseError {}

const MAX_DEPTH: usize = 32;

/// Convert a variant holding a nested node into a [`RawNode`].
pub fn node_from_value(v: &Value<'_>) -> Result<RawNode, ParseError> {
    let Value::Structure(s) = super::unwrap_variant(v) else {
        return Err(ParseError::NotAStructure);
    };
    let fields = s.fields();
    if fields.len() != 3 {
        return Err(ParseError::BadNode("expected three fields"));
    }
    let id = match super::unwrap_variant(&fields[0]) {
        Value::I32(i) => *i,
        _ => return Err(ParseError::BadNode("id")),
    };
    let props = match super::unwrap_variant(&fields[1]) {
        Value::Dict(d) => {
            let mut m = HashMap::new();
            for (k, val) in d.iter() {
                let Value::Str(k) = super::unwrap_variant(k) else {
                    return Err(ParseError::BadNode("property key"));
                };
                let owned =
                    OwnedValue::try_from(val).map_err(|_| ParseError::BadNode("property value"))?;
                m.insert(k.as_str().to_string(), owned);
            }
            m
        }
        _ => return Err(ParseError::BadNode("properties")),
    };
    let children = match super::unwrap_variant(&fields[2]) {
        Value::Array(a) => a
            .iter()
            .map(|c| OwnedValue::try_from(c).map_err(|_| ParseError::BadNode("child")))
            .collect::<Result<Vec<_>, _>>()?,
        _ => return Err(ParseError::BadNode("children")),
    };
    Ok((id, props, children))
}

/// The `toggle-state` of an item that has none: indeterminate.
const DEFAULT_TOGGLE_STATE: i32 = -1;

/// The `toggle-type` name of a toggle, `""` for none.
fn toggle_kind(toggle: Toggle) -> &'static str {
    match toggle {
        Toggle::None => "",
        Toggle::Check(_) | Toggle::Indeterminate => "checkmark",
        Toggle::Radio(_) | Toggle::RadioIndeterminate => "radio",
    }
}

/// The `toggle-state` of a toggle: 1 on, 0 off, -1 neither (also for no toggle, the protocol's default).
fn toggle_state(toggle: Toggle) -> i32 {
    match toggle {
        Toggle::Check(true) | Toggle::Radio(true) => 1,
        Toggle::Check(false) | Toggle::Radio(false) => 0,
        Toggle::None | Toggle::Indeterminate | Toggle::RadioIndeterminate => DEFAULT_TOGGLE_STATE,
    }
}

/// The toggle of a `toggle-type` and a `toggle-state`: a state other than 0 or 1 is neither on nor off.
fn toggle(kind: &str, state: i32) -> Toggle {
    match (kind, state) {
        ("checkmark", 0 | 1) => Toggle::Check(state == 1),
        ("checkmark", _) => Toggle::Indeterminate,
        ("radio", 0 | 1) => Toggle::Radio(state == 1),
        ("radio", _) => Toggle::RadioIndeterminate,
        _ => Toggle::None,
    }
}

/// The strings of an `as` value, or of each element of an `aas` value's arrays.
fn strings(value: &Value<'_>) -> Vec<String> {
    let Value::Array(a) = value else {
        return Vec::new();
    };
    a.iter()
        .filter_map(|v| match super::unwrap_variant(v) {
            Value::Str(s) => Some(s.as_str().to_string()),
            _ => None,
        })
        .collect()
}

/// The `shortcut` property (`aas`): the key presses of the sequence, each its modifiers and its key.
fn shortcut(value: &Value<'_>) -> Option<Shortcut> {
    let Value::Array(chords) = value else {
        return None;
    };
    let chords: Vec<Vec<String>> = chords
        .iter()
        .map(|chord| strings(super::unwrap_variant(chord)))
        .collect();
    Shortcut::from_sequence(&chords)
}

/// The `icon-data` property: the bytes of a PNG image.
fn icon_data(value: &Value<'_>) -> Option<Vec<u8>> {
    let Value::Array(a) = value else {
        return None;
    };
    let bytes: Vec<u8> = a
        .iter()
        .filter_map(|v| match super::unwrap_variant(v) {
            Value::U8(b) => Some(*b),
            _ => None,
        })
        .collect();
    (!bytes.is_empty()).then_some(bytes)
}

/// The most characters of an `accessible-desc` kept (the text comes from the app).
const MAX_DESCRIPTION_CHARS: usize = 512;

/// Apply the dbusmenu properties in `props` to `item`.
pub fn apply_properties(item: &mut MenuItem, props: &HashMap<String, OwnedValue>) {
    use super::{prop, prop_bool, prop_i32, prop_str};
    if let Some(t) = prop_str(props, "type") {
        item.separator = t == "separator";
    }
    if let Some(l) = prop_str(props, "label") {
        let (label, mnemonic) = crate::item::split_mnemonic(&l);
        item.label = label;
        item.mnemonic = mnemonic;
    }
    if let Some(b) = prop_bool(props, "enabled") {
        item.enabled = b;
    }
    if let Some(b) = prop_bool(props, "visible") {
        item.visible = b;
    }
    if let Some(n) = prop_str(props, "icon-name") {
        item.icon_name = (!n.is_empty()).then_some(n);
    }
    if let Some(v) = prop(props, "icon-data") {
        item.icon_data = icon_data(v);
    }
    if let Some(v) = prop(props, "shortcut") {
        item.shortcut = shortcut(v);
    }
    if let Some(d) = prop_str(props, "disposition") {
        item.disposition = Disposition::from_name(&d);
    }
    if let Some(d) = prop_str(props, "accessible-desc") {
        let d = crate::item::plain_text(&d, MAX_DESCRIPTION_CHARS);
        item.description = (!d.is_empty()).then_some(d);
    }
    let kind = prop_str(props, "toggle-type");
    let state = prop_i32(props, "toggle-state");
    if kind.is_some() || state.is_some() {
        let kind = kind.unwrap_or_else(|| toggle_kind(item.toggle).to_owned());
        item.toggle = toggle(&kind, state.unwrap_or(toggle_state(item.toggle)));
    }
    if let Some(d) = prop_str(props, "children-display") {
        item.has_submenu = d == "submenu" || !item.children.is_empty();
    }
}

/// Reset one property to its protocol default (`ItemsPropertiesUpdated` removed properties): a standard item, enabled,
/// visible, without label, icon or shortcut, no toggle, an indeterminate state, normal disposition.
pub fn reset_property(item: &mut MenuItem, name: &str) {
    match name {
        "type" => item.separator = false,
        "label" => {
            item.label.clear();
            item.mnemonic = None;
        }
        "enabled" => item.enabled = true,
        "visible" => item.visible = true,
        "icon-name" => item.icon_name = None,
        "icon-data" => item.icon_data = None,
        "shortcut" => item.shortcut = None,
        "toggle-type" => item.toggle = Toggle::None,
        "toggle-state" => item.toggle = toggle(toggle_kind(item.toggle), DEFAULT_TOGGLE_STATE),
        "children-display" => item.has_submenu = !item.children.is_empty(),
        "disposition" => item.disposition = Disposition::Normal,
        "accessible-desc" => item.description = None,
        _ => {}
    }
}

fn parse_node(node: &RawNode, depth: usize) -> Result<MenuItem, ParseError> {
    if depth > MAX_DEPTH {
        return Err(ParseError::TooDeep);
    }
    let (id, props, children) = node;
    let mut item = MenuItem {
        id: ItemId(*id),
        origin: Origin::Exporter,
        enabled: true,
        visible: true,
        ..Default::default()
    };
    for c in children {
        let raw = node_from_value(c)?;
        // the bar's own items use those ids; an exporter's item with one is left out
        if fallback::is_system(ItemId(raw.0)) {
            continue;
        }
        item.children.push(parse_node(&raw, depth + 1)?);
    }
    // a separator is never chosen, whatever it says of `enabled`, so one turned standard keeps what it said
    apply_properties(&mut item, props);
    if !item.children.is_empty() {
        item.has_submenu = true;
    }
    Ok(item)
}

/// Parse a `GetLayout` reply node into the root [`MenuItem`] (id 0 for the whole menu).
pub fn parse_layout(node: &RawNode) -> Result<MenuItem, ParseError> {
    parse_node(node, 0)
}

/// Apply an `ItemsPropertiesUpdated` signal to a tree. Unknown ids are ignored.
pub fn apply_properties_update(
    root: &mut MenuItem,
    updated: &[(i32, HashMap<String, OwnedValue>)],
    removed: &[(i32, Vec<String>)],
) {
    for (id, props) in updated {
        if let Some(item) = root.find_mut(ItemId(*id)) {
            apply_properties(item, props);
        }
    }
    for (id, names) in removed {
        if let Some(item) = root.find_mut(ItemId(*id)) {
            for n in names {
                reset_property(item, n);
            }
        }
    }
}

/// Replace the subtree under `parent` of `root` with `fresh`'s node of the same id.
pub fn replace_subtree(root: &mut MenuItem, fresh: MenuItem) -> bool {
    match root.find_mut(fresh.id) {
        Some(slot) => {
            *slot = fresh;
            true
        }
        None => false,
    }
}

/// What a followed menu reads again after a batch of `LayoutUpdated` signals.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reread {
    /// The layouts below these items.
    Parents(Vec<ItemId>),
    /// The whole tree.
    Whole,
}

/// At most this many parents of one batch are read one by one; more are read as the whole tree.
const REREAD_PARENTS: usize = 4;

/// The tree of one exporter as its replies and signals describe it, with the revision the exporter gave the last
/// whole layout read.
///
/// Its user makes one read at a time and applies the replies in the order asked, so a reply never replaces a newer
/// one. A signal's revision says which part to read: at or past the revision held, the parent it names; below it, the
/// whole tree, since an exporter that counts again from the start has replaced its menu.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MenuTree {
    root: Option<MenuItem>,
    revision: u32,
    props: MenuProperties,
}

impl MenuTree {
    /// The tree, once a whole layout was read.
    pub fn root(&self) -> Option<&MenuItem> {
        self.root.as_ref()
    }

    /// The revision of the last whole layout read.
    pub fn revision(&self) -> u32 {
        self.revision
    }

    /// No whole layout was read yet, so there is nothing to read a part of.
    pub fn is_unread(&self) -> bool {
        self.root.is_none()
    }

    /// What to read for the `LayoutUpdated` signals `(revision, parent)` of one batch; `None` for no signal, and
    /// before a whole layout was read.
    pub fn reread(&self, signals: &[(u32, ItemId)]) -> Option<Reread> {
        let root = self.root.as_ref()?;
        let mut parents: Vec<ItemId> = Vec::new();
        for (revision, parent) in signals {
            // a parent the tree does not hold changed with something above it
            if *revision < self.revision || parent.0 == 0 || root.find(*parent).is_none() {
                return Some(Reread::Whole);
            }
            if !parents.contains(parent) {
                parents.push(*parent);
            }
        }
        match parents.len() {
            0 => None,
            n if n > REREAD_PARENTS => Some(Reread::Whole),
            _ => Some(Reread::Parents(parents)),
        }
    }

    /// A `GetLayout` reply for `parent` arrived. `false` when the tree has no such item any more (or was never read),
    /// so the whole tree is to be read instead.
    pub fn apply_reply(&mut self, parent: ItemId, revision: u32, layout: MenuItem) -> bool {
        if parent.0 == 0 {
            self.root = Some(layout);
            self.revision = revision;
            return true;
        }
        match &mut self.root {
            Some(root) if layout.id == parent => replace_subtree(root, layout),
            _ => false,
        }
    }

    /// Apply an `ItemsPropertiesUpdated` signal; `true` when the tree changed.
    pub fn apply_properties(
        &mut self,
        updated: &[(i32, HashMap<String, OwnedValue>)],
        removed: &[(i32, Vec<String>)],
    ) -> bool {
        let Some(root) = &mut self.root else {
            return false;
        };
        let before = root.clone();
        apply_properties_update(root, updated, removed);
        *root != before
    }

    /// Whether `id` is a submenu whose rows were never read (or an item the tree does not hold).
    pub fn is_unloaded(&self, id: ItemId) -> bool {
        match self.root.as_ref().map(|root| root.find(id)) {
            Some(Some(item)) => item.needs_load(),
            _ => true,
        }
    }

    /// The menu's own properties as last read.
    pub fn properties(&self) -> &MenuProperties {
        &self.props
    }

    /// Apply the menu's properties `GetAll` answered or `PropertiesChanged` changed; `true` when they changed.
    pub fn apply_menu_properties(
        &mut self,
        changed: &HashMap<String, OwnedValue>,
        invalidated: &[String],
    ) -> bool {
        self.props.apply(changed, invalidated)
    }

    /// The tree as the bars show it: with the menu's properties laid on its items ([`MenuProperties::decorate`]).
    pub fn shown(&self) -> Option<MenuItem> {
        let mut root = self.root.clone()?;
        self.props.decorate(&mut root);
        Some(root)
    }

    /// Whether the tree holds an item of this id.
    pub fn holds(&self, id: ItemId) -> bool {
        self.root
            .as_ref()
            .is_some_and(|root| root.find(id).is_some())
    }
}

/// A change reported by the service.
#[derive(Debug)]
pub enum MenuUpdate {
    /// The layout below `parent` changed; refetch it.
    Layout { revision: u32, parent: ItemId },
    Properties {
        updated: Vec<(i32, HashMap<String, OwnedValue>)>,
        removed: Vec<(i32, Vec<String>)>,
    },
    /// The service asks for the item's menu to be opened (for example a hot key was pressed).
    ActivationRequested { id: ItemId, timestamp: u32 },
    /// The menu's own properties changed (`PropertiesChanged`), for [`MenuProperties::apply`].
    Menu {
        changed: HashMap<String, OwnedValue>,
        invalidated: Vec<String>,
    },
}

/// Encode a node as the variant a parent's `av` children array holds (for services and tests).
pub fn node_to_value(node: RawNode) -> zbus::zvariant::Result<OwnedValue> {
    use zbus::zvariant::{Array, Dict, Signature, StructureBuilder};
    let (id, props, children) = node;
    let mut dict = Dict::new(&Signature::Str, &Signature::Variant);
    for (k, v) in props {
        dict.append(Value::from(k), Value::Value(Box::new(Value::from(v))))?;
    }
    let mut array = Array::new(&Signature::Variant);
    for c in children {
        array.append(Value::Value(Box::new(Value::from(c))))?;
    }
    let st = StructureBuilder::new()
        .add_field(id)
        .append_field(Value::Dict(dict))
        .append_field(Value::Array(array))
        .build()?;
    OwnedValue::try_from(Value::Structure(st))
}

/// Build a wire node from plain parts: `children` are nodes of the same shape.
pub fn make_node(
    id: i32,
    props: HashMap<String, OwnedValue>,
    children: Vec<RawNode>,
) -> zbus::zvariant::Result<RawNode> {
    let kids = children
        .into_iter()
        .map(node_to_value)
        .collect::<Result<Vec<_>, _>>()?;
    Ok((id, props, kids))
}

pub struct DBusMenuClient {
    proxy: DBusMenuProxy<'static>,
    /// The object's `org.freedesktop.DBus.Properties`, for the menu's own properties and their changes.
    properties: PropertiesProxy<'static>,
}

impl DBusMenuClient {
    pub async fn connect(conn: &Connection, service: &str, path: &str) -> zbus::Result<Self> {
        // the menu's properties are read and followed by the follower, which waits for nothing it did not ask
        let proxy = DBusMenuProxy::builder(conn)
            .destination(service.to_string())?
            .path(path.to_string())?
            .cache_properties(CacheProperties::No)
            .build()
            .await?;
        let properties = PropertiesProxy::builder(conn)
            .destination(service.to_string())?
            .path(path.to_string())?
            .cache_properties(CacheProperties::No)
            .build()
            .await?;
        Ok(Self { proxy, properties })
    }

    pub fn proxy(&self) -> &DBusMenuProxy<'static> {
        &self.proxy
    }

    /// Fetch the layout below `parent` (0 for the root) down to `depth` levels (-1 = all).
    pub async fn fetch(&self, parent: ItemId, depth: i32) -> zbus::Result<(u32, MenuItem)> {
        let (revision, node) = self
            .proxy
            .get_layout(parent.0, depth, LAYOUT_PROPERTIES)
            .await?;
        let item = parse_layout(&node).map_err(|e| zbus::Error::Failure(e.to_string()))?;
        Ok((revision, item))
    }

    /// The menu's own properties (`Version`, `TextDirection`, `Status`, `IconThemePath`), for
    /// [`MenuProperties::apply`].
    pub async fn menu_properties(&self) -> zbus::Result<HashMap<String, OwnedValue>> {
        let interface = zbus::names::InterfaceName::from_static_str_unchecked(INTERFACE);
        Ok(self.properties.get_all(interface).await?)
    }

    /// `AboutToShow`; returns whether the layout changed and must be refetched.
    pub async fn about_to_show(&self, id: ItemId) -> zbus::Result<bool> {
        self.proxy.about_to_show(id.0).await
    }

    /// `AboutToShowGroup`: the items among `ids` whose layouts are to be read again. An exporter of an earlier
    /// version answers [`is_unknown_method`].
    pub async fn about_to_show_group(&self, ids: &[ItemId]) -> zbus::Result<Vec<ItemId>> {
        let ids: Vec<i32> = ids.iter().map(|id| id.0).collect();
        let (update, _not_found) = self.proxy.about_to_show_group(&ids).await?;
        Ok(update.into_iter().map(ItemId).collect())
    }

    /// Send `event` for `id`, at `timestamp` (milliseconds of the input clock).
    pub async fn event(&self, id: ItemId, event: MenuEvent, timestamp: u32) -> zbus::Result<()> {
        self.proxy
            .event(id.0, event.name(), &event_data(), timestamp)
            .await
    }

    /// Send several events in one `EventGroup` call. An exporter of an earlier version answers
    /// [`is_unknown_method`].
    pub async fn event_group(&self, events: &[(ItemId, MenuEvent, u32)]) -> zbus::Result<()> {
        let events: Vec<(i32, &str, Value<'_>, u32)> = events
            .iter()
            .map(|(id, event, time)| (id.0, event.name(), event_data(), *time))
            .collect();
        // the ids not found are items the exporter removed since; nothing is to be done for them
        self.proxy.event_group(&events).await.map(drop)
    }

    /// Send the `clicked` event for `id`.
    pub async fn activate(&self, id: ItemId, timestamp: u32) -> zbus::Result<()> {
        self.event(id, MenuEvent::Clicked, timestamp).await
    }

    /// Stream of change notifications from the service.
    pub async fn updates(&self) -> zbus::Result<impl Stream<Item = MenuUpdate> + Unpin> {
        let layout = self
            .proxy
            .receive_layout_updated()
            .await?
            .filter_map(|s| async move {
                let a = s.args().ok()?;
                Some(MenuUpdate::Layout {
                    revision: a.revision,
                    parent: ItemId(a.parent),
                })
            });
        let props = self
            .proxy
            .receive_items_properties_updated()
            .await?
            .filter_map(|s| async move {
                let a = s.args().ok()?;
                Some(MenuUpdate::Properties {
                    updated: a.updated_props,
                    removed: a.removed_props,
                })
            });
        let act = self
            .proxy
            .receive_item_activation_requested()
            .await?
            .filter_map(|s| async move {
                let a = s.args().ok()?;
                Some(MenuUpdate::ActivationRequested {
                    id: ItemId(a.id),
                    timestamp: a.timestamp,
                })
            });
        let menu = self
            .properties
            .receive_properties_changed_with_args(&[(0, INTERFACE)])
            .await?
            .filter_map(|s| async move {
                let a = s.args().ok()?;
                let changed = a
                    .changed_properties()
                    .iter()
                    .filter_map(|(k, v)| Some(((*k).to_owned(), OwnedValue::try_from(v).ok()?)))
                    .collect();
                let invalidated = a
                    .invalidated_properties()
                    .iter()
                    .map(|name| (*name).to_owned())
                    .collect();
                Some(MenuUpdate::Menu {
                    changed,
                    invalidated,
                })
            });
        let items = futures_util::stream::select(futures_util::stream::select(layout, props), act);
        Ok(futures_util::stream::select(items, menu).boxed())
    }
}
