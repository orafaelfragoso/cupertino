//! What the calls of later launches and of the desktop do ([`crate::instance`]): `Activate` brings the most recently
//! opened browser window forward (or, with none open, the hidden progress window while it has something to list, else
//! a new window on the home folder), `Open` opens a window on each folder, `ShowItems` selects each item in a window on
//! its folder (one window a folder, a window already showing it when there is one) once a thread of its own has found
//! which of them exist, so an item on a mount that does not answer never holds the windows still, and the actions show the progress
//! window or open a new window. That thread resolves each `..` on the disk too (an `Open` of a folder spelled with one
//! waits for it as well), and a call whose items are not all looked up within [`LOOK_UP_DEADLINE`] is answered with
//! those that were. A click on the notice of a file operation's end shows the progress window with it, as
//! `Activate` does when nothing is left to list. The window a call opens or brings forward is raised with the call's
//! activation token once it is shown.

use std::collections::BTreeMap;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cupertino_kit::prelude::*;

use super::{progress, window_key, woken, Message, Model};
use crate::instance::{Action, Call, Request, Target};
use crate::notice::{Action as Failed, Click};

/// Raise the window `key` with `token` once it has been shown: at once when it already has, else after its first frame,
/// so the compositor finds a mapped window for the token.
pub(super) fn raise(key: WindowKey, token: Option<String>) -> Command<Message> {
    Command::OnPresented {
        window: key.clone(),
        message: Message::Raise(key, token),
    }
}

impl Model {
    /// Do what each waiting call asks, answer it, and wait for the next.
    pub(super) fn remote_calls(&mut self) -> Vec<Command<Message>> {
        let Some(instance) = &self.session.instance else {
            return Vec::new();
        };
        let calls = instance.take();
        let mut commands = vec![woken(instance.woken(), Message::Remote)];
        if !calls.is_empty() {
            // the call a Files started without a window waits for came: it exits once nothing is left open
            self.awaiting_call = false;
        }
        for call in calls {
            commands.extend(self.remote_call(call));
        }
        commands
    }

    /// Do what `call` asks and answer it; `ShowItems`, and `Open` of a folder spelled with `..`, are answered once
    /// their items are looked up and shown ([`Model::look_up`]).
    fn remote_call(&mut self, call: Call) -> Vec<Command<Message>> {
        match &call.request {
            Request::ShowItems(_) => return self.look_up(call),
            Request::Open(targets) if targets.iter().any(goes_up) => return self.look_up(call),
            _ => {}
        }
        let commands = self.take_call(&call);
        call.done();
        commands
    }

    fn take_call(&mut self, call: &Call) -> Vec<Command<Message>> {
        let token = call.token.clone();
        match &call.request {
            Request::Activate => self.activate(token),
            Request::Open(targets) => self.open_targets(targets, token),
            Request::ShowItems(_) => Vec::new(),
            Request::Action(Action::NewWindow) => self.open_home(token),
            Request::Action(Action::ShowProgress) => {
                // nothing to list: the progress window stays closed, as Window › Show Progress is disabled then
                self.show_progress(token).unwrap_or_default()
            }
        }
    }

    /// A window on each folder of `targets`, the last raised with `token`.
    fn open_targets(&mut self, targets: &[Target], token: Option<String>) -> Vec<Command<Message>> {
        let mut commands = Vec::new();
        let mut last = None;
        for target in targets {
            let Some(folder) = self.folder_of(target) else {
                continue;
            };
            if let Some((id, wake)) = self.open_window(folder) {
                commands.push(wake);
                last = Some(id);
            }
        }
        // a token raises one window: the last one opened, which the others open beneath
        commands.extend(last.map(|id| raise(window_key(id), token)));
        commands
    }

    /// Bring Files forward: its most recently opened browser window; with none open, the progress window when it is
    /// hidden with something to list (a file operation runs on); else a new window on the home folder.
    fn activate(&mut self, token: Option<String>) -> Vec<Command<Message>> {
        if let Some(&id) = self.windows.keys().next_back() {
            return vec![raise(window_key(id), token)];
        }
        let jobs = &self.session.jobs;
        if !jobs.visible() && !jobs.is_empty() {
            if let Some(commands) = self.show_progress(token.clone()) {
                return commands;
            }
        }
        self.open_home(token)
    }

    /// Show the progress window raised with `token`, when it has something to list.
    fn show_progress(&mut self, token: Option<String>) -> Option<Vec<Command<Message>>> {
        let jobs = &mut self.session.jobs;
        jobs.show();
        jobs.visible()
            .then(|| vec![raise(WindowKey(Key::from(progress::KEY)), token)])
    }

    /// A notice of a file operation's end was clicked: the progress window shows its failure again, or what still
    /// runs; with neither, Files comes forward as a launch brings it. Then the next click is waited for.
    pub(super) fn noticed(&mut self, click: Option<Click>) -> Vec<Command<Message>> {
        let Some(Click { tag, token }) = click else {
            return Vec::new();
        };
        let session = &mut self.session;
        let failure = session
            .noticed
            .iter()
            .find(|(id, _)| *id == tag)
            .and_then(|(_, failure)| failure.clone());
        if let Some(failure) = failure {
            session.jobs.failed(failure);
        }
        let mut commands = match self.show_progress(token.clone()) {
            Some(commands) => commands,
            None => self.activate(token),
        };
        commands.extend(self.session.notice_clicks());
        commands
    }

    /// A new window on the home folder, raised with `token`.
    fn open_home(&mut self, token: Option<String>) -> Vec<Command<Message>> {
        let home = self.session.home.clone();
        match self.open_window(home) {
            Some((id, wake)) => vec![wake, raise(window_key(id), token)],
            None => Vec::new(),
        }
    }

    /// Find out on a thread of its own what the items of `call` are, each `..` resolved on the disk, and for
    /// `ShowItems` which of them exist, so an item on a mount that does not answer holds no window still. The call is
    /// answered once that is known, or with what is known by then [`LOOK_UP_DEADLINE`] after it came
    /// ([`Model::looked_up`]).
    fn look_up(&mut self, call: Call) -> Vec<Command<Message>> {
        let (paths, exists) = match &call.request {
            Request::ShowItems(targets) => (folder_paths(targets), true),
            Request::Open(targets) => (folder_paths(targets), false),
            _ => (Vec::new(), false),
        };
        self.look_ups.start(call, paths, exists)
    }

    /// The items of the call `id` were looked up, or its deadline passed first: show or open what was found, and
    /// answer the call. Nothing once it was answered.
    pub(super) fn looked_up(&mut self, id: u64) -> Vec<Command<Message>> {
        let Some((call, found)) = self.look_ups.take(id) else {
            return Vec::new();
        };
        let token = call.token.clone();
        let commands = match &call.request {
            Request::ShowItems(targets) => {
                let targets = self.found(targets, &found);
                self.show_items(&targets, token)
            }
            Request::Open(targets) => {
                let targets = self.found(targets, &found);
                self.open_targets(&targets, token)
            }
            _ => Vec::new(),
        };
        call.done();
        commands
    }

    /// `targets` with each path as its look-up found it (`found`, one a path in order): resolved, where it exists. One
    /// that does not, or that was not looked up by the deadline (a path past the end of `found`), is a notice and is
    /// left out.
    fn found(&self, targets: &[Target], found: &[Found]) -> Vec<Target> {
        let mut found = found.iter();
        targets
            .iter()
            .filter_map(|target| {
                let Target::Folder(item) = target else {
                    return Some(target.clone());
                };
                let e = match found.next() {
                    Some(Found::Here(path)) => return Some(Target::Folder(path.clone())),
                    Some(Found::Missing(e)) => e.error(),
                    None => io::Error::new(io::ErrorKind::TimedOut, "not looked up in time"),
                };
                self.session.failed(Failed::Open, &[item], &e);
                None
            })
            .collect()
    }

    /// Show each item of `targets` selected in a window on its folder, raised with `token`: the items of one folder in
    /// one window, the newest window that shows the folder (without a search) when there is one, else a new one; the
    /// trash is opened itself, and the root shows itself.
    fn show_items(&mut self, targets: &[Target], token: Option<String>) -> Vec<Command<Message>> {
        // each folder with its items, in the order the call names them
        let mut folders: Vec<(PathBuf, Vec<PathBuf>)> = Vec::new();
        for target in targets {
            let (folder, item) = match target {
                Target::Trash => match self.folder_of(target) {
                    Some(folder) => (folder, None),
                    None => continue,
                },
                Target::Folder(item) => match item.parent() {
                    Some(folder) => (folder.to_path_buf(), Some(item.clone())),
                    None => (item.clone(), None),
                },
            };
            match folders.iter_mut().find(|(f, _)| *f == folder) {
                Some((_, items)) => items.extend(item),
                None => folders.push((folder, item.into_iter().collect())),
            }
        }
        let mut commands = Vec::new();
        let mut last = None;
        for (folder, items) in folders {
            let shown = self
                .windows
                .iter()
                .rev()
                .find(|(_, w)| w.browser.dir() == folder && w.search.is_empty())
                .map(|(&id, _)| id);
            let id = match shown {
                Some(id) => id,
                None => match self.open_window(folder) {
                    Some((id, wake)) => {
                        commands.push(wake);
                        id
                    }
                    None => continue,
                },
            };
            last = Some(id);
            if items.is_empty() {
                continue;
            }
            let window = self.windows.get_mut(&id).expect("the window is open");
            window.browser.reveal(items);
            // a folder already read is selected at once; a new window's once its listing is read
            if let Some(path) = window.browser.take_revealed() {
                commands.extend(window.focus_item(id, &path));
            }
        }
        // a token raises one window: the last one shown, which the others open beneath
        commands.extend(last.map(|id| raise(window_key(id), token)));
        commands
    }

    /// The folder a window shows for `target`; a trash that cannot be made is a notice.
    fn folder_of(&self, target: &Target) -> Option<PathBuf> {
        let session = &self.session;
        target
            .folder(&session.trash)
            .map_err(|e| session.failed(Failed::Open, &[session.trash.home_trash()], &e))
            .ok()
    }
}

/// Whether `target` is a folder spelled with `..`, which only the disk resolves.
fn goes_up(target: &Target) -> bool {
    matches!(target, Target::Folder(path) if path.components().any(|c| c == Component::ParentDir))
}

/// The paths of the folders and items of `targets`, in order; the trash is none.
fn folder_paths(targets: &[Target]) -> Vec<PathBuf> {
    targets
        .iter()
        .filter_map(|t| match t {
            Target::Folder(path) => Some(path.clone()),
            Target::Trash => None,
        })
        .collect()
}

/// The most calls whose items are looked up at once.
pub const MAX_LOOK_UPS: usize = 8;

/// How long a call waits for its items to be looked up before it is answered with those found by then: a mount that
/// does not answer holds the caller no longer.
pub const LOOK_UP_DEADLINE: Duration = Duration::from_secs(4);

/// The most look-up threads alive at once, those that a mount which does not answer still holds past their call's
/// deadline among them; past it a call is refused until one of them ends.
pub(super) const MAX_LOOK_UP_THREADS: usize = 32;

/// What tells, on a look-up thread, whether the item at a path exists.
pub type LookUp = Arc<dyn Fn(&Path) -> io::Result<()> + Send + Sync>;

/// Whether the item at `path` exists: its own status, a symlink's and not its target's.
pub(super) fn look_up_item(path: &Path) -> io::Result<()> {
    std::fs::symlink_metadata(path).map(drop)
}

/// The calls whose items are being looked up, each on a thread of its own, by the id their message carries.
pub(super) struct LookUps {
    waiting: BTreeMap<u64, Waiting>,
    next: u64,
    look_up: LookUp,
    /// The look-up threads alive, those of calls already answered among them.
    threads: Arc<AtomicUsize>,
}

/// A call waiting for its items, and what its thread found of them so far.
struct Waiting {
    call: Call,
    found: Arc<Mutex<Vec<Found>>>,
}

impl LookUps {
    pub(super) fn new(look_up: LookUp) -> LookUps {
        LookUps {
            waiting: BTreeMap::new(),
            next: 0,
            look_up,
            threads: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Whether no call waits for its items.
    pub(super) fn is_empty(&self) -> bool {
        self.waiting.is_empty()
    }

    /// Look the `paths` of `call` up on a thread of its own, each `..` resolved, and with `exists` whether each item
    /// exists: the call's message comes once they are looked up, and at the latest [`LOOK_UP_DEADLINE`] later. Past
    /// [`MAX_LOOK_UPS`] calls waiting, or [`MAX_LOOK_UP_THREADS`] threads alive, the call is refused.
    fn start(&mut self, call: Call, paths: Vec<PathBuf>, exists: bool) -> Vec<Command<Message>> {
        if self.waiting.len() >= MAX_LOOK_UPS {
            call.refuse("too many items are being looked up");
            return Vec::new();
        }
        let look = exists.then(|| Arc::clone(&self.look_up));
        let looking = match look_up_items(paths, look, &self.threads) {
            Ok(looking) => looking,
            Err(why) => {
                call.refuse(why);
                return Vec::new();
            }
        };
        let id = self.next;
        self.next += 1;
        self.waiting.insert(
            id,
            Waiting {
                call,
                found: looking.found,
            },
        );
        let done = looking.done;
        vec![
            Command::Perform(Box::pin(async move {
                let _ = done.recv().await;
                Message::LookedUp(id)
            })),
            Command::After {
                delay: LOOK_UP_DEADLINE,
                message: Message::LookedUp(id),
            },
        ]
    }

    /// The call `id` and what was found of its items by now, one a path in order; none once it was taken.
    fn take(&mut self, id: u64) -> Option<(Call, Vec<Found>)> {
        let waiting = self.waiting.remove(&id)?;
        let found = lock(&waiting.found).clone();
        Some((waiting.call, found))
    }
}

/// What a look-up thread found so far, and what closes once it is done.
struct Looking {
    found: Arc<Mutex<Vec<Found>>>,
    done: async_channel::Receiver<()>,
}

/// One look-up thread alive, counted in the threads it holds until it ends.
struct Held(Arc<AtomicUsize>);

impl Held {
    /// One more thread, unless [`MAX_LOOK_UP_THREADS`] are alive.
    fn take(threads: &Arc<AtomicUsize>) -> Option<Held> {
        threads
            .try_update(Ordering::AcqRel, Ordering::Acquire, |alive| {
                (alive < MAX_LOOK_UP_THREADS).then_some(alive + 1)
            })
            .ok()?;
        Some(Held(Arc::clone(threads)))
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

fn lock(found: &Mutex<Vec<Found>>) -> std::sync::MutexGuard<'_, Vec<Found>> {
    found
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// `path` with each `..` resolved on the disk, as opening it resolves it: the part up to its last `..` made
/// canonical (the `..` of a symlinked folder is its target's parent), the rest kept as spelled. A path without `..`
/// is kept as it is, without reading the disk.
fn resolve(path: &Path) -> io::Result<PathBuf> {
    let parts: Vec<Component> = path.components().collect();
    let Some(last) = parts.iter().rposition(|c| *c == Component::ParentDir) else {
        return Ok(path.to_path_buf());
    };
    let mut resolved = std::fs::canonicalize(parts[..=last].iter().collect::<PathBuf>())?;
    resolved.extend(&parts[last + 1..]);
    Ok(resolved)
}

/// An item a call names as its thread found it: where it is, each `..` resolved, or why it is not there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Found {
    Here(PathBuf),
    Missing(LookUpError),
}

/// Why an item could not be looked up: what an [`io::Error`] says, kept so the message carrying it can be compared.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LookUpError {
    kind: io::ErrorKind,
    code: Option<i32>,
}

impl LookUpError {
    fn of(e: &io::Error) -> LookUpError {
        LookUpError {
            kind: e.kind(),
            code: e.raw_os_error(),
        }
    }

    fn error(self) -> io::Error {
        match self.code {
            Some(code) => io::Error::from_raw_os_error(code),
            None => self.kind.into(),
        }
    }
}

/// Resolve each of `paths` ([`resolve`]) and, given `look`, look it up with it, on a thread of its own counted in
/// `threads`: what was found is added as each path is, in order, and the channel closes once every path was. Refused
/// with why when [`MAX_LOOK_UP_THREADS`] are alive or no thread could be started.
fn look_up_items(
    paths: Vec<PathBuf>,
    look: Option<LookUp>,
    threads: &Arc<AtomicUsize>,
) -> Result<Looking, &'static str> {
    let held = Held::take(threads).ok_or("too many items are held by mounts that do not answer")?;
    let found = Arc::new(Mutex::new(Vec::with_capacity(paths.len())));
    let (sent, done) = async_channel::bounded::<()>(1);
    let into = Arc::clone(&found);
    std::thread::Builder::new()
        .name("files-look-up".into())
        .spawn(move || {
            let _held = held;
            for path in &paths {
                let item = resolve(path).and_then(|resolved| {
                    look.as_ref().map_or(Ok(()), |look| look(&resolved))?;
                    Ok(resolved)
                });
                lock(&into).push(match item {
                    Ok(path) => Found::Here(path),
                    Err(e) => Found::Missing(LookUpError::of(&e)),
                });
            }
            drop(sent);
        })
        .map_err(|_| "no thread could look the items up")?;
    Ok(Looking { found, done })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Condvar;

    /// A gate the look-ups wait at, as at a mount that does not answer, until it is opened (or a minute passes, so a
    /// failing test ends).
    #[derive(Default)]
    struct Gate {
        open: Mutex<bool>,
        opened: Condvar,
    }

    impl Gate {
        fn wait(&self) {
            let open = self.open.lock().unwrap();
            let _open = self
                .opened
                .wait_timeout_while(open, Duration::from_secs(60), |open| !*open)
                .unwrap();
        }

        fn release(&self) {
            *self.open.lock().unwrap() = true;
            self.opened.notify_all();
        }
    }

    #[test]
    fn items_are_looked_up_on_a_thread_of_their_own() {
        let gate = Arc::new(Gate::default());
        let held = Arc::clone(&gate);
        // the first look-up does not answer until released, as a folder on a mount that hangs
        let look: LookUp = Arc::new(move |path| {
            if path == Path::new("/here") {
                held.wait();
                Ok(())
            } else {
                Err(io::Error::from_raw_os_error(libc::ENOENT))
            }
        });
        let threads = Arc::new(AtomicUsize::new(0));
        let looking = look_up_items(vec!["/here".into(), "/gone".into()], Some(look), &threads)
            .expect("a thread");
        // the caller is back at once, with nothing found yet
        assert!(looking.done.try_recv().is_err());
        assert!(lock(&looking.found).is_empty());
        gate.release();
        let _ = zbus::block_on(looking.done.recv());
        let looked = lock(&looking.found).clone();
        assert_eq!(looked[0], Found::Here("/here".into()));
        let Found::Missing(e) = looked[1] else {
            panic!("{looked:?}");
        };
        assert_eq!(e.error().raw_os_error(), Some(libc::ENOENT));
        assert_eq!(e.error().kind(), io::ErrorKind::NotFound);
    }

    /// A thread a mount holds past its call's deadline still counts: past [`MAX_LOOK_UP_THREADS`] of them a look-up is
    /// refused, and once they end the next one starts.
    #[test]
    fn threads_held_by_a_mount_are_bounded_until_they_end() {
        let gate = Arc::new(Gate::default());
        let held = Arc::clone(&gate);
        let look: LookUp = Arc::new(move |_: &Path| {
            held.wait();
            Ok(())
        });
        let threads = Arc::new(AtomicUsize::new(0));
        let stuck: Vec<Looking> = (0..MAX_LOOK_UP_THREADS)
            .map(|_| {
                look_up_items(vec!["/hung".into()], Some(Arc::clone(&look)), &threads)
                    .expect("a thread")
            })
            .collect();
        assert!(
            look_up_items(vec!["/hung".into()], Some(Arc::clone(&look)), &threads).is_err(),
            "one more is refused"
        );
        gate.release();
        for looking in &stuck {
            let _ = zbus::block_on(looking.done.recv());
        }
        // a thread is counted out as it ends, after it closed its channel
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while threads.load(Ordering::Acquire) != 0 {
            assert!(std::time::Instant::now() < deadline, "the threads ended");
            std::thread::yield_now();
        }
        let looking =
            look_up_items(vec!["/hung".into()], Some(look), &threads).expect("taken again");
        let _ = zbus::block_on(looking.done.recv());
        assert_eq!(*lock(&looking.found), [Found::Here("/hung".into())]);
    }

    /// The `..` of a symlinked folder is its target's parent, as opening the path finds it, not the folder the link is
    /// in; a path without `..` is kept as spelled.
    #[test]
    fn a_parent_through_a_symlinked_folder_is_the_target_s_parent() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::create_dir_all(root.join("x/y")).unwrap();
        std::fs::write(root.join("x/b"), b"x").unwrap();
        std::fs::create_dir_all(root.join("a")).unwrap();
        std::os::unix::fs::symlink(root.join("x/y"), root.join("a/link")).unwrap();

        let through = root.join("a/link/../b");
        assert_eq!(resolve(&through).unwrap(), root.join("x/b"));
        assert_eq!(
            resolve(&root.join("a/link/../y/..")).unwrap(),
            root.join("x")
        );
        let kept = root.join("a/link/c");
        assert_eq!(resolve(&kept).unwrap(), kept, "no disk read without ..");
        let e = resolve(&root.join("a/nowhere/../b")).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::NotFound);

        // the look-up thread finds the item where opening it would
        let threads = Arc::new(AtomicUsize::new(0));
        let look: LookUp = Arc::new(look_up_item);
        let looking = look_up_items(vec![through], Some(look), &threads).expect("a thread");
        let _ = zbus::block_on(looking.done.recv());
        assert_eq!(*lock(&looking.found), [Found::Here(root.join("x/b"))]);
    }
}
