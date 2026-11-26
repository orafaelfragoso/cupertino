//! One DBusMenu exporter followed for as long as a bar shows its menu: an application's menus or a status item's.
//!
//! A [`Follower`] is a task of the session bus worker. It reads the exporter's tree and its menu's own properties,
//! keeps them current from the exporter's `LayoutUpdated`, `ItemsPropertiesUpdated` and `PropertiesChanged` signals
//! and reports every change as a whole tree ([`Event::Layout`]), so what a bar shows and the ids it sends back are the
//! exporter's current ones. Before a menu shows it sends `AboutToShow` and reads again what the exporter says has
//! changed; as a menu shows and hides it sends `opened` and `closed`, and `closed` for every menu still open when the
//! follower is let go. An `ItemActivationRequested` is reported to the bars ([`Event::ActivationRequested`]).
//!
//! Commands that come together are sent together: several `AboutToShow` as one `AboutToShowGroup`, several events
//! as one `EventGroup`, for an exporter that has them (protocol version 3 on, or one that does not say); one that
//! answers that it has no such method is sent the single calls from then on.
//!
//! The task makes one call at a time and applies the replies in the order asked, so an older reply never replaces a
//! newer layout. Every call has a bounded wait ([`Waits`]): an exporter that answers late or never holds up its own
//! menu's updates for that long and nothing else, since the bars only ever hear from the task. The exporter's signals
//! are taken while a call is awaited too: left in the connection's queue they would fill it, and a full queue stops
//! the connection, the awaited reply with it.

use crate::dbus::dbusmenu::{
    is_unknown_method, DBusMenuClient, MenuEvent, MenuTree, MenuUpdate, Reread,
};
use crate::item::ItemId;
use crate::source::{KdeAppMenu, MenuSource};
use crate::worker::{report, Event, Out};
use futures_util::future::{pending, select, Either};
use futures_util::{Stream, StreamExt};
use std::collections::VecDeque;
use std::fmt;
use std::future::Future;
use std::pin::pin;
use std::time::Duration;
use zbus::Connection;

/// Signals that come within this long of the first are read as one change, so an exporter that rebuilds its menu item
/// by item, or changes without pause, costs one read and one redraw per interval.
const CHANGE_INTERVAL: Duration = Duration::from_millis(50);
/// At most this many signals are kept for one change; past it the whole tree is read instead.
const CHANGE_BATCH: usize = 256;
/// The first wait before a menu that could not be read is read again; it doubles up to [`RETRY_LAST`], then the failure
/// is reported. An item or a window can name its menu before the menu's object answers: Chromium and Electron announce
/// a window's menu seconds before they export it. A window's follower ends with its holder, so the waits do too.
const RETRY_FIRST: Duration = Duration::from_millis(100);
/// The last wait of the doubling from [`RETRY_FIRST`]: 12.7 s in all before the failure is reported.
const RETRY_LAST: Duration = Duration::from_millis(6400);
/// At most this many commands of one kind are sent as one group call.
const GROUP_LIMIT: usize = 32;
/// At most this many menus are held as open; an exporter's menus nest far less deep, so more means ids that were
/// never closed (an exporter that renumbered its tree under open menus).
const OPEN_LIMIT: usize = 32;

/// How long a follower waits for an exporter's answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Waits {
    /// For the answer to `AboutToShow`, and to an event. The menu is on screen meanwhile with the rows last read; past
    /// the wait it stays as it is until the exporter signals a change.
    pub about_to_show: Duration,
    /// For a layout, and for the menu's own properties.
    pub read: Duration,
}

impl Default for Waits {
    fn default() -> Self {
        Self {
            about_to_show: Duration::from_secs(1),
            read: Duration::from_secs(5),
        }
    }
}

/// What the worker asks of a follower.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Command {
    /// The menu of this item (0 for the whole menu) is about to show.
    Show(i32),
    /// Report the tree again (to a bar that starts showing it), reading it first when it was never read.
    Publish,
    /// Send `opened` or `closed` for the menu of `item`, at `time` (milliseconds of the input clock).
    Event {
        item: i32,
        event: MenuEvent,
        time: u32,
    },
    /// Nothing shows the menu any more: close what is still open, then end.
    Finish,
}

/// Handle on a followed exporter; dropping it ends the following at once, [`Follower::finish`] once the menus still
/// open are closed.
pub(crate) struct Follower {
    commands: async_channel::Sender<Command>,
    task: zbus::Task<()>,
}

impl Follower {
    /// Follow `menu` and report its tree; a first read that fails is made again for a while. `patient` is for a status
    /// item's menu: it is told that it is about to show before the first read, as an exporter that fills its menu only
    /// then expects.
    pub(crate) fn start(
        conn: &Connection,
        out: Out,
        menu: KdeAppMenu,
        patient: bool,
        waits: Waits,
    ) -> Follower {
        let (commands, received) = async_channel::unbounded();
        let task = conn.executor().clone().spawn(
            run(conn.clone(), out, menu, patient, waits, received),
            "cupertino-follow-menu",
        );
        Follower { commands, task }
    }

    pub(crate) fn send(&self, command: Command) {
        // the task is gone only when its exporter's object could not be addressed, which it reported
        let _ = self.commands.try_send(command);
    }

    /// Let the exporter go: the task tells it the menus still open closed, within its wait, and ends.
    pub(crate) fn finish(self) {
        self.send(Command::Finish);
        self.task.detach();
    }
}

/// The exporter's signals: those that came and wait to be applied, and the stream of the next.
struct Signals<S> {
    /// `None` when the signals could not be followed, or the connection closed.
    stream: Option<S>,
    waiting: Vec<MenuUpdate>,
    /// More than [`CHANGE_BATCH`] came: those past it were dropped, so the whole tree is to be read.
    overflowed: bool,
}

impl<S: Stream<Item = MenuUpdate> + Unpin> Signals<S> {
    fn keep(&mut self, update: MenuUpdate) {
        if self.waiting.len() < CHANGE_BATCH {
            self.waiting.push(update);
        } else {
            self.overflowed = true;
        }
    }

    fn is_empty(&self) -> bool {
        self.waiting.is_empty() && !self.overflowed
    }

    /// The next signal; `None` when the stream ended. Without a stream it never comes.
    async fn next(&mut self) -> Option<MenuUpdate> {
        match &mut self.stream {
            Some(stream) => stream.next().await,
            None => pending().await,
        }
    }

    /// The answer of `call`, `None` when it did not come within `wait`. Signals that come meanwhile are kept.
    async fn within<T>(&mut self, wait: Duration, call: impl Future<Output = T>) -> Option<T> {
        let mut call = pin!(call);
        let mut timer = pin!(async_io::Timer::after(wait));
        loop {
            let signal = {
                let next = pin!(self.next());
                match select(call.as_mut(), select(timer.as_mut(), next)).await {
                    Either::Left((answer, _)) => return Some(answer),
                    Either::Right((Either::Left(_), _)) => return None,
                    Either::Right((Either::Right((signal, _)), _)) => signal,
                }
            };
            match signal {
                Some(update) => self.keep(update),
                None => self.stream = None,
            }
        }
    }
}

enum Next {
    Signal(MenuUpdate),
    /// Signals wait to be applied.
    Changes,
    Command(Command),
    Retry,
    Ended,
}

/// What an idle follower handles next: a signal, a command, or the end of the wait before a read is made again.
async fn next<S: Stream<Item = MenuUpdate> + Unpin>(
    signals: &mut Signals<S>,
    commands: &async_channel::Receiver<Command>,
    retry: Option<Duration>,
) -> Next {
    let timer = async {
        match retry {
            Some(wait) => {
                async_io::Timer::after(wait).await;
            }
            None => pending().await,
        }
    };
    let (command, timer) = (pin!(commands.recv()), pin!(timer));
    match select(pin!(signals.next()), select(command, timer)).await {
        Either::Left((Some(update), _)) => Next::Signal(update),
        Either::Right((Either::Left((Ok(command), _)), _)) => Next::Command(command),
        Either::Right((Either::Right(_), _)) => Next::Retry,
        // the connection closed, or the worker dropped the follower
        Either::Left((None, _)) | Either::Right((Either::Left((Err(_), _)), _)) => Next::Ended,
    }
}

async fn run(
    conn: Connection,
    out: Out,
    menu: KdeAppMenu,
    patient: bool,
    waits: Waits,
    commands: async_channel::Receiver<Command>,
) {
    let client = match DBusMenuClient::connect(&conn, &menu.service, &menu.path).await {
        Ok(client) => client,
        Err(e) => {
            report(
                &format!("cannot load the menu of {}{}", menu.service, menu.path),
                e,
            );
            return out.emit(Event::LayoutFailed {
                source: MenuSource::Kde(menu),
            });
        }
    };
    // followed before the first read, so no change between the two is missed
    let stream = match client.updates().await {
        Ok(updates) => Some(updates),
        Err(e) => {
            report(
                &format!("cannot follow the menu of {}{}", menu.service, menu.path),
                e,
            );
            None
        }
    };
    let mut state = State {
        client,
        menu,
        out,
        waits,
        signals: Signals {
            stream,
            waiting: Vec::new(),
            overflowed: false,
        },
        tree: MenuTree::default(),
        retry: Some(RETRY_FIRST),
        show_reported: false,
        open: Vec::new(),
        show_groups: true,
        event_groups: true,
        queue: VecDeque::new(),
        last_time: 0,
    };
    state.read_menu_properties().await;
    if patient {
        // many exporters do not implement it and the tree is worth reading anyway
        let told = state.client.about_to_show(ItemId(0));
        let _ = state.signals.within(waits.about_to_show, told).await;
    }
    state.load().await;
    loop {
        // a command goes before the signals that wait, so an exporter that signals without pause keeps no menu from
        // being told that it shows
        let step = if let Some(command) = state.queue.pop_front() {
            Next::Command(command)
        } else if state.signals.is_empty() {
            next(&mut state.signals, &commands, state.retry).await
        } else {
            match commands.try_recv() {
                Ok(command) => Next::Command(command),
                Err(async_channel::TryRecvError::Empty) => Next::Changes,
                Err(async_channel::TryRecvError::Closed) => Next::Ended,
            }
        };
        // the commands that came with this one join it, so those of a kind go as one call
        while let Ok(command) = commands.try_recv() {
            state.queue.push_back(command);
        }
        match step {
            Next::Signal(update) => state.signals.keep(update),
            Next::Changes => {
                // those of the interval join the ones that wait
                let interval = state.signals.within(CHANGE_INTERVAL, pending::<()>());
                interval.await;
                state.changed().await;
            }
            Next::Command(Command::Show(item)) => {
                let mut items = vec![ItemId(item)];
                while let Some(Command::Show(more)) = state.queue.front().copied() {
                    if items.len() == GROUP_LIMIT {
                        break;
                    }
                    state.queue.pop_front();
                    if !items.contains(&ItemId(more)) {
                        items.push(ItemId(more));
                    }
                }
                state.show(&items).await;
            }
            Next::Command(Command::Event { item, event, time }) => {
                let mut events = vec![(ItemId(item), event, time)];
                while let Some(Command::Event { item, event, time }) = state.queue.front().copied()
                {
                    if events.len() == GROUP_LIMIT {
                        break;
                    }
                    state.queue.pop_front();
                    events.push((ItemId(item), event, time));
                }
                state.events(events).await;
            }
            Next::Command(Command::Publish) => {
                if state.tree.is_unread() {
                    state.load().await;
                } else {
                    state.emit();
                }
            }
            Next::Command(Command::Finish) => return state.finish().await,
            Next::Retry => {
                state.retry = state
                    .retry
                    .map(|wait| wait * 2)
                    .filter(|wait| *wait <= RETRY_LAST);
                state.load().await;
            }
            Next::Ended => return,
        }
    }
}

struct State<S> {
    client: DBusMenuClient,
    menu: KdeAppMenu,
    out: Out,
    waits: Waits,
    signals: Signals<S>,
    tree: MenuTree,
    /// The wait before a first read that failed is made again; `None` when it is not.
    retry: Option<Duration>,
    /// An `AboutToShow` that failed was reported: an exporter without the method fails at every opening.
    show_reported: bool,
    /// The menus the exporter was told opened and not yet closed, by their ids then.
    open: Vec<ItemId>,
    /// Whether `AboutToShowGroup` and `EventGroup` are worth sending: the version allows them and the exporter did not
    /// answer that it has no such method.
    show_groups: bool,
    event_groups: bool,
    /// Commands taken from the worker and not handled yet.
    queue: VecDeque<Command>,
    /// The time of the last event sent, milliseconds of the bar's input clock.
    last_time: u32,
}

/// An exporter that did not answer within its wait.
struct NoAnswer(Duration);

impl fmt::Display for NoAnswer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "no answer within {} ms", self.0.as_millis())
    }
}

impl<S: Stream<Item = MenuUpdate> + Unpin> State<S> {
    fn emit(&self) {
        if let Some(root) = self.tree.shown() {
            self.out.emit(Event::Layout {
                source: MenuSource::Kde(self.menu.clone()),
                root: Box::new(root),
            });
        }
    }

    /// Read the menu's own properties. An exporter without them (or without `org.freedesktop.DBus.Properties`) has
    /// the defaults: left to right, a normal status, no icon directories, a version not said.
    async fn read_menu_properties(&mut self) {
        let read = self.client.menu_properties();
        if let Some(Ok(props)) = self.signals.within(self.waits.read, read).await {
            self.tree.apply_menu_properties(&props, &[]);
        }
        let groups = self.tree.properties().has_groups();
        self.show_groups = groups;
        self.event_groups = groups;
    }

    /// A read failed. The first read is made again without a word while its waits last.
    fn failed(&mut self, error: impl fmt::Display) {
        let unread = self.tree.is_unread();
        if unread && self.retry.is_some() {
            return;
        }
        let verb = if unread { "load" } else { "reload" };
        report(
            &format!(
                "cannot {verb} the menu of {}{}",
                self.menu.service, self.menu.path
            ),
            error,
        );
        if unread {
            self.out.emit(Event::LayoutFailed {
                source: MenuSource::Kde(self.menu.clone()),
            });
        }
    }

    /// Read the whole tree and report it.
    async fn load(&mut self) {
        let read = self.client.fetch(ItemId(0), -1);
        match self.signals.within(self.waits.read, read).await {
            Some(Ok((revision, root))) => {
                self.tree.apply_reply(ItemId(0), revision, root);
                self.retry = None;
                self.emit();
            }
            Some(Err(e)) => self.failed(e),
            None => self.failed(NoAnswer(self.waits.read)),
        }
    }

    /// Read the layout below `parent` into the tree; `false` when the tree has no such item any more or the exporter
    /// does not give it, so the whole tree is to be read.
    async fn read_part(&mut self, parent: ItemId) -> bool {
        let read = self.client.fetch(parent, -1);
        match self.signals.within(self.waits.read, read).await {
            Some(Ok((revision, layout))) => self.tree.apply_reply(parent, revision, layout),
            Some(Err(_)) | None => false,
        }
    }

    /// Read the layouts below `parents` and report the tree; the whole tree when one of them cannot be read, or is the
    /// root.
    async fn read_parts(&mut self, parents: &[ItemId]) {
        if parents.iter().any(|p| p.0 == 0) {
            return self.load().await;
        }
        for parent in parents {
            if !self.read_part(*parent).await {
                return self.load().await;
            }
        }
        self.emit();
    }

    /// Apply the signals that wait.
    async fn changed(&mut self) {
        let batch = std::mem::take(&mut self.signals.waiting);
        let overflowed = std::mem::take(&mut self.signals.overflowed);
        let mut layouts = Vec::new();
        let mut asked = Vec::new();
        let mut dirty = false;
        for update in batch {
            match update {
                MenuUpdate::Layout { revision, parent } => layouts.push((revision, parent)),
                MenuUpdate::Properties { updated, removed } => {
                    dirty |= self.tree.apply_properties(&updated, &removed);
                }
                MenuUpdate::Menu {
                    changed,
                    invalidated,
                } => {
                    dirty |= self.tree.apply_menu_properties(&changed, &invalidated);
                    let groups = self.tree.properties().has_groups();
                    self.show_groups &= groups;
                    self.event_groups &= groups;
                }
                MenuUpdate::ActivationRequested { id, .. } => asked.push(id),
            }
        }
        if overflowed || (!layouts.is_empty() && self.tree.is_unread()) {
            self.load().await;
        } else {
            match self.tree.reread(&layouts) {
                Some(Reread::Whole) => self.load().await,
                Some(Reread::Parents(parents)) => self.read_parts(&parents).await,
                None if dirty => self.emit(),
                None => {}
            }
        }
        // after the tree they may name an item of
        for id in asked {
            self.out.emit(Event::ActivationRequested {
                source: MenuSource::Kde(self.menu.clone()),
                item: id.0,
            });
        }
    }

    /// The menus of `items` are about to show: tell the exporter and read again what it says has changed. Rows the
    /// tree never read are read whatever the answer, and so are the menus of an exporter that fails the call, which
    /// cannot say; one that does not answer in time is left alone until it signals.
    async fn show(&mut self, items: &[ItemId]) {
        let asked = if items.len() > 1 && self.show_groups {
            let told = self.client.about_to_show_group(items);
            match self.signals.within(self.waits.about_to_show, told).await {
                Some(Err(e)) if is_unknown_method(&e) => {
                    self.show_groups = false;
                    None
                }
                asked => Some(asked),
            }
        } else {
            None
        };
        let read: Vec<ItemId> = match asked {
            Some(Some(Ok(update))) => items
                .iter()
                .copied()
                .filter(|item| update.contains(item) || self.tree.is_unloaded(*item))
                .collect(),
            Some(Some(Err(e))) => {
                self.report_show(e);
                items.to_vec()
            }
            Some(None) => {
                self.report_show(NoAnswer(self.waits.about_to_show));
                Vec::new()
            }
            // one call each
            None => {
                let mut read = Vec::new();
                for item in items {
                    if self.show_one(*item).await {
                        read.push(*item);
                    }
                }
                read
            }
        };
        if self.tree.is_unread() {
            return self.load().await;
        }
        if !read.is_empty() {
            self.read_parts(&read).await;
        }
    }

    /// `AboutToShow` for `item`: whether its layout is to be read again.
    async fn show_one(&mut self, item: ItemId) -> bool {
        let told = self.client.about_to_show(item);
        match self.signals.within(self.waits.about_to_show, told).await {
            Some(Ok(update)) => update || self.tree.is_unloaded(item),
            Some(Err(e)) => {
                self.report_show(e);
                true
            }
            None => {
                self.report_show(NoAnswer(self.waits.about_to_show));
                false
            }
        }
    }

    fn report_show(&mut self, error: impl fmt::Display) {
        if !std::mem::replace(&mut self.show_reported, true) {
            report(
                &format!("AboutToShow on {}{}", self.menu.service, self.menu.path),
                error,
            );
        }
    }

    /// Send `opened` and `closed` events. A menu is told it closed only when it was told it opened or the tree holds
    /// its item, so a bar that changed exporters under an open menu sends the new one no stray `closed`.
    async fn events(&mut self, events: Vec<(ItemId, MenuEvent, u32)>) {
        let mut sent = Vec::with_capacity(events.len());
        for (item, event, time) in events {
            self.last_time = time;
            match event {
                MenuEvent::Opened => {
                    if !self.open.contains(&item) {
                        if self.open.len() == OPEN_LIMIT {
                            self.open.remove(0);
                        }
                        self.open.push(item);
                    }
                }
                MenuEvent::Closed => {
                    let was_open = self.open.contains(&item);
                    self.open.retain(|open| *open != item);
                    if !was_open && !self.tree.holds(item) {
                        continue;
                    }
                }
                MenuEvent::Clicked => {}
            }
            sent.push((item, event, time));
        }
        self.send_events(&sent).await;
    }

    /// Send `events`, as one group when there are several and the exporter has the call. The events are advisory: an
    /// exporter that fails them, or left the bus, is not reported.
    async fn send_events(&mut self, events: &[(ItemId, MenuEvent, u32)]) {
        if events.len() > 1 && self.event_groups {
            let call = self.client.event_group(events);
            match self.signals.within(self.waits.about_to_show, call).await {
                Some(Err(e)) if is_unknown_method(&e) => self.event_groups = false,
                _ => return,
            }
        }
        for (item, event, time) in events {
            let call = self.client.event(*item, *event, *time);
            let _ = self.signals.within(self.waits.about_to_show, call).await;
        }
    }

    /// Nothing shows the menu any more: the menus still open closed, the deepest first.
    async fn finish(&mut self) {
        let pending: Vec<_> = std::mem::take(&mut self.queue)
            .into_iter()
            .filter_map(|command| match command {
                Command::Event { item, event, time } => Some((ItemId(item), event, time)),
                _ => None,
            })
            .collect();
        if !pending.is_empty() {
            self.events(pending).await;
        }
        // the time of the last event the bar sent, the latest of its input clock the follower knows
        let time = self.last_time;
        let closing: Vec<_> = self
            .open
            .drain(..)
            .rev()
            .map(|item| (item, MenuEvent::Closed, time))
            .collect();
        self.send_events(&closing).await;
    }
}
