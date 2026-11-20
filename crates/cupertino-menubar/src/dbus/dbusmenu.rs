//! Client for `com.canonical.dbusmenu`.
//!
//! Wire format (an open Canonical specification): `GetLayout(parentId i, recursionDepth i,
//! propertyNames as) → (revision u, layout (ia{sv}av))`, where every `av` element is a variant
//! holding a nested `(ia{sv}av)` node. Items carry the properties `type`, `label`, `enabled`,
//! `visible`, `icon-name`, `icon-data`, `shortcut`, `toggle-type`, `toggle-state` and
//! `children-display`. Events (`clicked`, `hovered`, `opened`, `closed`) go through `Event`;
//! `AboutToShow` lets a service refresh a submenu lazily; `LayoutUpdated` and
//! `ItemsPropertiesUpdated` signal changes.

use std::collections::HashMap;

use futures_util::{Stream, StreamExt};
use zbus::proxy;
use zbus::zvariant::{OwnedValue, Value};
use zbus::Connection;

use crate::fallback;
use crate::item::{ItemId, MenuItem, Origin, Shortcut, Toggle};

/// One node of the wire layout: id, properties, children (variants holding nested nodes).
pub type RawNode = (i32, HashMap<String, OwnedValue>, Vec<OwnedValue>);

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

    fn about_to_show(&self, id: i32) -> zbus::Result<bool>;

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
    if let Some(Value::Array(a)) = prop(props, "icon-data") {
        let bytes: Vec<u8> = a
            .iter()
            .filter_map(|v| {
                if let Value::U8(b) = super::unwrap_variant(v) {
                    Some(*b)
                } else {
                    None
                }
            })
            .collect();
        item.icon_data = (!bytes.is_empty()).then_some(bytes);
    }
    if let Some(Value::Array(a)) = prop(props, "shortcut") {
        // aas: take the first combination
        if let Some(Value::Array(first)) = a.iter().next().map(super::unwrap_variant) {
            let tokens: Vec<String> = first
                .iter()
                .filter_map(|t| {
                    if let Value::Str(s) = super::unwrap_variant(t) {
                        Some(s.as_str().to_string())
                    } else {
                        None
                    }
                })
                .collect();
            item.shortcut = Shortcut::from_tokens(&tokens);
        }
    }
    let toggle_type = prop_str(props, "toggle-type");
    let toggle_state = prop_i32(props, "toggle-state");
    if toggle_type.is_some() || toggle_state.is_some() {
        let kind = toggle_type.unwrap_or_else(|| match item.toggle {
            Toggle::Radio(_) => "radio".into(),
            Toggle::None => String::new(),
            _ => "checkmark".into(),
        });
        let state = toggle_state.unwrap_or(match item.toggle {
            Toggle::Check(true) | Toggle::Radio(true) => 1,
            Toggle::Indeterminate => -1,
            _ => 0,
        });
        item.toggle = match (kind.as_str(), state) {
            ("checkmark", 0) => Toggle::Check(false),
            ("checkmark", 1) => Toggle::Check(true),
            ("radio", 0) => Toggle::Radio(false),
            ("radio", 1) => Toggle::Radio(true),
            ("checkmark" | "radio", _) => Toggle::Indeterminate,
            _ => Toggle::None,
        };
    }
    if let Some(d) = prop_str(props, "children-display") {
        item.has_submenu = d == "submenu";
    }
}

/// Reset one property to its protocol default (`ItemsPropertiesUpdated` removed properties).
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
        "toggle-type" | "toggle-state" => item.toggle = Toggle::None,
        "children-display" => item.has_submenu = !item.children.is_empty(),
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
    apply_properties(&mut item, props);
    if item.separator {
        item.enabled = false;
    }
    for c in children {
        let raw = node_from_value(c)?;
        // the bar's own items use those ids; an exporter's item with one is left out
        if fallback::is_system(ItemId(raw.0)) {
            continue;
        }
        item.children.push(parse_node(&raw, depth + 1)?);
    }
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
    /// The service asks for the item to be activated (for example a hot key was pressed).
    ActivationRequested { id: ItemId, timestamp: u32 },
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
}

impl DBusMenuClient {
    pub async fn connect(conn: &Connection, service: &str, path: &str) -> zbus::Result<Self> {
        let proxy = DBusMenuProxy::builder(conn)
            .destination(service.to_string())?
            .path(path.to_string())?
            .build()
            .await?;
        Ok(Self { proxy })
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

    /// `AboutToShow`; returns whether the layout changed and must be refetched.
    pub async fn about_to_show(&self, id: ItemId) -> zbus::Result<bool> {
        self.proxy.about_to_show(id.0).await
    }

    /// Send the `clicked` event for `id`.
    pub async fn activate(&self, id: ItemId, timestamp: u32) -> zbus::Result<()> {
        self.proxy
            .event(id.0, "clicked", &Value::from(0i32), timestamp)
            .await
    }

    pub async fn hover(&self, id: ItemId, timestamp: u32) -> zbus::Result<()> {
        self.proxy
            .event(id.0, "hovered", &Value::from(0i32), timestamp)
            .await
    }

    /// Tell the service a submenu was opened (`opened`) or closed (`closed`).
    pub async fn notify_open(&self, id: ItemId, open: bool, timestamp: u32) -> zbus::Result<()> {
        self.proxy
            .event(
                id.0,
                if open { "opened" } else { "closed" },
                &Value::from(0i32),
                timestamp,
            )
            .await
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
        Ok(futures_util::stream::select(futures_util::stream::select(layout, props), act).boxed())
    }
}
