//! One DBusMenu exporter followed for as long as a bar shows its menu: an application's menus or a status item's.
//!
//! A [`Follower`] is a task of the session bus worker. It reads the exporter's tree, keeps it current from the
//! exporter's `LayoutUpdated` and `ItemsPropertiesUpdated` signals and reports every change as a whole tree
//! ([`Event::Layout`]), so what a bar shows and the ids it sends back are the exporter's current ones. Before a menu
//! shows it sends `AboutToShow` and reads again what the exporter says has changed.
//!
//! The task makes one call at a time and applies the replies in the order asked, so an older reply never replaces a
//! newer layout. Every call has a bounded wait ([`Waits`]): an exporter that answers late or never holds up its own
//! menu's updates for that long and nothing else, since the bars only ever hear from the task. The exporter's signals
//! are taken while a call is awaited too: left in the connection's queue they would fill it, and a full queue stops
//! the connection, the awaited reply with it.

use crate::dbus::dbusmenu::{DBusMenuClient, MenuTree, MenuUpdate, Reread};
use crate::item::ItemId;
use crate::source::{KdeAppMenu, MenuSource};
use crate::worker::{report, Event, Out};
use async_channel::TryRecvError;
use futures_util::future::{pending, select, Either};
use futures_util::{Stream, StreamExt};
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

/// How long a follower waits for an exporter's answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Waits {
    /// For the answer to `AboutToShow`. The menu is on screen meanwhile with the rows last read; past the wait it
    /// stays as it is until the exporter signals a change.
    pub about_to_show: Duration,
    /// For a layout.
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
}

/// Handle on a followed exporter; dropping it ends the following.
pub(crate) struct Follower {
    commands: async_channel::Sender<Command>,
    _task: zbus::Task<()>,
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
        Follower {
            commands,
            _task: task,
        }
    }

    pub(crate) fn send(&self, command: Command) {
        // the task is gone only when its exporter's object could not be addressed, which it reported
        let _ = self.commands.try_send(command);
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
    };
    if patient {
        // many exporters do not implement it and the tree is worth reading anyway
        let told = state.client.about_to_show(ItemId(0));
        let _ = state.signals.within(waits.about_to_show, told).await;
    }
    state.load().await;
    loop {
        // a command goes before the signals that wait, so an exporter that signals without pause keeps no menu from
        // being told that it shows
        let step = if state.signals.is_empty() {
            next(&mut state.signals, &commands, state.retry).await
        } else {
            match commands.try_recv() {
                Ok(command) => Next::Command(command),
                Err(TryRecvError::Empty) => Next::Changes,
                Err(TryRecvError::Closed) => Next::Ended,
            }
        };
        match step {
            Next::Signal(update) => state.signals.keep(update),
            Next::Changes => {
                // those of the interval join the ones that wait
                let interval = state.signals.within(CHANGE_INTERVAL, pending::<()>());
                interval.await;
                state.changed().await;
            }
            Next::Command(Command::Show(item)) => state.show(ItemId(item)).await,
            Next::Command(Command::Publish) => {
                if state.tree.is_unread() {
                    state.load().await;
                } else {
                    state.emit();
                }
            }
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
        if let Some(root) = self.tree.root() {
            self.out.emit(Event::Layout {
                source: MenuSource::Kde(self.menu.clone()),
                root: Box::new(root.clone()),
            });
        }
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

    /// Read the layouts below `parents` and report the tree; the whole tree when one of them cannot be read.
    async fn read_parts(&mut self, parents: &[ItemId]) {
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
        let mut dirty = false;
        for update in batch {
            match update {
                MenuUpdate::Layout { revision, parent } => layouts.push((revision, parent)),
                MenuUpdate::Properties { updated, removed } => {
                    dirty |= self.tree.apply_properties(&updated, &removed);
                }
                // the bar opens menus for the person only
                MenuUpdate::ActivationRequested { .. } => {}
            }
        }
        if overflowed || (!layouts.is_empty() && self.tree.is_unread()) {
            return self.load().await;
        }
        match self.tree.reread(&layouts) {
            Some(Reread::Whole) => self.load().await,
            Some(Reread::Parents(parents)) => self.read_parts(&parents).await,
            None if dirty => self.emit(),
            None => {}
        }
    }

    /// The menu of `item` is about to show: tell the exporter and read again what it says has changed. Rows the tree
    /// never read are read whatever the answer, and so is the menu of an exporter that fails the call, which cannot
    /// say; one that does not answer in time is left alone until it signals.
    async fn show(&mut self, item: ItemId) {
        let told = self.client.about_to_show(item);
        let asked = self.signals.within(self.waits.about_to_show, told).await;
        if self.tree.is_unread() {
            return self.load().await;
        }
        let read = match asked {
            Some(Ok(update)) => update || self.tree.is_unloaded(item),
            Some(Err(e)) => {
                self.report_show(e);
                true
            }
            None => {
                self.report_show(NoAnswer(self.waits.about_to_show));
                false
            }
        };
        if !read {
            return;
        }
        if item.0 == 0 {
            self.load().await;
        } else {
            self.read_parts(&[item]).await;
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
}
