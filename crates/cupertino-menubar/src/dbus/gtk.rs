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
