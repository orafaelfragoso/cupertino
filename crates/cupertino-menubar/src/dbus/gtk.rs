//! Reader for GMenuModel exported over D-Bus (`org.gtk.Menus` and `org.gtk.Actions`, open GNOME
//! interfaces).
//!
//! `org.gtk.Menus.Start(au) → a(uuaa{sv})` returns whole menus keyed by `(group, menu)`; an item
//! links to other menus through the `:section` and `:submenu` keys (each a `(uu)` pair). Sections are
//! flattened with separators between them. Item attributes used: `label`, `action`,
//! `action-namespace`, `target`, `hidden-when`, `accel`; an item keeps its full action name and its accelerator
//! text, which the bar shows through its modifier map. The application menu (`app_menu_object_path`) is read next
//! to the menu bar and leads the tree under [`GTK_APP_MENU`]. Action state and enabled flags come from
//! `org.gtk.Actions.DescribeAll`; activation calls `org.gtk.Actions.Activate` on the object that
//! owns the action (`app.` actions on the application object, `win.` actions on the window object).
//! Both interfaces' `Changed` signals keep the cache current ([`GtkMenuClient::changes`]); records for menus never
//! loaded are ignored, and a menu keeps at most [`MAX_MENU_ITEMS`] items, since any application can send them.

use std::collections::{HashMap, HashSet};

use futures_util::stream::BoxStream;
use futures_util::StreamExt;
use zbus::zvariant::{OwnedValue, Signature, Value};
use zbus::Connection;

use crate::item::{split_gtk_mnemonic, ItemId, MenuItem, Toggle};
use crate::merge::GTK_APP_MENU;
use crate::source::GtkMenuProps;

pub type RawItems = Vec<HashMap<String, OwnedValue>>;
/// Reply item of `Start`: `(group, menu, items)`.
pub type RawMenu = (u32, u32, RawItems);
/// Change record of the `Changed` signal: `(group, menu, position, removed, added)`.
pub type RawChange = (u32, u32, u32, u32, RawItems);
/// `DescribeAll` value: `(enabled, parameter type, state)`.
pub type RawAction = (bool, Signature, Vec<OwnedValue>);

/// The most items a menu of an exporter keeps; the rest are dropped.
pub const MAX_MENU_ITEMS: usize = 1000;
/// The most entries an exporter's tree is built from, links included: links that share or repeat menus could
/// otherwise multiply the work and the items.
pub const MAX_TREE_ITEMS: usize = 5000;
/// The most actions the bar keeps of an exporter; further ones are not added.
pub const MAX_ACTIONS: usize = 10_000;

/// Which `org.gtk.Menus` object of an exporter a change is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuObject {
    MenuBar,
    AppMenu,
}

/// A `Changed` signal of an exporter.
#[derive(Debug)]
pub enum GtkChange {
    Menus(MenuObject, Vec<RawChange>),
    /// The actions of the `app.` or `win.` object: removed, enabled changed, state changed, added.
    Actions {
        scope: &'static str,
        removed: Vec<String>,
        enabled: HashMap<String, bool>,
        state: HashMap<String, OwnedValue>,
        added: HashMap<String, RawAction>,
    },
}

/// Proxy for `org.gtk.Menus` (a module of its own: the generated signal types would otherwise
/// collide with those of `org.gtk.Actions`).
pub mod menus_iface {
    use super::{RawChange, RawMenu};
    use zbus::proxy;

    #[proxy(interface = "org.gtk.Menus", assume_defaults = false)]
    pub trait GtkMenus {
        fn start(&self, subscriptions: &[u32]) -> zbus::Result<Vec<RawMenu>>;

        fn end(&self, subscriptions: &[u32]) -> zbus::Result<()>;

        #[zbus(signal)]
        fn changed(&self, changes: Vec<RawChange>) -> zbus::Result<()>;
    }
}

/// Proxy for `org.gtk.Actions`.
pub mod actions_iface {
    use super::RawAction;
    use std::collections::HashMap;
    use zbus::proxy;
    use zbus::zvariant::{OwnedValue, Value};

    #[proxy(interface = "org.gtk.Actions", assume_defaults = false)]
    pub trait GtkActions {
        fn describe_all(&self) -> zbus::Result<HashMap<String, RawAction>>;

        fn activate(
            &self,
            action_name: &str,
            parameter: &[Value<'_>],
            platform_data: HashMap<&str, Value<'_>>,
        ) -> zbus::Result<()>;

        #[zbus(signal)]
        fn changed(
            &self,
            removed: Vec<String>,
            enabled_changed: HashMap<String, bool>,
            state_changed: HashMap<String, OwnedValue>,
            added: HashMap<String, RawAction>,
        ) -> zbus::Result<()>;
    }
}

pub use actions_iface::GtkActionsProxy;
pub use menus_iface::GtkMenusProxy;

/// The cached menus of one exporter.
#[derive(Default)]
pub struct GtkMenus {
    menus: HashMap<(u32, u32), RawItems>,
}

impl GtkMenus {
    pub fn from_reply(reply: Vec<RawMenu>) -> Self {
        let mut m = Self::default();
        m.merge_reply(reply);
        m
    }

    pub fn merge_reply(&mut self, reply: Vec<RawMenu>) {
        for (g, m, mut items) in reply {
            items.truncate(MAX_MENU_ITEMS);
            self.menus.insert((g, m), items);
        }
    }

    /// Apply one record of the `Changed` signal: replace `removed` items at `position` with `added`. A record for a
    /// menu that was never loaded is ignored.
    pub fn apply_change(&mut self, change: RawChange) {
        let (g, m, position, removed, added) = change;
        let Some(list) = self.menus.get_mut(&(g, m)) else {
            return;
        };
        let pos = (position as usize).min(list.len());
        let end = pos.saturating_add(removed as usize).min(list.len());
        list.splice(pos..end, added);
        list.truncate(MAX_MENU_ITEMS);
    }

    pub fn get(&self, group: u32, menu: u32) -> Option<&RawItems> {
        self.menus.get(&(group, menu))
    }

    /// Menu references reachable from `root` whose group has not been loaded.
    pub fn missing_groups(&self, root: (u32, u32)) -> Vec<u32> {
        let mut seen = HashSet::new();
        let mut missing = Vec::new();
        let mut stack = vec![root];
        while let Some(key) = stack.pop() {
            if !seen.insert(key) {
                continue;
            }
            match self.menus.get(&key) {
                Some(items) => {
                    for it in items {
                        for k in [":section", ":submenu"] {
                            if let Some(r) = link(it, k) {
                                stack.push(r);
                            }
                        }
                    }
                }
                None if !missing.contains(&key.0) => missing.push(key.0),
                None => {}
            }
        }
        missing.sort_unstable();
        missing
    }
}

fn link(item: &HashMap<String, OwnedValue>, key: &str) -> Option<(u32, u32)> {
    let v = super::prop(item, key)?;
    let Value::Structure(s) = v else { return None };
    match s.fields() {
        [a, b] => match (super::unwrap_variant(a), super::unwrap_variant(b)) {
            (Value::U32(g), Value::U32(m)) => Some((*g, *m)),
            _ => None,
        },
        _ => None,
    }
}

#[derive(Clone, Debug)]
pub struct GtkActionState {
    pub enabled: bool,
    pub state: Option<OwnedValue>,
}

impl GtkActionState {
    pub fn from_raw(raw: &RawAction) -> Self {
        Self {
            enabled: raw.0,
            state: raw.2.first().and_then(|v| v.try_clone().ok()),
        }
    }
}

/// Keep the action `name`, unless the exporter has [`MAX_ACTIONS`] already: a known one is updated.
fn add_action(states: &mut HashMap<String, GtkActionState>, name: String, raw: &RawAction) {
    if states.len() < MAX_ACTIONS || states.contains_key(&name) {
        states.insert(name, GtkActionState::from_raw(raw));
    }
}

/// What an item activates.
#[derive(Debug)]
pub struct GtkActionRef {
    /// Full action name including the scope prefix, for example `app.quit` or `win.close`.
    pub action: String,
    pub target: Option<OwnedValue>,
}

pub struct GtkBuild {
    pub items: Vec<MenuItem>,
    pub actions: HashMap<ItemId, GtkActionRef>,
}

struct Builder<'a> {
    menus: &'a GtkMenus,
    actions: &'a HashMap<String, GtkActionState>,
    next_id: i32,
    refs: HashMap<ItemId, GtkActionRef>,
    /// The menus being built, outermost first: a link back to one of them builds nothing.
    path: Vec<(u32, u32)>,
    /// Entries looked at so far, of every menu.
    examined: usize,
}

impl Builder<'_> {
    fn new_id(&mut self) -> i32 {
        self.next_id += 1;
        self.next_id
    }

    fn build(&mut self, key: (u32, u32), namespace: &str) -> Vec<MenuItem> {
        if self.path.len() > 16 || self.path.contains(&key) {
            return Vec::new();
        }
        let mut out: Vec<MenuItem> = Vec::new();
        let Some(items) = self.menus.get(key.0, key.1) else {
            return out;
        };
        self.path.push(key);
        for it in items {
            // the budget of the whole tree, spent by links as well as items
            self.examined += 1;
            if self.examined > MAX_TREE_ITEMS {
                break;
            }
            let ns = match super::prop_str(it, "action-namespace") {
                Some(n) => format!("{namespace}{n}."),
                None => namespace.to_string(),
            };
            if let Some(sec) = link(it, ":section") {
                let inner = self.build(sec, &ns);
                if inner.is_empty() {
                    continue;
                }
                if !out.is_empty() {
                    let id = self.new_id();
                    out.push(MenuItem::separator(id));
                }
                out.extend(inner);
                continue;
            }
            let label = super::prop_str(it, "label").unwrap_or_default();
            if let Some(sub) = link(it, ":submenu") {
                let id = self.new_id();
                let children = self.build(sub, &ns);
                let mut item = MenuItem::submenu(id, "", children);
                (item.label, item.mnemonic) = split_gtk_mnemonic(&label);
                out.push(item);
                continue;
            }
            let action = super::prop_str(it, "action");
            if label.is_empty() && action.is_none() {
                continue;
            }
            let full = action.as_ref().map(|a| {
                if a.contains('.') && ns.is_empty() {
                    a.clone()
                } else {
                    format!("{ns}{a}")
                }
            });
            let state = full.as_ref().and_then(|a| self.actions.get(a));
            match (super::prop_str(it, "hidden-when").as_deref(), &full, state) {
                (Some("action-disabled"), _, Some(s)) if !s.enabled => continue,
                (Some("action-missing"), Some(_), None) => continue,
                _ => {}
            }
            let id = self.new_id();
            let mut item = MenuItem::new(id, "");
            let (l, m) = split_gtk_mnemonic(&label);
            item.label = l;
            item.mnemonic = m;
            item.enabled = match (&full, state) {
                (Some(_), Some(s)) => s.enabled,
                (Some(_), None) => false,
                (None, _) => true,
            };
            let target = it.get("target").and_then(|v| v.try_clone().ok());
            if let Some(s) = state.and_then(|s| s.state.as_ref()) {
                item.toggle = match (
                    super::unwrap_variant(s),
                    target.as_ref().map(|t| super::unwrap_variant(t)),
                ) {
                    (Value::Bool(b), None) => Toggle::Check(*b),
                    (Value::Str(a), Some(Value::Str(b))) => Toggle::Radio(a.as_str() == b.as_str()),
                    (Value::I32(a), Some(Value::I32(b))) => Toggle::Radio(a == b),
                    _ => Toggle::None,
                };
            }
            item.accel = super::prop_str(it, "accel");
            item.icon_name = super::prop_str(it, "icon").filter(|name| !name.is_empty());
            if let Some(a) = full {
                item.action = Some(a.clone());
                self.refs
                    .insert(ItemId(id), GtkActionRef { action: a, target });
            }
            out.push(item);
        }
        while out.last().is_some_and(|l| l.separator) {
            out.pop();
        }
        self.path.pop();
        out
    }
}

/// Build the item tree below `root` (the menu bar is `(0, 0)`). `actions` is keyed by full action
/// name (`app.quit`, `win.close`).
pub fn build_menu(
    menus: &GtkMenus,
    root: (u32, u32),
    actions: &HashMap<String, GtkActionState>,
) -> GtkBuild {
    let mut b = Builder {
        menus,
        actions,
        next_id: 0,
        refs: HashMap::new(),
        path: Vec::new(),
        examined: 0,
    };
    let items = b.build(root, "");
    GtkBuild {
        items: items.into_iter().map(MenuItem::exported).collect(),
        actions: b.refs,
    }
}

/// The tree of an exporter: the menus of its menu bar, led by the application menu under [`GTK_APP_MENU`] when that
/// has items. Ids are numbered across both from 1, the menu bar's first, so they do not depend on the application
/// menu and never reach the bar's own ids (all negative).
pub fn build_menus(
    menubar: &GtkMenus,
    app_menu: &GtkMenus,
    actions: &HashMap<String, GtkActionState>,
) -> GtkBuild {
    let mut b = Builder {
        menus: menubar,
        actions,
        next_id: 0,
        refs: HashMap::new(),
        path: Vec::new(),
        examined: 0,
    };
    let mut items = b.build((0, 0), "");
    b.menus = app_menu;
    let app = b.build((0, 0), "");
    if !app.is_empty() {
        items.insert(0, MenuItem::submenu(GTK_APP_MENU.0, "", app));
    }
    GtkBuild {
        items: items.into_iter().map(MenuItem::exported).collect(),
        actions: b.refs,
    }
}

/// A connected exporter.
pub struct GtkMenuClient {
    conn: Connection,
    props: GtkMenuProps,
    menus: GtkMenus,
    app_menu: GtkMenus,
    action_states: HashMap<String, GtkActionState>,
    refs: HashMap<ItemId, GtkActionRef>,
}

impl GtkMenuClient {
    /// Load the menu bar, the application menu and the action states of the window and application objects.
    pub async fn load(
        conn: &Connection,
        props: &GtkMenuProps,
    ) -> zbus::Result<(Self, Vec<MenuItem>)> {
        let mut c = Self {
            conn: conn.clone(),
            props: props.clone(),
            menus: GtkMenus::default(),
            app_menu: GtkMenus::default(),
            action_states: HashMap::new(),
            refs: HashMap::new(),
        };
        let items = c.reload().await?;
        Ok((c, items))
    }

    async fn menus_proxy(&self, path: &str) -> zbus::Result<GtkMenusProxy<'static>> {
        GtkMenusProxy::builder(&self.conn)
            .destination(self.props.bus_name.clone())?
            .path(path.to_string())?
            .build()
            .await
    }

    async fn actions_proxy(&self, path: &str) -> zbus::Result<GtkActionsProxy<'static>> {
        GtkActionsProxy::builder(&self.conn)
            .destination(self.props.bus_name.clone())?
            .path(path.to_string())?
            .build()
            .await
    }

    /// The menus of the `org.gtk.Menus` object at `path` reachable from `(0, 0)`; none without a path.
    async fn load_menus(&self, path: Option<&str>) -> zbus::Result<GtkMenus> {
        let Some(path) = path else {
            return Ok(GtkMenus::default());
        };
        let menus = self.menus_proxy(path).await?;
        let mut cache = GtkMenus::from_reply(menus.start(&[0]).await?);
        let mut subscribed: HashSet<u32> = HashSet::from([0]);
        for _ in 0..8 {
            let missing: Vec<u32> = cache
                .missing_groups((0, 0))
                .into_iter()
                .filter(|g| !subscribed.contains(g))
                .collect();
            if missing.is_empty() {
                break;
            }
            cache.merge_reply(menus.start(&missing).await?);
            subscribed.extend(missing);
        }
        Ok(cache)
    }

    /// Fetch everything again and rebuild the tree.
    pub async fn reload(&mut self) -> zbus::Result<Vec<MenuItem>> {
        self.menus = self
            .load_menus(self.props.menubar_object_path.as_deref())
            .await?;
        self.app_menu = self
            .load_menus(self.props.app_menu_object_path.as_deref())
            .await?;
        self.action_states.clear();
        for (scope, path) in [
            ("app", self.props.application_object_path.clone()),
            ("win", self.props.window_object_path.clone()),
        ] {
            let Some(path) = path else { continue };
            if let Ok(proxy) = self.actions_proxy(&path).await {
                if let Ok(all) = proxy.describe_all().await {
                    for (name, raw) in all {
                        add_action(&mut self.action_states, format!("{scope}.{name}"), &raw);
                    }
                }
            }
        }
        Ok(self.rebuild())
    }

    fn rebuild(&mut self) -> Vec<MenuItem> {
        let built = build_menus(&self.menus, &self.app_menu, &self.action_states);
        self.refs = built.actions;
        built.items
    }

    /// What activating `item` calls; `None` for an item without an action.
    pub fn activation(&self, item: ItemId) -> Option<GtkActivation> {
        let r = self.refs.get(&item)?;
        let (scope, name) = r.action.split_once('.').unwrap_or(("app", &r.action));
        let path = match scope {
            "win" => self.props.window_object_path.clone(),
            _ => self.props.application_object_path.clone(),
        }?;
        let params: Vec<Value<'static>> = r
            .target
            .as_ref()
            .and_then(|t| t.try_clone().ok())
            .map(|t| match Value::from(t) {
                Value::Value(inner) => vec![*inner],
                other => vec![other],
            })
            .unwrap_or_default();
        Some(GtkActivation {
            conn: self.conn.clone(),
            bus_name: self.props.bus_name.clone(),
            path,
            name: name.to_string(),
            params,
        })
    }

    /// Activate the action behind `item`. Returns `false` for items without an action.
    pub async fn activate(&self, item: ItemId) -> zbus::Result<bool> {
        match self.activation(item) {
            Some(activation) => activation.run().await.map(|()| true),
            None => Ok(false),
        }
    }

    pub fn menus(&self) -> &GtkMenus {
        &self.menus
    }

    /// The `Changed` signals of the exporter's menus and actions, as one stream. Subscribe before [`Self::load`], so
    /// no change made while the menus load is missed.
    pub async fn changes(
        conn: &Connection,
        props: &GtkMenuProps,
    ) -> zbus::Result<BoxStream<'static, GtkChange>> {
        let menus = |path: String| {
            GtkMenusProxy::builder(conn)
                .destination(props.bus_name.clone())
                .and_then(|b| b.path(path))
        };
        let actions = |path: String| {
            GtkActionsProxy::builder(conn)
                .destination(props.bus_name.clone())
                .and_then(|b| b.path(path))
        };
        let mut streams: Vec<BoxStream<'static, GtkChange>> = Vec::new();
        for (object, path) in [
            (MenuObject::MenuBar, &props.menubar_object_path),
            (MenuObject::AppMenu, &props.app_menu_object_path),
        ] {
            let Some(path) = path.clone() else { continue };
            let changed = menus(path)?.build().await?.receive_changed().await?;
            streams.push(
                changed
                    .filter_map(move |signal| async move {
                        let args = signal.args().ok()?;
                        Some(GtkChange::Menus(object, args.changes))
                    })
                    .boxed(),
            );
        }
        for (scope, path) in [
            ("app", &props.application_object_path),
            ("win", &props.window_object_path),
        ] {
            let Some(path) = path.clone() else { continue };
            let changed = actions(path)?.build().await?.receive_changed().await?;
            streams.push(
                changed
                    .filter_map(move |signal| async move {
                        let args = signal.args().ok()?;
                        Some(GtkChange::Actions {
                            scope,
                            removed: args.removed,
                            enabled: args.enabled_changed,
                            state: args.state_changed,
                            added: args.added,
                        })
                    })
                    .boxed(),
            );
        }
        Ok(futures_util::stream::select_all(streams).boxed())
    }

    /// Take a change of the exporter into the cache; [`Self::items`] then builds the tree from it.
    pub fn apply(&mut self, change: GtkChange) {
        match change {
            GtkChange::Menus(object, records) => {
                let menus = match object {
                    MenuObject::MenuBar => &mut self.menus,
                    MenuObject::AppMenu => &mut self.app_menu,
                };
                for record in records {
                    menus.apply_change(record);
                }
            }
            GtkChange::Actions {
                scope,
                removed,
                enabled,
                state,
                added,
            } => {
                for name in removed {
                    self.action_states.remove(&format!("{scope}.{name}"));
                }
                for (name, raw) in added {
                    add_action(&mut self.action_states, format!("{scope}.{name}"), &raw);
                }
                for (name, on) in enabled {
                    if let Some(action) = self.action_states.get_mut(&format!("{scope}.{name}")) {
                        action.enabled = on;
                    }
                }
                for (name, value) in state {
                    if let Some(action) = self.action_states.get_mut(&format!("{scope}.{name}")) {
                        action.state = Some(value);
                    }
                }
            }
        }
    }

    /// The tree of the cached menus and actions.
    pub fn items(&mut self) -> Vec<MenuItem> {
        self.rebuild()
    }
}

/// An item's activation, made apart from its client: the client stays with the worker meanwhile.
pub struct GtkActivation {
    conn: Connection,
    bus_name: String,
    path: String,
    name: String,
    params: Vec<Value<'static>>,
}

impl GtkActivation {
    pub async fn run(self) -> zbus::Result<()> {
        let proxy = GtkActionsProxy::builder(&self.conn)
            .destination(self.bus_name)?
            .path(self.path)?
            .build()
            .await?;
        proxy
            .activate(&self.name, &self.params, HashMap::new())
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v<T: Into<Value<'static>>>(x: T) -> OwnedValue {
        OwnedValue::try_from(x.into()).unwrap()
    }

    fn item(pairs: Vec<(&str, OwnedValue)>) -> HashMap<String, OwnedValue> {
        pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
    }

    fn pair(g: u32, m: u32) -> OwnedValue {
        v(Value::from((g, m)))
    }

    fn sample() -> GtkMenus {
        GtkMenus::from_reply(vec![
            (
                0,
                0,
                vec![
                    item(vec![("label", v("_File & Edit")), (":submenu", pair(0, 1))]),
                    item(vec![("label", v("View")), (":submenu", pair(0, 2))]),
                ],
            ),
            (
                0,
                1,
                vec![
                    item(vec![(":section", pair(0, 3))]),
                    item(vec![(":section", pair(0, 4))]),
                    item(vec![(":section", pair(0, 5))]),
                ],
            ),
            (
                0,
                2,
                vec![
                    item(vec![
                        ("label", v("Show Hidden")),
                        ("action", v("win.hidden")),
                    ]),
                    item(vec![
                        ("label", v("Sort by name")),
                        ("action", v("win.sort")),
                        ("target", v("name")),
                    ]),
                    item(vec![
                        ("label", v("Sort by size")),
                        ("action", v("win.sort")),
                        ("target", v("size")),
                    ]),
                ],
            ),
            (
                0,
                3,
                vec![
                    item(vec![
                        ("label", v("_New Window")),
                        ("action", v("app.new")),
                        ("accel", v("<Primary>n")),
                    ]),
                    item(vec![
                        ("label", v("Close & Save")),
                        ("action", v("win.close")),
                        ("accel", v("<Primary><Shift>w")),
                    ]),
                ],
            ),
            (0, 4, vec![]),
            (
                0,
                5,
                vec![
                    item(vec![
                        ("label", v("Quit")),
                        ("action", v("app.quit")),
                        ("accel", v("<Primary>q")),
                    ]),
                    item(vec![
                        ("label", v("Only in debug")),
                        ("action", v("app.debug")),
                        ("hidden-when", v("action-missing")),
                    ]),
                ],
            ),
        ])
    }

    fn actions() -> HashMap<String, GtkActionState> {
        let mut m = HashMap::new();
        m.insert(
            "app.new".to_string(),
            GtkActionState {
                enabled: true,
                state: None,
            },
        );
        m.insert(
            "app.quit".to_string(),
            GtkActionState {
                enabled: true,
                state: None,
            },
        );
        m.insert(
            "win.close".to_string(),
            GtkActionState {
                enabled: false,
                state: None,
            },
        );
        m.insert(
            "win.hidden".to_string(),
            GtkActionState {
                enabled: true,
                state: Some(v(true)),
            },
        );
        m.insert(
            "win.sort".to_string(),
            GtkActionState {
                enabled: true,
                state: Some(v("size")),
            },
        );
        m
    }

    #[test]
    fn builds_the_tree_with_flattened_sections() {
        let b = build_menu(&sample(), (0, 0), &actions());
        assert_eq!(b.items.len(), 2);
        let file = &b.items[0];
        assert_eq!(
            (file.label.as_str(), file.mnemonic, file.has_submenu),
            ("File & Edit", Some('F'), true)
        );
        let labels: Vec<&str> = file.children.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(
            labels,
            vec!["New Window", "Close & Save", "", "Quit"],
            "empty sections vanish, others are separated, the action-missing item is hidden, & is plain text"
        );
        assert!(file.children[2].separator);
        assert_eq!(
            (
                file.children[0].accel.as_deref(),
                file.children[1].accel.as_deref()
            ),
            (Some("<Primary>n"), Some("<Primary><Shift>w")),
            "the accelerator text is kept for the bar's modifier map"
        );
        assert_eq!(file.children[1].action.as_deref(), Some("win.close"));
        assert!(
            file.children[0].enabled && !file.children[1].enabled,
            "enabled flags come from the actions"
        );
    }

    #[test]
    fn toggles_come_from_action_state_and_targets() {
        let b = build_menu(&sample(), (0, 0), &actions());
        let view = &b.items[1];
        assert_eq!(view.children[0].toggle, Toggle::Check(true));
        assert_eq!(view.children[1].toggle, Toggle::Radio(false));
        assert_eq!(view.children[2].toggle, Toggle::Radio(true));
    }

    #[test]
    fn items_remember_their_actions() {
        let b = build_menu(&sample(), (0, 0), &actions());
        let new = &b.items[0].children[0];
        let r = &b.actions[&new.id];
        assert_eq!(r.action, "app.new");
        assert!(r.target.is_none());
        let sort = &b.items[1].children[2];
        let r = &b.actions[&sort.id];
        assert_eq!(r.action, "win.sort");
        assert!(r.target.is_some());
        assert!(
            !b.actions.contains_key(&b.items[0].id),
            "submenu items have no action"
        );
    }

    #[test]
    fn unknown_actions_are_disabled() {
        let b = build_menu(&sample(), (0, 0), &HashMap::new());
        let file = &b.items[0];
        assert!(file
            .children
            .iter()
            .filter(|c| !c.separator)
            .all(|c| !c.enabled));
        assert_eq!(
            file.children.iter().filter(|c| !c.separator).count(),
            3,
            "only the action-missing item vanishes"
        );
    }

    #[test]
    fn missing_groups_are_discovered() {
        let mut m = GtkMenus::from_reply(vec![(
            0,
            0,
            vec![
                item(vec![("label", v("A")), (":submenu", pair(1, 0))]),
                item(vec![("label", v("B")), (":section", pair(2, 3))]),
            ],
        )]);
        assert_eq!(m.missing_groups((0, 0)), vec![1, 2]);
        m.merge_reply(vec![
            (1, 0, vec![]),
            (
                2,
                3,
                vec![item(vec![("label", v("x")), (":submenu", pair(0, 0))])],
            ),
        ]);
        assert!(m.missing_groups((0, 0)).is_empty(), "cycles terminate");
    }

    #[test]
    fn changed_records_splice_items() {
        let mut m = sample();
        let before = m.get(0, 2).unwrap().len();
        m.apply_change((
            0,
            2,
            1,
            1,
            vec![
                item(vec![
                    ("label", v("Sort by date")),
                    ("action", v("win.sort")),
                    ("target", v("date")),
                ]),
                item(vec![("label", v("Extra")), ("action", v("win.x"))]),
            ],
        ));
        assert_eq!(m.get(0, 2).unwrap().len(), before + 1);
        let b = build_menu(&m, (0, 0), &actions());
        let labels: Vec<&str> = b.items[1]
            .children
            .iter()
            .map(|c| c.label.as_str())
            .collect();
        assert_eq!(
            labels,
            vec!["Show Hidden", "Sort by date", "Extra", "Sort by size"]
        );
        let len = m.get(0, 2).unwrap().len();
        m.apply_change((0, 2, 99, u32::MAX, vec![item(vec![("label", v("late"))])]));
        assert_eq!(m.get(0, 2).unwrap().len(), len + 1, "positions are clamped");
        m.apply_change((0, 9, 0, 0, vec![item(vec![("label", v("stray"))])]));
        assert!(
            m.get(0, 9).is_none(),
            "a menu that was never loaded is not made"
        );
    }

    /// Every item of the tree, the submenus' too.
    fn count(items: &[MenuItem]) -> usize {
        items.iter().map(|i| 1 + count(&i.children)).sum()
    }

    #[test]
    fn links_back_to_a_menu_being_built_build_nothing() {
        // each item sections the menu it is in
        let looping = (0..MAX_MENU_ITEMS)
            .map(|_| item(vec![("label", v("x")), (":section", pair(0, 0))]))
            .collect();
        let m = GtkMenus::from_reply(vec![(0, 0, looping)]);
        let started = std::time::Instant::now();
        let built = build_menu(&m, (0, 0), &actions());
        assert!(count(&built.items) <= MAX_TREE_ITEMS);
        assert!(started.elapsed() < std::time::Duration::from_secs(1));

        // two menus that open each other
        let m = GtkMenus::from_reply(vec![
            (
                0,
                0,
                vec![item(vec![("label", v("A")), (":submenu", pair(0, 1))])],
            ),
            (
                0,
                1,
                vec![item(vec![("label", v("B")), (":submenu", pair(0, 0))])],
            ),
        ]);
        let built = build_menu(&m, (0, 0), &actions());
        assert_eq!(
            count(&built.items),
            2,
            "A, then B, whose link back to A builds nothing"
        );
    }

    #[test]
    fn a_tree_is_built_with_at_most_its_budget_of_items() {
        // no cycle, but every item opens the same full menu: a million items
        let opening = (0..MAX_MENU_ITEMS)
            .map(|_| item(vec![("label", v("x")), (":submenu", pair(0, 1))]))
            .collect();
        let full = (0..MAX_MENU_ITEMS)
            .map(|_| item(vec![("label", v("y"))]))
            .collect();
        let m = GtkMenus::from_reply(vec![(0, 0, opening), (0, 1, full)]);
        let started = std::time::Instant::now();
        let built = build_menu(&m, (0, 0), &actions());
        assert!(
            count(&built.items) <= MAX_TREE_ITEMS,
            "{}",
            count(&built.items)
        );
        assert!(started.elapsed() < std::time::Duration::from_secs(1));

        // no cycle and no item: sixteen menus whose every entry sections the next, a thousand to the sixteenth links
        let chain = (0..16)
            .map(|m| {
                let links = (0..MAX_MENU_ITEMS)
                    .map(|_| item(vec![(":section", pair(0, m + 1))]))
                    .collect();
                (0, m, links)
            })
            .collect();
        let started = std::time::Instant::now();
        let built = build_menu(&GtkMenus::from_reply(chain), (0, 0), &actions());
        assert!(built.items.is_empty());
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
    }

    #[test]
    fn an_exporter_keeps_at_most_its_cap_of_actions() {
        let raw: RawAction = (true, Signature::Unit, Vec::new());
        let mut states = HashMap::new();
        for i in 0..MAX_ACTIONS + 10 {
            add_action(&mut states, format!("app.a{i}"), &raw);
        }
        assert_eq!(states.len(), MAX_ACTIONS);
        // a known action is still updated
        add_action(
            &mut states,
            "app.a0".into(),
            &(false, Signature::Unit, Vec::new()),
        );
        assert!(!states["app.a0"].enabled);
    }

    #[test]
    fn a_menu_keeps_at_most_its_cap_of_items() {
        let many = || {
            (0..MAX_MENU_ITEMS + 5)
                .map(|_| item(vec![("label", v("x"))]))
                .collect()
        };
        let mut m = GtkMenus::from_reply(vec![(0, 0, many())]);
        assert_eq!(m.get(0, 0).unwrap().len(), MAX_MENU_ITEMS);
        m.apply_change((0, 0, 0, 0, many()));
        assert_eq!(m.get(0, 0).unwrap().len(), MAX_MENU_ITEMS);
    }

    #[test]
    fn namespaces_prefix_relative_actions() {
        let m = GtkMenus::from_reply(vec![
            (
                0,
                0,
                vec![item(vec![
                    ("label", v("Tools")),
                    (":submenu", pair(0, 1)),
                    ("action-namespace", v("win")),
                ])],
            ),
            (
                0,
                1,
                vec![item(vec![("label", v("Run")), ("action", v("run"))])],
            ),
        ]);
        let mut a = HashMap::new();
        a.insert(
            "win.run".to_string(),
            GtkActionState {
                enabled: true,
                state: None,
            },
        );
        let b = build_menu(&m, (0, 0), &a);
        let run = &b.items[0].children[0];
        assert!(run.enabled);
        assert_eq!(b.actions[&run.id].action, "win.run");
    }

    #[test]
    fn the_application_menu_leads_the_tree_without_moving_the_menu_bars_ids() {
        let app_menu = GtkMenus::from_reply(vec![(
            0,
            0,
            vec![
                item(vec![("label", v("_About")), ("action", v("app.about"))]),
                item(vec![("label", v("Quit")), ("action", v("app.quit"))]),
            ],
        )]);
        let alone = build_menu(&sample(), (0, 0), &actions());
        let b = build_menus(&sample(), &app_menu, &actions());
        assert_eq!(b.items[0].id, GTK_APP_MENU);
        assert_eq!(&b.items[1..], &alone.items[..]);
        let about = &b.items[0].children[0];
        assert_eq!(
            (about.label.as_str(), about.action.as_deref()),
            ("About", Some("app.about"))
        );
        assert_eq!(b.actions[&about.id].action, "app.about");
        assert!(about.id.0 > alone.actions.keys().map(|i| i.0).max().unwrap());

        let only = build_menus(&GtkMenus::default(), &app_menu, &actions());
        assert_eq!(
            only.items.len(),
            1,
            "an application menu without a menu bar"
        );
        let none = build_menus(&sample(), &GtkMenus::default(), &actions());
        assert_eq!(none.items, alone.items);
    }
}
