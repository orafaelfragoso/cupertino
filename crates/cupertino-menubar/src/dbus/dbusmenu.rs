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

use crate::item::{ItemId, MenuItem, Shortcut, Toggle};

/// One node of the wire layout: id, properties, children (variants holding nested nodes).
pub type RawNode = (i32, HashMap<String, OwnedValue>, Vec<OwnedValue>);

/// Properties requested from `GetLayout`.
pub const LAYOUT_PROPERTIES: &[&str] =
    &["type", "label", "enabled", "visible", "icon-name", "icon-data", "shortcut", "toggle-type", "toggle-state", "children-display"];

#[proxy(interface = "com.canonical.dbusmenu", assume_defaults = false)]
pub trait DBusMenu {
    fn get_layout(&self, parent_id: i32, recursion_depth: i32, property_names: &[&str]) -> zbus::Result<(u32, RawNode)>;

    fn get_group_properties(&self, ids: &[i32], property_names: &[&str]) -> zbus::Result<Vec<(i32, HashMap<String, OwnedValue>)>>;

    fn get_property(&self, id: i32, name: &str) -> zbus::Result<OwnedValue>;

    fn event(&self, id: i32, event_id: &str, data: &Value<'_>, timestamp: u32) -> zbus::Result<()>;

    fn about_to_show(&self, id: i32) -> zbus::Result<bool>;

    #[zbus(signal)]
    fn items_properties_updated(&self, updated_props: Vec<(i32, HashMap<String, OwnedValue>)>, removed_props: Vec<(i32, Vec<String>)>) -> zbus::Result<()>;

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
    let Value::Structure(s) = super::unwrap_variant(v) else { return Err(ParseError::NotAStructure) };
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
                let Value::Str(k) = super::unwrap_variant(k) else { return Err(ParseError::BadNode("property key")) };
                let owned = OwnedValue::try_from(val).map_err(|_| ParseError::BadNode("property value"))?;
                m.insert(k.as_str().to_string(), owned);
            }
            m
        }
        _ => return Err(ParseError::BadNode("properties")),
    };
    let children = match super::unwrap_variant(&fields[2]) {
        Value::Array(a) => a.iter().map(|c| OwnedValue::try_from(c).map_err(|_| ParseError::BadNode("child"))).collect::<Result<Vec<_>, _>>()?,
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
        let bytes: Vec<u8> = a.iter().filter_map(|v| if let Value::U8(b) = super::unwrap_variant(v) { Some(*b) } else { None }).collect();
        item.icon_data = (!bytes.is_empty()).then_some(bytes);
    }
    if let Some(Value::Array(a)) = prop(props, "shortcut") {
        // aas: take the first combination
        if let Some(Value::Array(first)) = a.iter().next().map(super::unwrap_variant) {
            let tokens: Vec<String> = first.iter().filter_map(|t| if let Value::Str(s) = super::unwrap_variant(t) { Some(s.as_str().to_string()) } else { None }).collect();
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
    let mut item = MenuItem { id: ItemId(*id), enabled: true, visible: true, ..Default::default() };
    apply_properties(&mut item, props);
    if item.separator {
        item.enabled = false;
    }
    for c in children {
        let raw = node_from_value(c)?;
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
pub fn apply_properties_update(root: &mut MenuItem, updated: &[(i32, HashMap<String, OwnedValue>)], removed: &[(i32, Vec<String>)]) {
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

/// A change reported by the service.
#[derive(Debug)]
pub enum MenuUpdate {
    /// The layout below `parent` changed; refetch it.
    Layout { revision: u32, parent: ItemId },
    Properties { updated: Vec<(i32, HashMap<String, OwnedValue>)>, removed: Vec<(i32, Vec<String>)> },
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
    let st = StructureBuilder::new().add_field(id).append_field(Value::Dict(dict)).append_field(Value::Array(array)).build()?;
    OwnedValue::try_from(Value::Structure(st))
}

/// Build a wire node from plain parts: `children` are nodes of the same shape.
pub fn make_node(id: i32, props: HashMap<String, OwnedValue>, children: Vec<RawNode>) -> zbus::zvariant::Result<RawNode> {
    let kids = children.into_iter().map(node_to_value).collect::<Result<Vec<_>, _>>()?;
    Ok((id, props, kids))
}

pub struct DBusMenuClient {
    proxy: DBusMenuProxy<'static>,
}

impl DBusMenuClient {
    pub async fn connect(conn: &Connection, service: &str, path: &str) -> zbus::Result<Self> {
        let proxy = DBusMenuProxy::builder(conn).destination(service.to_string())?.path(path.to_string())?.build().await?;
        Ok(Self { proxy })
    }

    pub fn proxy(&self) -> &DBusMenuProxy<'static> {
        &self.proxy
    }

    /// Fetch the layout below `parent` (0 for the root) down to `depth` levels (-1 = all).
    pub async fn fetch(&self, parent: ItemId, depth: i32) -> zbus::Result<(u32, MenuItem)> {
        let (revision, node) = self.proxy.get_layout(parent.0, depth, LAYOUT_PROPERTIES).await?;
        let item = parse_layout(&node).map_err(|e| zbus::Error::Failure(e.to_string()))?;
        Ok((revision, item))
    }

    /// `AboutToShow`; returns whether the layout changed and must be refetched.
    pub async fn about_to_show(&self, id: ItemId) -> zbus::Result<bool> {
        self.proxy.about_to_show(id.0).await
    }

    /// Send the `clicked` event for `id`.
    pub async fn activate(&self, id: ItemId, timestamp: u32) -> zbus::Result<()> {
        self.proxy.event(id.0, "clicked", &Value::from(0i32), timestamp).await
    }

    pub async fn hover(&self, id: ItemId, timestamp: u32) -> zbus::Result<()> {
        self.proxy.event(id.0, "hovered", &Value::from(0i32), timestamp).await
    }

    /// Tell the service a submenu was opened (`opened`) or closed (`closed`).
    pub async fn notify_open(&self, id: ItemId, open: bool, timestamp: u32) -> zbus::Result<()> {
        self.proxy.event(id.0, if open { "opened" } else { "closed" }, &Value::from(0i32), timestamp).await
    }

    /// Stream of change notifications from the service.
    pub async fn updates(&self) -> zbus::Result<impl Stream<Item = MenuUpdate> + Unpin> {
        let layout = self.proxy.receive_layout_updated().await?.filter_map(|s| async move {
            let a = s.args().ok()?;
            Some(MenuUpdate::Layout { revision: a.revision, parent: ItemId(a.parent) })
        });
        let props = self.proxy.receive_items_properties_updated().await?.filter_map(|s| async move {
            let a = s.args().ok()?;
            Some(MenuUpdate::Properties { updated: a.updated_props, removed: a.removed_props })
        });
        let act = self.proxy.receive_item_activation_requested().await?.filter_map(|s| async move {
            let a = s.args().ok()?;
            Some(MenuUpdate::ActivationRequested { id: ItemId(a.id), timestamp: a.timestamp })
        });
        Ok(futures_util::stream::select(futures_util::stream::select(layout, props), act).boxed())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn props(pairs: Vec<(&str, Value<'static>)>) -> HashMap<String, OwnedValue> {
        pairs.into_iter().map(|(k, v)| (k.to_string(), OwnedValue::try_from(v).unwrap())).collect()
    }

    fn sample() -> RawNode {
        let new = make_node(2, props(vec![("label", "_New".into()), ("shortcut", vec![vec!["Control", "N"]].into()), ("icon-name", "document-new".into())]), vec![]).unwrap();
        let sep = make_node(3, props(vec![("type", "separator".into())]), vec![]).unwrap();
        let check = make_node(4, props(vec![("label", "Show Hidden".into()), ("toggle-type", "checkmark".into()), ("toggle-state", 1i32.into())]), vec![]).unwrap();
        let radio = make_node(5, props(vec![("label", "Mode A".into()), ("toggle-type", "radio".into()), ("toggle-state", 0i32.into())]), vec![]).unwrap();
        let disabled = make_node(6, props(vec![("label", "Close".into()), ("enabled", false.into())]), vec![]).unwrap();
        let hidden = make_node(7, props(vec![("label", "Secret".into()), ("visible", false.into())]), vec![]).unwrap();
        let file = make_node(1, props(vec![("label", "_File".into()), ("children-display", "submenu".into())]), vec![new, sep, check, radio, disabled, hidden]).unwrap();
        let edit = make_node(8, props(vec![("label", "Edit".into()), ("children-display", "submenu".into())]), vec![]).unwrap();
        make_node(0, props(vec![("children-display", "submenu".into())]), vec![file, edit]).unwrap()
    }

    #[test]
    fn parses_nested_layouts_with_properties() {
        let root = parse_layout(&sample()).unwrap();
        assert_eq!(root.id, ItemId(0));
        assert_eq!(root.children.len(), 2);
        let file = &root.children[0];
        assert_eq!((file.label.as_str(), file.mnemonic, file.has_submenu), ("File", Some('F'), true));
        let new = &file.children[0];
        assert_eq!(new.label, "New");
        assert_eq!(new.shortcut.as_ref().unwrap().display(), "Ctrl+N");
        assert_eq!(new.icon_name.as_deref(), Some("document-new"));
        assert!(new.enabled && new.visible && !new.separator);
        let sep = &file.children[1];
        assert!(sep.separator && !sep.enabled);
        assert_eq!(file.children[2].toggle, Toggle::Check(true));
        assert_eq!(file.children[3].toggle, Toggle::Radio(false));
        assert!(!file.children[4].enabled);
        assert!(!file.children[5].visible);
        assert_eq!(file.visible_children().len(), 5, "the hidden item is skipped");
    }

    #[test]
    fn lazy_submenus_are_flagged_for_loading() {
        let root = parse_layout(&sample()).unwrap();
        let edit = &root.children[1];
        assert!(edit.has_submenu && edit.children.is_empty() && edit.needs_load());
        assert!(!root.children[0].needs_load());
    }

    #[test]
    fn indeterminate_and_unknown_toggles() {
        let mut item = MenuItem::new(1, "x");
        apply_properties(&mut item, &props(vec![("toggle-type", "checkmark".into()), ("toggle-state", (-1i32).into())]));
        assert_eq!(item.toggle, Toggle::Indeterminate);
        apply_properties(&mut item, &props(vec![("toggle-type", "".into())]));
        assert_eq!(item.toggle, Toggle::None);
        apply_properties(&mut item, &props(vec![("toggle-type", "radio".into()), ("toggle-state", 1i32.into())]));
        assert_eq!(item.toggle, Toggle::Radio(true));
        apply_properties(&mut item, &props(vec![("toggle-state", 0i32.into())]));
        assert_eq!(item.toggle, Toggle::Radio(false), "a state update keeps the toggle kind");
    }

    #[test]
    fn icon_data_is_copied() {
        let mut item = MenuItem::new(1, "x");
        apply_properties(&mut item, &props(vec![("icon-data", vec![137u8, 80, 78, 71].into())]));
        assert_eq!(item.icon_data.as_deref(), Some(&[137u8, 80, 78, 71][..]));
        apply_properties(&mut item, &props(vec![("icon-name", "".into())]));
        assert_eq!(item.icon_name, None);
    }

    #[test]
    fn property_updates_and_removals() {
        let mut root = parse_layout(&sample()).unwrap();
        let updated = vec![(2, props(vec![("label", "New _Tab".into()), ("enabled", false.into())])), (99, props(vec![("label", "ghost".into())]))];
        let removed = vec![(2, vec!["enabled".to_string(), "icon-name".to_string()]), (4, vec!["toggle-state".to_string()])];
        apply_properties_update(&mut root, &updated, &removed);
        let new = root.find(ItemId(2)).unwrap();
        assert_eq!(new.label, "New Tab");
        assert_eq!(new.mnemonic, Some('T'));
        assert!(new.enabled, "a removed property returns to its default");
        assert!(new.icon_name.is_none());
        assert_eq!(root.find(ItemId(4)).unwrap().toggle, Toggle::None);
        assert!(root.find(ItemId(99)).is_none());
    }

    #[test]
    fn subtrees_can_be_replaced() {
        let mut root = parse_layout(&sample()).unwrap();
        let fresh = MenuItem::submenu(8, "Edit", vec![MenuItem::new(9, "Undo")]);
        assert!(replace_subtree(&mut root, fresh));
        assert_eq!(root.children[1].children[0].label, "Undo");
        assert!(!replace_subtree(&mut root, MenuItem::new(1000, "x")));
    }

    #[test]
    fn malformed_children_are_errors() {
        let bad = (0, HashMap::new(), vec![OwnedValue::try_from(Value::from(5i32)).unwrap()]);
        assert!(matches!(parse_layout(&bad), Err(ParseError::NotAStructure)));
        let short = {
            use zbus::zvariant::StructureBuilder;
            let st = StructureBuilder::new().add_field(1i32).build().unwrap();
            OwnedValue::try_from(Value::Structure(st)).unwrap()
        };
        assert!(matches!(parse_layout(&(0, HashMap::new(), vec![short])), Err(ParseError::BadNode(_))));
    }

    #[test]
    fn deep_nesting_is_bounded() {
        let mut node = make_node(1000, HashMap::new(), vec![]).unwrap();
        for i in 0..40 {
            node = make_node(i, HashMap::new(), vec![node]).unwrap();
        }
        assert!(matches!(parse_layout(&node), Err(ParseError::TooDeep)));
    }

    #[test]
    fn node_values_roundtrip_through_the_wire_encoding() {
        let v = node_to_value(sample()).unwrap();
        let raw = node_from_value(&v).unwrap();
        assert_eq!(raw.0, 0);
        assert_eq!(raw.2.len(), 2);
        let root = parse_layout(&raw).unwrap();
        assert_eq!(root.children[0].children.len(), 6);
    }
}
