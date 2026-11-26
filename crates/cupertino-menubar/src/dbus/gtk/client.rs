//! The connection to one exporter of GMenuModel menus: its loads, subscriptions, changes and activations.

use std::collections::HashMap;

use futures_util::stream::BoxStream;
use futures_util::StreamExt;
use zbus::zvariant::Value;
use zbus::Connection;

use super::{
    add_action, apply_actions, build_menus, platform_data, GtkActionRef, GtkActions,
    GtkActionsProxy, GtkChange, GtkMenus, GtkMenusProxy, MenuObject, RawMenu, LOAD_ROUNDS,
    MAX_SIGNAL_RECORDS, SCOPES,
};
use crate::item::{ItemId, MenuItem};
use crate::source::GtkMenuProps;

/// A connected exporter.
pub struct GtkMenuClient {
    conn: Connection,
    props: GtkMenuProps,
    menus: GtkMenus,
    app_menu: GtkMenus,
    action_states: GtkActions,
    refs: HashMap<ItemId, GtkActionRef>,
}

async fn menus_proxy(
    conn: &Connection,
    bus_name: &str,
    path: &str,
) -> zbus::Result<GtkMenusProxy<'static>> {
    GtkMenusProxy::builder(conn)
        .destination(bus_name.to_owned())?
        .path(path.to_owned())?
        .build()
        .await
}

async fn actions_proxy(
    conn: &Connection,
    bus_name: &str,
    path: &str,
) -> zbus::Result<GtkActionsProxy<'static>> {
    GtkActionsProxy::builder(conn)
        .destination(bus_name.to_owned())?
        .path(path.to_owned())?
        .build()
        .await
}

/// `Start(groups)`, with the serial number of the reply, which orders it against the exporter's signals.
async fn start(proxy: &GtkMenusProxy<'_>, groups: &[u32]) -> zbus::Result<(u32, Vec<RawMenu>)> {
    let reply = proxy.inner().call_method("Start", &(groups,)).await?;
    let serial = reply.primary_header().serial_num().get();
    let menus: Vec<RawMenu> = reply.body().deserialize()?;
    Ok((serial, menus))
}

/// Subscribe `cache` to the groups of the menus at `path` reachable from `(0, 0)`, [`LOAD_ROUNDS`] rounds deep.
async fn load_menus(
    conn: &Connection,
    bus_name: &str,
    path: Option<&str>,
    cache: &mut GtkMenus,
) -> zbus::Result<()> {
    let Some(path) = path else {
        return Ok(());
    };
    let proxy = menus_proxy(conn, bus_name, path).await?;
    let mut groups = vec![0];
    for _ in 0..LOAD_ROUNDS {
        cache.begin(&groups);
        match start(&proxy, &groups).await {
            Ok((serial, reply)) => cache.started(&groups, Some(serial), reply),
            Err(e) => {
                cache.refused(&groups);
                return Err(e);
            }
        }
        groups = cache.missing_groups();
        if groups.is_empty() {
            break;
        }
    }
    Ok(())
}

impl GtkMenuClient {
    /// Load the menu bar, the application menu and the action states of the window and application objects. A load
    /// that fails ends the subscriptions it made.
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
        match c.fetch().await {
            Ok(()) => {
                let items = c.rebuild();
                Ok((c, items))
            }
            Err(e) => {
                if let Some(end) = c.release() {
                    end.run().await;
                }
                Err(e)
            }
        }
    }

    async fn fetch(&mut self) -> zbus::Result<()> {
        let (conn, bus) = (&self.conn, self.props.bus_name.as_str());
        load_menus(
            conn,
            bus,
            self.props.menubar_object_path.as_deref(),
            &mut self.menus,
        )
        .await?;
        load_menus(
            conn,
            bus,
            self.props.app_menu_object_path.as_deref(),
            &mut self.app_menu,
        )
        .await?;
        for scope in SCOPES {
            let Some(path) = self.scope_path(scope) else {
                continue;
            };
            if let Ok(proxy) = actions_proxy(conn, bus, path).await {
                if let Ok(all) = proxy.describe_all().await {
                    for (name, raw) in all {
                        add_action(&mut self.action_states, format!("{scope}.{name}"), &raw);
                    }
                }
            }
        }
        Ok(())
    }

    /// The object of the actions of `scope` (`app` or `win`); none for any other scope.
    fn scope_path(&self, scope: &str) -> Option<&str> {
        match scope {
            "app" => self.props.application_object_path.as_deref(),
            "win" => self.props.window_object_path.as_deref(),
            _ => None,
        }
    }

    fn menu_path(&self, object: MenuObject) -> Option<&str> {
        match object {
            MenuObject::MenuBar => self.props.menubar_object_path.as_deref(),
            MenuObject::AppMenu => self.props.app_menu_object_path.as_deref(),
        }
    }

    fn cache(&mut self, object: MenuObject) -> &mut GtkMenus {
        match object {
            MenuObject::MenuBar => &mut self.menus,
            MenuObject::AppMenu => &mut self.app_menu,
        }
    }

    fn rebuild(&mut self) -> Vec<MenuItem> {
        let built = build_menus(&self.menus, &self.app_menu, &self.action_states);
        self.refs = built.actions;
        built.items
    }

    /// What activating `item` of the tree last built calls: its action's name and target and the object that owns the
    /// action, with `token` (an activation token) in its platform data ([`GtkActivation::with_token`] hands one over
    /// later); `None` for an item without an action or whose action belongs to no object of the exporter. The ids are
    /// numbered afresh with each build, so an activation taken when the item was chosen stays that item's.
    pub fn activation(&self, item: ItemId, token: Option<&str>) -> Option<GtkActivation> {
        let r = self.refs.get(&item)?;
        let (scope, name) = r.action.split_once('.')?;
        let path = self.scope_path(scope)?.to_owned();
        let params: Vec<Value<'static>> = r
            .target
            .as_ref()
            .and_then(|t| t.try_clone().ok())
            .map(|t| vec![Value::from(t)])
            .unwrap_or_default();
        Some(GtkActivation {
            conn: self.conn.clone(),
            bus_name: self.props.bus_name.clone(),
            path,
            name: name.to_string(),
            params,
            platform_data: platform_data(token),
        })
    }

    /// Activate the action behind `item`. Returns `false` for items without an action.
    pub async fn activate(&self, item: ItemId, token: Option<&str>) -> zbus::Result<bool> {
        match self.activation(item, token) {
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
        let bus = props.bus_name.as_str();
        let mut streams: Vec<BoxStream<'static, GtkChange>> = Vec::new();
        for (object, path) in [
            (MenuObject::MenuBar, &props.menubar_object_path),
            (MenuObject::AppMenu, &props.app_menu_object_path),
        ] {
            let Some(path) = path else { continue };
            let changed = menus_proxy(conn, bus, path)
                .await?
                .receive_changed()
                .await?;
            streams.push(
                changed
                    .filter_map(move |signal| async move {
                        let serial = signal.message().primary_header().serial_num().get();
                        let args = signal.args().ok()?;
                        Some(GtkChange::Menus(object, serial, args.changes))
                    })
                    .boxed(),
            );
        }
        for (scope, path) in [
            ("app", &props.application_object_path),
            ("win", &props.window_object_path),
        ] {
            let Some(path) = path else { continue };
            let changed = actions_proxy(conn, bus, path)
                .await?
                .receive_changed()
                .await?;
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

    /// Take a change of the exporter into the cache; [`Self::items`] then builds the tree from it, and
    /// [`Self::subscription`] follows the groups it adds and removes.
    pub fn apply(&mut self, change: GtkChange) {
        match change {
            GtkChange::Menus(object, serial, records) => {
                let menus = self.cache(object);
                for record in records.into_iter().take(MAX_SIGNAL_RECORDS) {
                    menus.apply_change(serial, record);
                }
            }
            GtkChange::Actions {
                scope,
                removed,
                enabled,
                state,
                added,
            } => apply_actions(
                &mut self.action_states,
                scope,
                removed,
                enabled,
                state,
                added,
            ),
        }
    }

    /// The tree of the cached menus and actions.
    pub fn items(&mut self) -> Vec<MenuItem> {
        self.rebuild()
    }

    /// Subscribe to the groups the tree links to and is not subscribed to, and end those it no longer links to, as
    /// one call to make apart from the client; `None` when there is nothing to do. A menu object with a `Start`
    /// awaited makes no other call until it is answered ([`Self::started`]), so subscriptions follow the tree one round
    /// at a time.
    pub fn subscription(&mut self) -> Option<GtkSubscription> {
        let mut calls = Vec::new();
        for object in [MenuObject::MenuBar, MenuObject::AppMenu] {
            let Some(path) = self.menu_path(object).map(str::to_owned) else {
                continue;
            };
            let cache = self.cache(object);
            if cache.is_pending() {
                continue;
            }
            let (start, end) = (cache.missing_groups(), cache.unreachable_groups());
            if start.is_empty() && end.is_empty() {
                continue;
            }
            cache.ended(&end);
            cache.begin(&start);
            calls.push(Call {
                object,
                path,
                start,
                end,
            });
        }
        (!calls.is_empty()).then(|| GtkSubscription {
            conn: self.conn.clone(),
            bus_name: self.props.bus_name.clone(),
            calls,
        })
    }

    /// Take in the replies of a [`GtkSubscription`] of this client.
    pub fn started(&mut self, started: GtkStarted) {
        for (object, groups, reply) in started.starts {
            let cache = self.cache(object);
            match reply {
                Ok((serial, menus)) => cache.started(&groups, Some(serial), menus),
                Err(_) => cache.refused(&groups),
            }
        }
    }

    /// End every subscription of the client, which is dropped: the exporter stops sending its changes. Groups whose
    /// `Start` is awaited are ended once it is answered ([`GtkStarted::release`]).
    pub fn release(self) -> Option<GtkSubscription> {
        let mut calls = Vec::new();
        for (object, cache) in [
            (MenuObject::MenuBar, &self.menus),
            (MenuObject::AppMenu, &self.app_menu),
        ] {
            let (Some(path), end) = (self.menu_path(object), cache.subscribed_groups()) else {
                continue;
            };
            if !end.is_empty() {
                calls.push(Call {
                    object,
                    path: path.to_owned(),
                    start: Vec::new(),
                    end,
                });
            }
        }
        (!calls.is_empty()).then(|| GtkSubscription {
            conn: self.conn.clone(),
            bus_name: self.props.bus_name.clone(),
            calls,
        })
    }
}

/// One menu object's part of a [`GtkSubscription`].
struct Call {
    object: MenuObject,
    path: String,
    start: Vec<u32>,
    end: Vec<u32>,
}

/// Calls to `End` and `Start` of an exporter, made apart from its client.
pub struct GtkSubscription {
    conn: Connection,
    bus_name: String,
    calls: Vec<Call>,
}

impl GtkSubscription {
    /// The groups each menu object ends and starts.
    pub fn groups(&self) -> Vec<(MenuObject, Vec<u32>, Vec<u32>)> {
        self.calls
            .iter()
            .map(|c| (c.object, c.end.clone(), c.start.clone()))
            .collect()
    }

    /// End, then start, the groups of each menu object.
    pub async fn run(self) -> GtkStarted {
        let mut starts = Vec::new();
        let mut failures = Vec::new();
        let mut paths = HashMap::new();
        for call in self.calls {
            let proxy = match menus_proxy(&self.conn, &self.bus_name, &call.path).await {
                Ok(proxy) => proxy,
                Err(e) => {
                    if !call.start.is_empty() {
                        starts.push((call.object, call.start, Err(e)));
                    }
                    continue;
                }
            };
            if !call.end.is_empty() {
                if let Err(e) = proxy.end(&call.end).await {
                    failures.push(e);
                }
            }
            if !call.start.is_empty() {
                let reply = start(&proxy, &call.start).await;
                starts.push((call.object, call.start, reply));
            }
            paths.insert(call.object, call.path);
        }
        GtkStarted {
            conn: self.conn,
            bus_name: self.bus_name,
            paths,
            starts,
            failures,
        }
    }
}

/// `Start` reply of a menu object: the reply's serial and menus.
type StartReply = zbus::Result<(u32, Vec<RawMenu>)>;

/// The replies of a [`GtkSubscription`].
pub struct GtkStarted {
    conn: Connection,
    bus_name: String,
    paths: HashMap<MenuObject, String>,
    starts: Vec<(MenuObject, Vec<u32>, StartReply)>,
    /// The calls that failed, besides `Start`s, whose failure the client takes in.
    pub failures: Vec<zbus::Error>,
}

impl GtkStarted {
    /// End the groups this subscribed to: the client they were for is gone.
    pub fn release(self) -> Option<GtkSubscription> {
        let calls: Vec<Call> = self
            .starts
            .into_iter()
            .filter(|(_, _, reply)| reply.is_ok())
            .filter_map(|(object, groups, _)| {
                Some(Call {
                    object,
                    path: self.paths.get(&object)?.clone(),
                    start: Vec::new(),
                    end: groups,
                })
            })
            .collect();
        (!calls.is_empty()).then(|| GtkSubscription {
            conn: self.conn,
            bus_name: self.bus_name,
            calls,
        })
    }
}

/// An item's activation, made apart from its client: the client stays with the worker meanwhile.
pub struct GtkActivation {
    conn: Connection,
    bus_name: String,
    path: String,
    name: String,
    params: Vec<Value<'static>>,
    platform_data: HashMap<&'static str, Value<'static>>,
}

impl GtkActivation {
    /// The same activation with `token` (an activation token) in its platform data instead.
    pub fn with_token(self, token: Option<&str>) -> Self {
        Self {
            platform_data: platform_data(token),
            ..self
        }
    }

    pub async fn run(self) -> zbus::Result<()> {
        let proxy = actions_proxy(&self.conn, &self.bus_name, &self.path).await?;
        proxy
            .activate(&self.name, &self.params, self.platform_data)
            .await
    }
}
