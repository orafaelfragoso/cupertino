//! The worker's GMenuModel exporters: their loads, reloads, changes, subscriptions and activations.
//!
//! An exporter is followed from [`Request::FetchGtk`] until [`Request::ReleaseGtk`]. Its changes are applied to its
//! client as they come and its tree sent at most every [`GTK_CHANGE_INTERVAL`]; with the tree, the groups the changes
//! brought into it are subscribed and those they took out ended, one `Start` of a menu object awaited at a time. A
//! released exporter, a client a reload replaced and a load or `Start` that ends after its exporter was released end
//! their subscriptions, so the exporter stops sending their changes.

use std::collections::{HashMap, HashSet};

use futures_util::stream::BoxStream;
use futures_util::StreamExt;
use zbus::Connection;

use super::{
    is_access_denied, report, Event, Out, Request, Work, GTK_CHANGE_INTERVAL, GTK_RELOADS,
    GTK_RETRY_FIRST, GTK_RETRY_LAST,
};
use crate::dbus::gtk::{GtkChange, GtkMenuClient, GtkStarted, GtkSubscription};
use crate::item::{ItemId, MenuItem};
use crate::source::{GtkMenuProps, MenuSource};
use crate::spawned::Spawner;

/// A loaded exporter: its client, and the load it came from, which names the `Start` replies meant for it.
struct Exporter {
    client: GtkMenuClient,
    generation: u64,
}

pub(super) struct Exporters {
    conn: Connection,
    spawner: Spawner,
    tx: async_channel::Sender<Work>,
    out: Out,
    clients: HashMap<GtkMenuProps, Exporter>,
    /// The task that forwards each exporter's changes; replacing or dropping it ends the following.
    follows: HashMap<GtkMenuProps, zbus::Task<()>>,
    /// Exporters whose changes were applied since their tree was last sent.
    unsent: HashSet<GtkMenuProps>,
    /// Exporters whose menus are loaded again, and those a change came for while they loaded.
    reloading: HashSet<GtkMenuProps>,
    missed: HashSet<GtkMenuProps>,
    /// Exporters asked for and not released: a load of any other is ended as it comes.
    wanted: HashSet<GtkMenuProps>,
    next_generation: u64,
}

impl Exporters {
    pub(super) fn new(
        conn: &Connection,
        spawner: &Spawner,
        tx: &async_channel::Sender<Work>,
        out: &Out,
    ) -> Self {
        Self {
            conn: conn.clone(),
            spawner: spawner.clone(),
            tx: tx.clone(),
            out: out.clone(),
            clients: HashMap::new(),
            follows: HashMap::new(),
            unsent: HashSet::new(),
            reloading: HashSet::new(),
            missed: HashSet::new(),
            wanted: HashSet::new(),
            next_generation: 0,
        }
    }

    fn emit(&self, props: &GtkMenuProps, items: Vec<MenuItem>) {
        self.out.emit(Event::Layout {
            source: MenuSource::Gtk(props.clone()),
            root: Box::new(MenuItem::submenu(0, "", items)),
        });
    }

    /// Load the menus of `props` and follow their changes.
    pub(super) fn fetch(&mut self, props: GtkMenuProps) {
        self.wanted.insert(props.clone());
        let (conn, tx, out) = (self.conn.clone(), self.tx.clone(), self.out.clone());
        self.spawner.spawn(
            async move {
                // followed before the load and read from the start: a full signal queue would stop the connection,
                // and with it the load's own reply
                let follow = match GtkMenuClient::changes(&conn, &props).await {
                    Ok(changes) => Some(follow_gtk(&conn, &tx, props.clone(), changes)),
                    Err(e) => {
                        report(&format!("cannot follow the menus of {}", props.bus_name), e);
                        None
                    }
                };
                let mut wait = GTK_RETRY_FIRST;
                let loaded = loop {
                    match GtkMenuClient::load(&conn, &props).await {
                        Err(e) if is_access_denied(&e) && wait <= GTK_RETRY_LAST => {
                            async_io::Timer::after(wait).await;
                            wait *= 2;
                        }
                        loaded => break loaded,
                    }
                };
                match loaded {
                    Ok((client, items)) => {
                        let loaded = Work::GtkLoaded {
                            props,
                            client: Box::new(client),
                            items,
                            follow,
                            reloads: 0,
                        };
                        // the worker drops its receiver only when it ends
                        if tx.send(loaded).await.is_err() {
                            report(
                                "cannot keep the menu exporter",
                                "the worker is shutting down",
                            );
                        }
                    }
                    Err(e) => {
                        report(&format!("cannot load the menu of {}", props.bus_name), e);
                        out.emit(Event::LayoutFailed {
                            source: MenuSource::Gtk(props),
                        });
                    }
                }
            },
            "cupertino-fetch-gtk",
        );
    }

    /// No bar shows the menus of `props` any more: stop following them and end their subscriptions.
    pub(super) fn release(&mut self, props: &GtkMenuProps) {
        self.wanted.remove(props);
        self.follows.remove(props);
        self.unsent.remove(props);
        self.reloading.remove(props);
        self.missed.remove(props);
        if let Some(exporter) = self.clients.remove(props) {
            self.end(exporter.client.release());
        }
    }

    /// The menus of `props` loaded, after `reloads` loads before this one.
    pub(super) fn loaded(
        &mut self,
        props: GtkMenuProps,
        client: GtkMenuClient,
        items: Vec<MenuItem>,
        follow: Option<zbus::Task<()>>,
        reloads: u8,
    ) {
        if !self.wanted.contains(&props) {
            return self.end(client.release());
        }
        // sent once the client is kept, so an item chosen from the tree finds its action
        self.emit(&props, items);
        self.next_generation += 1;
        let exporter = Exporter {
            client,
            generation: self.next_generation,
        };
        if let Some(old) = self.clients.insert(props.clone(), exporter) {
            // after the new client's `Start`s: the exporter counts the bar's subscriptions of a group
            self.end(old.client.release());
        }
        if reloads == 0 {
            match follow {
                Some(task) => {
                    self.follows.insert(props.clone(), task);
                }
                None => {
                    self.follows.remove(&props);
                }
            }
        }
        self.reloading.remove(&props);
        if self.missed.remove(&props) && reloads < GTK_RELOADS {
            self.reloading.insert(props.clone());
            self.reload(props, reloads + 1);
        } else {
            self.subscribe(&props);
        }
    }

    /// A change of `props`; `applied` is answered at once, so the exporter has one change waiting at most.
    pub(super) fn changed(
        &mut self,
        props: GtkMenuProps,
        change: GtkChange,
        applied: &async_channel::Sender<()>,
    ) {
        // the forwarding task waits for the answer; it is gone only when its exporter was replaced
        let _ = applied.try_send(());
        if self.reloading.contains(&props) || !self.clients.contains_key(&props) {
            self.missed.insert(props.clone());
        }
        let Some(exporter) = self.clients.get_mut(&props) else {
            return;
        };
        exporter.client.apply(change);
        if self.unsent.insert(props.clone()) {
            let tx = self.tx.clone();
            self.spawner.spawn(
                async move {
                    async_io::Timer::after(GTK_CHANGE_INTERVAL).await;
                    // the worker drops its receiver only when it ends
                    let _ = tx.send(Work::GtkFlush(props)).await;
                },
                "cupertino-gtk-flush",
            );
        }
    }

    /// Send the tree of `props`, whose changes were applied since it was last sent, and follow its groups.
    pub(super) fn flush(&mut self, props: GtkMenuProps) {
        self.unsent.remove(&props);
        if let Some(exporter) = self.clients.get_mut(&props) {
            let items = exporter.client.items();
            self.emit(&props, items);
            self.subscribe(&props);
        }
    }

    /// Subscribe to the groups the tree of `props` links to and end those it no longer does.
    fn subscribe(&mut self, props: &GtkMenuProps) {
        let Some(exporter) = self.clients.get_mut(props) else {
            return;
        };
        let Some(subscription) = exporter.client.subscription() else {
            return;
        };
        let (tx, props, generation) = (self.tx.clone(), props.clone(), exporter.generation);
        self.spawner.spawn(
            async move {
                let started = subscription.run().await;
                // the worker drops its receiver only when it ends
                let _ = tx
                    .send(Work::GtkStarted {
                        props,
                        generation,
                        started: Box::new(started),
                    })
                    .await;
            },
            "cupertino-gtk-subscribe",
        );
    }

    /// The replies of a subscription of the load `generation` of `props`: taken in and the tree sent, or, for a client
    /// that is gone, the groups started ended.
    pub(super) fn started(&mut self, props: GtkMenuProps, generation: u64, started: GtkStarted) {
        for e in &started.failures {
            report(&format!("cannot end menus of {}", props.bus_name), e);
        }
        match self.clients.get_mut(&props) {
            Some(exporter) if exporter.generation == generation => {
                exporter.client.started(started);
                let items = exporter.client.items();
                self.emit(&props, items);
                self.subscribe(&props);
            }
            _ => self.end(started.release()),
        }
    }

    /// End the subscriptions of a client that is gone.
    fn end(&self, subscription: Option<GtkSubscription>) {
        let Some(subscription) = subscription else {
            return;
        };
        self.spawner.spawn(
            async move {
                // an exporter that went ended them itself, and one that refuses keeps nothing the bar can undo
                let _ = subscription.run().await;
            },
            "cupertino-gtk-end",
        );
    }

    /// Load the menus of `props` again, after changes came while they loaded.
    fn reload(&self, props: GtkMenuProps, reloads: u8) {
        let (conn, tx) = (self.conn.clone(), self.tx.clone());
        self.spawner.spawn(
            async move {
                match GtkMenuClient::load(&conn, &props).await {
                    Ok((client, items)) => {
                        let loaded = Work::GtkLoaded {
                            props,
                            client: Box::new(client),
                            items,
                            follow: None,
                            reloads,
                        };
                        // the worker drops its receiver only when it ends
                        let _ = tx.send(loaded).await;
                    }
                    Err(e) => report(&format!("cannot load the menu of {}", props.bus_name), e),
                }
            },
            "cupertino-reload-gtk",
        );
    }

    /// Activate `item` of `props`, with `token` in the activation's platform data.
    pub(super) fn activate(&self, props: &GtkMenuProps, item: i32, token: Option<&str>) {
        // the client stays: its changes keep coming while the activation runs
        let Some(activation) = self
            .clients
            .get(props)
            .and_then(|e| e.client.activation(ItemId(item), token))
        else {
            return report(
                &format!("cannot activate item {item} of {}", props.bus_name),
                "the menu is not loaded or the item has no action",
            );
        };
        let bus_name = props.bus_name.clone();
        self.spawner.spawn(
            async move {
                if let Err(e) = activation.run().await {
                    report(&format!("cannot activate item {item} of {bus_name}"), e);
                }
            },
            "cupertino-activate-gtk",
        );
    }

    /// Serve a request for GMenuModel exporters; any other is handed back.
    pub(super) fn handle(&mut self, request: Request) -> Option<Request> {
        match request {
            Request::FetchGtk(props) => self.fetch(props),
            Request::ReleaseGtk(props) => self.release(&props),
            Request::ActivateGtk { props, item, token } => {
                self.activate(&props, item, token.as_deref())
            }
            other => return Some(other),
        }
        None
    }
}

/// Forward the changes of the exporter of `props` to the worker for as long as the returned task lives.
fn follow_gtk(
    conn: &Connection,
    tx: &async_channel::Sender<Work>,
    props: GtkMenuProps,
    mut changes: BoxStream<'static, GtkChange>,
) -> zbus::Task<()> {
    let tx = tx.clone();
    conn.executor().clone().spawn(
        async move {
            let (applied, done) = async_channel::bounded(1);
            while let Some(change) = changes.next().await {
                let work = Work::GtkChanged(props.clone(), change, applied.clone());
                if tx.send(work).await.is_err() || done.recv().await.is_err() {
                    return;
                }
            }
        },
        "cupertino-follow-gtk",
    )
}
