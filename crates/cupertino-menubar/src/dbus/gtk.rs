//! Reader for GMenuModel exported over D-Bus (`org.gtk.Menus` and `org.gtk.Actions`, open GNOME
//! interfaces).
//!
//! `org.gtk.Menus.Start(au) → a(uuaa{sv})` subscribes to groups of menus and returns their menus keyed by
//! `(group, menu)`; `End(au)` ends a subscription, and the exporter sends changes only for the groups subscribed. An
//! item links to other menus through the `:section` and `:submenu` keys (each a `(uu)` pair): a section is a menu of the
//! same group, a submenu usually one of a group of its own. Sections are flattened with separators between them. Item
//! attributes used: `label` (`_` marks the mnemonic, `__` is an underscore), `action`, `action-namespace` (on a link: it
//! prefixes the actions of the linked menu), `target`, `hidden-when` (`action-missing`, `action-disabled`, and
//! `macos-menubar`, which a global menu bar hides always), `accel` (kept as text, which the bar shows through its
//! modifier map), `icon` and `verb-icon` (a serialized icon: a name, a themed icon's names or an image's bytes). The
//! application menu (`app_menu_object_path`) is read next to the menu bar and leads the tree under [`GTK_APP_MENU`].
//!
//! An item's action is found as a menu tracker finds it: the action of its full name (`app.` actions are the
//! application object's, `win.` actions the window object's, any other prefix names nothing here) must exist and take
//! a parameter exactly when the item has a target, of the target's type; else the item is disabled. A stateful action
//! with a target shows a radio mark, checked when its state equals the target; a boolean one without a target a check
//! mark. Action state and enabled flags come from `org.gtk.Actions.DescribeAll`; activation calls
//! `org.gtk.Actions.Activate` on the object that owns the action, with the item's target and an activation token in
//! its platform data. Both interfaces' `Changed` signals keep the cache current ([`GtkMenuClient::changes`]): change
//! records apply to the groups subscribed after their `Start` reply, a menu of such a group that a record only inserts
//! into is created and one left empty dropped; groups that come into the tree are subscribed and those that leave it
//! ended ([`GtkMenuClient::subscription`]). A menu keeps at most [`MAX_MENU_ITEMS`] items, an object at most
//! [`MAX_MENUS`] menus and [`MAX_GROUPS`] groups, and a signal applies at most [`MAX_SIGNAL_RECORDS`] records, since any
//! application can send them.

use std::collections::{HashMap, HashSet};

use zbus::zvariant::{OwnedValue, Signature, Value};

use crate::item::{split_gtk_mnemonic, ItemId, MenuItem, Toggle};
use crate::merge::GTK_APP_MENU;

mod client;

pub use client::{GtkActivation, GtkMenuClient, GtkStarted, GtkSubscription};

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
/// The most groups the bar subscribes to on one menu object of an exporter: groups further down the tree are not
/// subscribed, so an exporter that keeps adding submenus cannot make the bar subscribe without end.
pub const MAX_GROUPS: usize = 256;
/// The most change records kept for groups whose `Start` reply has not come yet; later ones are dropped.
pub const MAX_HELD_RECORDS: usize = 1000;
/// The most menus the bar keeps of one menu object of an exporter, sections included: a `Start` reply's menus past it
/// and records that would add further menus are dropped, so an exporter cannot grow the cache without end.
pub const MAX_MENUS: usize = 1024;
/// The most change records of one `Changed` signal applied; the rest of the signal is dropped, so one signal cannot
/// hold the worker for as long as its sender likes.
pub const MAX_SIGNAL_RECORDS: usize = 1000;
/// How deep links are followed: a tree nested deeper builds nothing below this depth.
const MAX_DEPTH: usize = 16;
/// The rounds of `Start` a load makes for groups the menus it read link to; groups further down are subscribed after
/// the load, one round at a time ([`GtkMenuClient::subscription`]).
pub const LOAD_ROUNDS: usize = 8;

/// Which `org.gtk.Menus` object of an exporter a change is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MenuObject {
    MenuBar,
    AppMenu,
}

/// A `Changed` signal of an exporter.
#[derive(Debug)]
pub enum GtkChange {
    /// The records of a menu object's signal, with the signal's serial number, which orders it against the replies of
    /// the same exporter.
    Menus(MenuObject, u32, Vec<RawChange>),
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

    #[proxy(
        gen_blocking = false,
        interface = "org.gtk.Menus",
        assume_defaults = false
    )]
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

    #[proxy(
        gen_blocking = false,
        interface = "org.gtk.Actions",
        assume_defaults = false
    )]
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

/// The cached menus of one `org.gtk.Menus` object of an exporter, and its subscriptions.
#[derive(Default)]
pub struct GtkMenus {
    menus: HashMap<(u32, u32), RawItems>,
    /// Groups whose `Start` reply came: their changes apply.
    subscribed: HashSet<u32>,
    /// Groups asked for with `Start` whose reply has not come.
    pending: HashSet<u32>,
    /// Groups whose `Start` failed: they are not asked for again by this cache.
    refused: HashSet<u32>,
    /// Records for pending groups, with their signal's serial: applied once the reply came if sent after it.
    held: Vec<(u32, RawChange)>,
    /// The serial of each subscribed group's `Start` reply: a record of a signal sent before it (delivered after it)
    /// is already in the reply's menus, and is dropped.
    replied: HashMap<u32, u32>,
}

impl GtkMenus {
    /// The menus of the reply to `Start(groups)`, those groups subscribed.
    pub fn from_reply(groups: &[u32], reply: Vec<RawMenu>) -> Self {
        let mut m = Self::default();
        m.begin(groups);
        m.started(groups, None, reply);
        m
    }

    /// `groups` were asked for with `Start`.
    pub fn begin(&mut self, groups: &[u32]) {
        self.pending.extend(groups);
    }

    /// The reply to `Start(groups)` came, as the message of serial `serial`: the groups' menus are the reply's, and
    /// the records held for them that were sent after the reply apply on top. A group the reply has no menus of is
    /// subscribed and empty. Records of signals sent before the reply are dropped from then on, as the reply holds
    /// them already; the reply's menus past [`MAX_MENUS`] are dropped.
    pub fn started(&mut self, groups: &[u32], serial: Option<u32>, reply: Vec<RawMenu>) {
        for g in groups {
            self.pending.remove(g);
            self.subscribed.insert(*g);
            match serial {
                Some(serial) => self.replied.insert(*g, serial),
                None => self.replied.remove(g),
            };
        }
        self.menus.retain(|(g, _), _| !groups.contains(g));
        for (g, m, mut items) in reply {
            if !groups.contains(&g) || items.is_empty() {
                continue;
            }
            if self.menus.len() >= MAX_MENUS && !self.menus.contains_key(&(g, m)) {
                continue;
            }
            items.truncate(MAX_MENU_ITEMS);
            self.menus.insert((g, m), items);
        }
        let (now, still): (Vec<_>, Vec<_>) = std::mem::take(&mut self.held)
            .into_iter()
            .partition(|(_, record)| groups.contains(&record.0));
        self.held = still;
        for (s, record) in now {
            if serial.is_none_or(|reply| s > reply) {
                self.apply_record(record);
            }
        }
    }

    /// `Start(groups)` failed: they are no longer pending, and not asked for again.
    pub fn refused(&mut self, groups: &[u32]) {
        for g in groups {
            self.pending.remove(g);
            self.refused.insert(*g);
        }
        self.held.retain(|(_, record)| !groups.contains(&record.0));
    }

    /// `groups` were ended: their menus are forgotten and their changes no longer apply.
    pub fn ended(&mut self, groups: &[u32]) {
        for g in groups {
            self.subscribed.remove(g);
            self.replied.remove(g);
        }
        self.menus.retain(|(g, _), _| !groups.contains(g));
    }

    /// Apply one record of a `Changed` signal of serial `serial`: replace `removed` items at `position` with `added`.
    /// A record for a group not subscribed is ignored, one for a group whose reply is awaited is held until it comes,
    /// one sent before the group's reply is ignored (the reply holds it), and one that removes past the end of its
    /// menu is ignored, as the menus' clients do.
    pub fn apply_change(&mut self, serial: u32, change: RawChange) {
        if self.pending.contains(&change.0) {
            if self.held.len() < MAX_HELD_RECORDS {
                self.held.push((serial, change));
            }
            return;
        }
        if self
            .replied
            .get(&change.0)
            .is_some_and(|reply| serial <= *reply)
        {
            return;
        }
        self.apply_record(change);
    }

    fn apply_record(&mut self, change: RawChange) {
        let (g, m, position, removed, added) = change;
        if !self.subscribed.contains(&g) {
            return;
        }
        let key = (g, m);
        // a new menu of a subscribed group (a section added) comes as a record of its own, which only inserts
        let inserts = position == 0 && removed == 0 && !added.is_empty();
        if !self.menus.contains_key(&key) {
            if !inserts || self.menus.len() >= MAX_MENUS {
                return;
            }
            self.menus.insert(key, Vec::new());
        }
        let Some(list) = self.menus.get_mut(&key) else {
            return;
        };
        let (pos, end) = (position as usize, position as usize + removed as usize);
        if end > list.len() {
            return;
        }
        list.splice(pos..end, added);
        list.truncate(MAX_MENU_ITEMS);
        if list.is_empty() {
            self.menus.remove(&key);
        }
    }

    /// How many menus are kept.
    pub fn menu_count(&self) -> usize {
        self.menus.len()
    }

    pub fn get(&self, group: u32, menu: u32) -> Option<&RawItems> {
        self.menus.get(&(group, menu))
    }

    /// The groups subscribed, not those whose reply is awaited.
    pub fn subscribed_groups(&self) -> Vec<u32> {
        let mut all: Vec<u32> = self.subscribed.iter().copied().collect();
        all.sort_unstable();
        all
    }

    /// The groups the tree below `(0, 0)` links to, and whether the walk saw all of it.
    fn reachable(&self) -> (HashSet<u32>, bool) {
        let mut groups = HashSet::from([0]);
        let mut seen = HashSet::new();
        let mut stack = vec![(0, 0)];
        let mut examined = 0;
        while let Some(key) = stack.pop() {
            if !seen.insert(key) {
                continue;
            }
            groups.insert(key.0);
            for it in self.menus.get(&key).into_iter().flatten() {
                examined += 1;
                if examined > MAX_TREE_ITEMS {
                    return (groups, false);
                }
                for k in [":section", ":submenu"] {
                    if let Some(r) = link(it, k) {
                        groups.insert(r.0);
                        stack.push(r);
                    }
                }
            }
        }
        (groups, true)
    }

    /// The groups the tree links to that are not subscribed, asked for or refused, at most as many as keep the
    /// subscriptions within [`MAX_GROUPS`].
    pub fn missing_groups(&self) -> Vec<u32> {
        let room = MAX_GROUPS.saturating_sub(self.subscribed.len() + self.pending.len());
        let mut missing: Vec<u32> = self
            .reachable()
            .0
            .into_iter()
            .filter(|g| {
                !self.subscribed.contains(g)
                    && !self.pending.contains(g)
                    && !self.refused.contains(g)
            })
            .collect();
        missing.sort_unstable();
        missing.truncate(room);
        missing
    }

    /// The groups subscribed that the tree no longer links to (never the root's); none when the tree is too large to
    /// walk whole.
    pub fn unreachable_groups(&self) -> Vec<u32> {
        let (reachable, whole) = self.reachable();
        if !whole {
            return Vec::new();
        }
        let mut gone: Vec<u32> = self
            .subscribed
            .iter()
            .filter(|g| **g != 0 && !reachable.contains(g))
            .copied()
            .collect();
        gone.sort_unstable();
        gone
    }

    /// Whether a `Start` reply is awaited.
    pub fn is_pending(&self) -> bool {
        !self.pending.is_empty()
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

/// An item's icon, from its serialized form: a name, `('themed', <names>)` or `('bytes', <image>)`. A file's icon is
/// not read.
fn icon(item: &HashMap<String, OwnedValue>, key: &str) -> (Option<String>, Option<Vec<u8>>) {
    let name = |s: &str| {
        let file = s.starts_with('/') || s.contains("://");
        (!s.is_empty() && !file).then(|| s.to_owned())
    };
    match super::prop(item, key) {
        Some(Value::Str(s)) => (name(s.as_str()), None),
        Some(Value::Structure(s)) => match s.fields() {
            [kind, value] => match (super::unwrap_variant(kind), super::unwrap_variant(value)) {
                (Value::Str(k), Value::Array(names)) if k.as_str() == "themed" => {
                    let first = names.iter().find_map(|n| match n {
                        Value::Str(n) => name(n.as_str()),
                        _ => None,
                    });
                    (first, None)
                }
                (Value::Str(k), Value::Str(n)) if k.as_str() == "themed" => {
                    (name(n.as_str()), None)
                }
                (Value::Str(k), Value::Array(bytes)) if k.as_str() == "bytes" => {
                    let data: Vec<u8> = bytes
                        .iter()
                        .filter_map(|b| match b {
                            Value::U8(b) => Some(*b),
                            _ => None,
                        })
                        .collect();
                    (None, (!data.is_empty()).then_some(data))
                }
                _ => (None, None),
            },
            _ => (None, None),
        },
        _ => (None, None),
    }
}

#[derive(Clone, Debug)]
pub struct GtkActionState {
    pub enabled: bool,
    /// The type of the action's parameter; `None` for an action without one.
    pub parameter: Option<Signature>,
    pub state: Option<OwnedValue>,
}

impl GtkActionState {
    pub fn from_raw(raw: &RawAction) -> Self {
        Self {
            enabled: raw.0,
            parameter: (raw.1 != Signature::Unit).then(|| raw.1.clone()),
            state: raw.2.first().and_then(|v| v.try_clone().ok()),
        }
    }
}

/// The actions of an exporter by full name (`app.quit`, `win.close`).
pub type GtkActions = HashMap<String, GtkActionState>;

/// Keep the action `name` unless it is known (an addition of a known action changes nothing) or the exporter has
/// [`MAX_ACTIONS`] already.
fn add_action(states: &mut GtkActions, name: String, raw: &RawAction) {
    if states.len() < MAX_ACTIONS && !states.contains_key(&name) {
        states.insert(name, GtkActionState::from_raw(raw));
    }
}

/// Apply a `Changed` signal of the actions of `scope` as the action group's clients do: removals, then enabled
/// changes, then state changes (to a state of the type the action has), then additions.
pub fn apply_actions(
    states: &mut GtkActions,
    scope: &str,
    removed: Vec<String>,
    enabled: HashMap<String, bool>,
    state: HashMap<String, OwnedValue>,
    added: HashMap<String, RawAction>,
) {
    for name in removed {
        states.remove(&format!("{scope}.{name}"));
    }
    for (name, on) in enabled {
        if let Some(action) = states.get_mut(&format!("{scope}.{name}")) {
            action.enabled = on;
        }
    }
    for (name, value) in state {
        if let Some(action) = states.get_mut(&format!("{scope}.{name}")) {
            if let Some(old) = &action.state {
                if old.value_signature() == value.value_signature() {
                    action.state = Some(value);
                }
            }
        }
    }
    for (name, raw) in added {
        add_action(states, format!("{scope}.{name}"), &raw);
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
    actions: &'a GtkActions,
    next_id: i32,
    refs: HashMap<ItemId, GtkActionRef>,
    /// The menus being built, outermost first: a link back to one of them builds nothing.
    path: Vec<(u32, u32)>,
    /// Entries looked at so far, of every menu.
    examined: usize,
}

/// Whether an item of `target` can activate `action`: the action takes a parameter exactly when the item has a target,
/// of the target's type.
fn can_activate(action: &GtkActionState, target: Option<&OwnedValue>) -> bool {
    match (&action.parameter, target) {
        (None, None) => true,
        (Some(parameter), Some(target)) => target.value_signature() == parameter,
        _ => false,
    }
}

/// The mark of an item of `target` whose action has `state`.
fn toggle(state: Option<&OwnedValue>, target: Option<&OwnedValue>) -> Toggle {
    match (state, target) {
        (Some(state), Some(target)) => {
            let same = state.value_signature() == target.value_signature() && **state == **target;
            Toggle::Radio(same)
        }
        (Some(state), None) => match &**state {
            Value::Bool(b) => Toggle::Check(*b),
            _ => Toggle::None,
        },
        _ => Toggle::None,
    }
}

impl Builder<'_> {
    fn new_id(&mut self) -> i32 {
        self.next_id += 1;
        self.next_id
    }

    fn build(&mut self, key: (u32, u32), namespace: &str) -> Vec<MenuItem> {
        if self.path.len() > MAX_DEPTH || self.path.contains(&key) {
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
            if super::prop_str(it, "hidden-when").as_deref() == Some("macos-menubar") {
                continue;
            }
            // a link's namespace prefixes the actions of the menu it links to
            let linked = match super::prop_str(it, "action-namespace") {
                Some(n) => format!("{namespace}{n}."),
                None => namespace.to_string(),
            };
            if let Some(sec) = link(it, ":section") {
                let inner = self.build(sec, &linked);
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
                let children = self.build(sub, &linked);
                let mut item = MenuItem::submenu(id, "", children);
                (item.label, item.mnemonic) = split_gtk_mnemonic(&label);
                (item.icon_name, item.icon_data) = icon(it, "icon");
                out.push(item);
                continue;
            }
            if let Some(item) = self.item(it, &label, namespace) {
                out.push(item);
            }
        }
        while out.last().is_some_and(|l| l.separator) {
            out.pop();
        }
        self.path.pop();
        out
    }

    /// The row of an item that links to no menu, unless it is hidden.
    fn item(
        &mut self,
        it: &HashMap<String, OwnedValue>,
        label: &str,
        namespace: &str,
    ) -> Option<MenuItem> {
        let action = super::prop_str(it, "action");
        if label.is_empty() && action.is_none() {
            return None;
        }
        let full = action.map(|a| format!("{namespace}{a}"));
        let target = it.get("target").and_then(|v| v.try_clone().ok());
        let found = full.as_ref().and_then(|a| self.actions.get(a));
        let usable = found.filter(|s| can_activate(s, target.as_ref()));
        let enabled = match (&full, usable) {
            (Some(_), Some(s)) => s.enabled,
            (Some(_), None) => false,
            (None, _) => true,
        };
        match super::prop_str(it, "hidden-when").as_deref() {
            Some("action-disabled") if full.is_some() && !enabled => return None,
            Some("action-missing") if full.is_some() && usable.is_none() => return None,
            _ => {}
        }
        let id = self.new_id();
        let mut item = MenuItem::new(id, "");
        (item.label, item.mnemonic) = split_gtk_mnemonic(label);
        item.enabled = enabled;
        if let Some(s) = usable {
            item.toggle = toggle(s.state.as_ref(), target.as_ref());
        }
        item.accel = super::prop_str(it, "accel").filter(|a| !a.is_empty());
        (item.icon_name, item.icon_data) = match icon(it, "icon") {
            (None, None) => icon(it, "verb-icon"),
            found => found,
        };
        if let Some(a) = full {
            item.action = Some(a.clone());
            self.refs
                .insert(ItemId(id), GtkActionRef { action: a, target });
        }
        Some(item)
    }
}

/// Build the item tree below `root` (the menu bar is `(0, 0)`). `actions` is keyed by full action
/// name (`app.quit`, `win.close`).
pub fn build_menu(menus: &GtkMenus, root: (u32, u32), actions: &GtkActions) -> GtkBuild {
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
pub fn build_menus(menubar: &GtkMenus, app_menu: &GtkMenus, actions: &GtkActions) -> GtkBuild {
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

/// Scope names of the action objects an exporter announces: the application's and the window's.
pub const SCOPES: [&str; 2] = ["app", "win"];

/// The platform data `Activate` carries: the activation token under both keys toolkits read it from (`activation-token`
/// on Wayland, `desktop-startup-id` elsewhere and in older toolkits), so a window the action opens takes the focus.
pub fn platform_data(token: Option<&str>) -> HashMap<&'static str, Value<'static>> {
    let mut data = HashMap::new();
    if let Some(token) = token.filter(|t| !t.is_empty()) {
        data.insert("activation-token", Value::from(token.to_owned()));
        data.insert("desktop-startup-id", Value::from(token.to_owned()));
    }
    data
}
