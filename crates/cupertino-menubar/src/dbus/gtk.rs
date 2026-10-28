//! Reader for GMenuModel exported over D-Bus (`org.gtk.Menus` and `org.gtk.Actions`, open GNOME
//! interfaces).
//!
//! `org.gtk.Menus.Start(au) → a(uuaa{sv})` returns whole menus keyed by `(group, menu)`; an item
//! links to other menus through the `:section` and `:submenu` keys (each a `(uu)` pair). Sections are
//! flattened with separators between them. Item attributes used: `label`, `action`,
//! `action-namespace`, `target`, `hidden-when`, `accel`. Action state and enabled flags come from
//! `org.gtk.Actions.DescribeAll`; activation calls `org.gtk.Actions.Activate` on the object that
//! owns the action (`app.` actions on the application object, `win.` actions on the window object).

use std::collections::{HashMap, HashSet};

use zbus::zvariant::{OwnedValue, Signature, Value};
use zbus::Connection;

use crate::item::{split_gtk_mnemonic, ItemId, MenuItem, Shortcut, ShortcutModifier, Toggle};
use crate::source::GtkMenuProps;

pub type RawItems = Vec<HashMap<String, OwnedValue>>;
/// Reply item of `Start`: `(group, menu, items)`.
pub type RawMenu = (u32, u32, RawItems);
/// Change record of the `Changed` signal: `(group, menu, position, removed, added)`.
pub type RawChange = (u32, u32, u32, u32, RawItems);
/// `DescribeAll` value: `(enabled, parameter type, state)`.
pub type RawAction = (bool, Signature, Vec<OwnedValue>);

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
        for (g, m, items) in reply {
            self.menus.insert((g, m), items);
        }
    }

    /// Apply one record of the `Changed` signal: replace `removed` items at `position` with `added`.
    pub fn apply_change(&mut self, change: RawChange) {
        let (g, m, position, removed, added) = change;
        let list = self.menus.entry((g, m)).or_default();
        let pos = (position as usize).min(list.len());
        let end = (pos + removed as usize).min(list.len());
        list.splice(pos..end, added);
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

/// Parse a GTK accelerator such as `<Primary><Shift>q`.
pub fn parse_accel(accel: &str) -> Option<Shortcut> {
    let mut rest = accel.trim();
    let mut modifiers = Vec::new();
    while let Some(r) = rest.strip_prefix('<') {
        let end = r.find('>')?;
        let m = match r[..end].to_ascii_lowercase().as_str() {
            "primary" | "control" | "ctrl" => ShortcutModifier::Ctrl,
            "alt" | "mod1" => ShortcutModifier::Alt,
            "shift" => ShortcutModifier::Shift,
            "super" | "meta" | "mod4" => ShortcutModifier::Primary,
            _ => return None,
        };
        if !modifiers.contains(&m) {
            modifiers.push(m);
        }
        rest = &r[end + 1..];
    }
    modifiers.sort();
    (!rest.is_empty()).then(|| Shortcut {
        modifiers,
        key: rest.to_string(),
    })
}

struct Builder<'a> {
    menus: &'a GtkMenus,
    actions: &'a HashMap<String, GtkActionState>,
    next_id: i32,
    refs: HashMap<ItemId, GtkActionRef>,
    depth: usize,
}

impl Builder<'_> {
    fn new_id(&mut self) -> i32 {
        self.next_id += 1;
        self.next_id
    }

    fn build(&mut self, key: (u32, u32), namespace: &str) -> Vec<MenuItem> {
        if self.depth > 16 {
            return Vec::new();
        }
        self.depth += 1;
        let mut out: Vec<MenuItem> = Vec::new();
        let Some(items) = self.menus.get(key.0, key.1) else {
            self.depth -= 1;
            return out;
        };
        for it in items {
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
                let mut item = MenuItem::submenu(id, &label, children);
                item.has_submenu = true;
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
            if let Some(a) = super::prop_str(it, "accel") {
                item.shortcut = parse_accel(&a);
            }
            if let Some(a) = full {
                self.refs
                    .insert(ItemId(id), GtkActionRef { action: a, target });
            }
            out.push(item);
        }
        while out.last().is_some_and(|l| l.separator) {
            out.pop();
        }
        self.depth -= 1;
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
        depth: 0,
    };
    let items = b.build(root, "");
    GtkBuild {
        items,
        actions: b.refs,
    }
}

/// A connected exporter.
pub struct GtkMenuClient {
    conn: Connection,
    props: GtkMenuProps,
    menus: GtkMenus,
    action_states: HashMap<String, GtkActionState>,
    refs: HashMap<ItemId, GtkActionRef>,
}

impl GtkMenuClient {
    /// Load the menu bar and the action states of the window and application objects.
    pub async fn load(
        conn: &Connection,
        props: &GtkMenuProps,
    ) -> zbus::Result<(Self, Vec<MenuItem>)> {
        let mut c = Self {
            conn: conn.clone(),
            props: props.clone(),
            menus: GtkMenus::default(),
            action_states: HashMap::new(),
            refs: HashMap::new(),
        };
        let items = c.reload().await?;
        Ok((c, items))
    }

    async fn menus_proxy(&self) -> zbus::Result<GtkMenusProxy<'static>> {
        let path = self
            .props
            .menubar_object_path
            .clone()
            .ok_or_else(|| zbus::Error::Failure("no menu bar path".into()))?;
        GtkMenusProxy::builder(&self.conn)
            .destination(self.props.bus_name.clone())?
            .path(path)?
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

    /// Fetch everything again and rebuild the tree.
    pub async fn reload(&mut self) -> zbus::Result<Vec<MenuItem>> {
        let menus = self.menus_proxy().await?;
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
        self.menus = cache;
        self.action_states.clear();
        for (scope, path) in [
            ("app", self.props.application_object_path.clone()),
            ("win", self.props.window_object_path.clone()),
        ] {
            let Some(path) = path else { continue };
            if let Ok(proxy) = self.actions_proxy(&path).await {
                if let Ok(all) = proxy.describe_all().await {
                    for (name, raw) in all {
                        self.action_states
                            .insert(format!("{scope}.{name}"), GtkActionState::from_raw(&raw));
                    }
                }
            }
        }
        let built = build_menu(&self.menus, (0, 0), &self.action_states);
        self.refs = built.actions;
        Ok(built.items)
    }

    /// Activate the action behind `item`. Returns `false` for items without an action.
    pub async fn activate(&self, item: ItemId) -> zbus::Result<bool> {
        let Some(r) = self.refs.get(&item) else {
            return Ok(false);
        };
        let (scope, name) = r.action.split_once('.').unwrap_or(("app", &r.action));
        let path = match scope {
            "win" => self.props.window_object_path.clone(),
            _ => self.props.application_object_path.clone(),
        };
        let Some(path) = path else { return Ok(false) };
        let proxy = self.actions_proxy(&path).await?;
        let params: Vec<Value<'static>> = r
            .target
            .as_ref()
            .and_then(|t| t.try_clone().ok())
            .map(|t| match Value::from(t) {
                Value::Value(inner) => vec![*inner],
                other => vec![other],
            })
            .unwrap_or_default();
        proxy.activate(name, &params, HashMap::new()).await?;
        Ok(true)
    }

    pub fn menus(&self) -> &GtkMenus {
        &self.menus
    }

    /// Apply a `Changed` record and rebuild the tree.
    pub fn apply_menu_change(&mut self, change: RawChange) -> Vec<MenuItem> {
        self.menus.apply_change(change);
        let built = build_menu(&self.menus, (0, 0), &self.action_states);
        self.refs = built.actions;
        built.items
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
                    item(vec![("label", v("_File")), (":submenu", pair(0, 1))]),
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
            ("File", Some('F'), true)
        );
        let labels: Vec<&str> = file.children.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(
            labels,
            vec!["New Window", "Close & Save", "", "Quit"],
            "empty sections vanish, others are separated, the action-missing item is hidden, & is plain text"
        );
        assert!(file.children[2].separator);
        assert_eq!(
            file.children[0].shortcut.as_ref().unwrap().display(),
            "Ctrl+n"
        );
        assert_eq!(
            file.children[1].shortcut.as_ref().unwrap().display(),
            "Ctrl+Shift+w"
        );
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
        m.apply_change((0, 9, 5, 3, vec![item(vec![("label", v("late"))])]));
        assert_eq!(m.get(0, 9).unwrap().len(), 1, "positions are clamped");
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
    fn accelerators_parse() {
        assert_eq!(parse_accel("<Primary>q").unwrap().display(), "Ctrl+q");
        assert_eq!(
            parse_accel("<Shift><Alt>F4").unwrap().display(),
            "Alt+Shift+F4"
        );
        assert_eq!(
            parse_accel("<Super>space").unwrap().display(),
            "Super+space"
        );
        assert!(parse_accel("<Hyper>x").is_none());
        assert!(parse_accel("<Primary>").is_none());
        assert!(parse_accel("<Primary").is_none());
    }
}
