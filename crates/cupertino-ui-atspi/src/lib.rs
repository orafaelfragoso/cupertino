//! Publishes a CupertinoKit application's accessibility tree to assistive technology over AT-SPI.
//!
//! [`AtspiAdapter`] is core's [`AccessibilityAdapter`] on AccessKit's Unix adapter. Each window is a top-level AT-SPI
//! object. Nothing is exported until an assistive technology is on the accessibility bus; core builds no tree until
//! then. AccessKit runs on an executor thread of its own; requests from assistive technology reach the application
//! only through [`AccessibilityAdapter::drain_actions`] on its own loop, after the window system's waker woke it.
//! While the session is locked no node it sends is a live region, so nothing is announced.

mod convert;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::Waker;

use accesskit::{ActionHandler, ActivationHandler, DeactivationHandler, TreeInfo, TreeUpdate};
use cupertino_ui_core::{AccessibilityAdapter, AccessibilityUpdate, Action, ActionRequest, NodeId};
use cupertino_ui_platform::WindowId;

/// The AT-SPI adapter of an application: one AccessKit adapter per window, created when core first names the window.
pub struct AtspiAdapter {
    shared: Arc<Shared>,
    windows: HashMap<WindowId, Window>,
    muted: bool,
    /// The application draws the lock screen itself: the session's lock never mutes it.
    lock_screen: bool,
}

/// The most action requests waiting for the application's loop; more are dropped until it drains them, and an
/// assistive technology asks again.
const MAX_QUEUED: usize = 64;

/// What the executor thread and the application's loop share.
struct Shared {
    /// Windows whose AccessKit adapter is active.
    active: AtomicUsize,
    /// Counts the times the last window deactivated. Core stops sending then, so a tree from an earlier epoch may be
    /// stale; core sends every window whole once one is active again.
    epoch: AtomicU64,
    actions: Mutex<Vec<ActionRequest>>,
    waker: Waker,
}

struct Window {
    adapter: accesskit_unix::Adapter,
    mirror: Arc<Mutex<Mirror>>,
}

#[derive(Default)]
struct Titled {
    /// The window's title, which names its root when the root has no name of its own.
    title: Option<String>,
    /// The root node took its name from the title.
    root_untitled: bool,
}

/// The tree a window last sent, kept so a window that activates after core sent it can be given it whole without
/// asking core from the executor thread.
#[derive(Default)]
struct Mirror {
    root: Option<NodeId>,
    focus: Option<NodeId>,
    nodes: HashMap<NodeId, Published>,
    titled: Titled,
    /// The epoch of the update that last changed the tree.
    epoch: u64,
    /// The window's AccessKit adapter is active.
    active: bool,
    /// AccessKit holds the whole tree, so updates may send only what changed.
    initialized: bool,
    /// The id of the last text run given out; ids below 2^32 are never a node's.
    last_run: u64,
}

struct Published {
    node: accesskit::Node,
    actions: Vec<Action>,
    protected: bool,
    runs: Vec<convert::Run>,
}

impl Published {
    /// The node and its text runs, as AccessKit takes them.
    fn sent(&self, id: NodeId) -> impl Iterator<Item = (accesskit::NodeId, accesskit::Node)> + '_ {
        std::iter::once((convert::node_id(id), self.node.clone()))
            .chain(self.runs.iter().map(|r| (r.id, r.node.clone())))
    }
}

impl Mirror {
    fn full(&self, root: NodeId) -> TreeUpdate {
        let mut tree = TreeInfo::new(convert::node_id(root));
        tree.toolkit_name = Some("CupertinoKit".into());
        tree.toolkit_version = Some(env!("CARGO_PKG_VERSION").into());
        TreeUpdate {
            nodes: self.nodes.iter().flat_map(|(id, p)| p.sent(*id)).collect(),
            tree: Some(tree),
            tree_id: accesskit::TreeId::ROOT,
            focus: convert::node_id(self.focus.unwrap_or(root)),
        }
    }

    /// Whether assistive technology may ask `request` of its target: the target advertised the action in its last
    /// update, and a secret never takes a value or a selection.
    fn allows(&self, request: &ActionRequest) -> bool {
        self.nodes.get(&request.target).is_some_and(|p| {
            p.actions.contains(&request.action)
                && !(p.protected
                    && matches!(request.action, Action::SetValue | Action::SetSelection))
        })
    }
}

impl AtspiAdapter {
    /// An adapter that wakes the application's loop with `waker` (the window system's) when assistive technology
    /// activates or leaves it, and when it asks for an action.
    pub fn new(waker: Waker) -> AtspiAdapter {
        AtspiAdapter {
            shared: Arc::new(Shared {
                active: AtomicUsize::new(0),
                epoch: AtomicU64::new(0),
                actions: Mutex::new(Vec::new()),
                waker,
            }),
            windows: HashMap::new(),
            muted: false,
            lock_screen: false,
        }
    }

    /// Like [`Self::new`], for the application that draws the lock screen: it is what a person hears while the session is
    /// locked, so [`AccessibilityAdapter::session_locked`] never mutes it and the answer to an attempt is announced.
    pub fn lock_screen(waker: Waker) -> AtspiAdapter {
        AtspiAdapter {
            lock_screen: true,
            ..AtspiAdapter::new(waker)
        }
    }

    /// Like [`Self::new`], but muted until the first [`AccessibilityAdapter::session_locked`]: an application started
    /// while the session is locked announces nothing before it learns so.
    pub fn muted(waker: Waker) -> AtspiAdapter {
        AtspiAdapter {
            muted: true,
            ..AtspiAdapter::new(waker)
        }
    }

    fn window(&mut self, id: WindowId) -> &mut Window {
        let shared = &self.shared;
        self.windows.entry(id).or_insert_with(|| {
            let mirror = Arc::new(Mutex::new(Mirror::default()));
            let handlers = Handlers {
                shared: Arc::clone(shared),
                mirror: Arc::clone(&mirror),
            };
            Window {
                adapter: accesskit_unix::Adapter::new(handlers.clone(), handlers.clone(), handlers),
                mirror,
            }
        })
    }
}

impl AccessibilityAdapter for AtspiAdapter {
    fn is_active(&self) -> bool {
        self.shared.active.load(Ordering::Acquire) > 0
    }

    fn window_focus(&mut self, window: WindowId, focused: bool) {
        self.window(window)
            .adapter
            .update_window_focus_state(focused);
    }

    fn update(&mut self, update: &AccessibilityUpdate) {
        let epoch = self.shared.epoch.load(Ordering::Acquire);
        let muted = self.muted;
        let window = self.window(update.window);
        let delta = {
            let mut m = lock(&window.mirror);
            m.epoch = epoch;
            if update.full {
                m.nodes.clear();
            }
            for id in &update.removed {
                m.nodes.remove(id);
            }
            for (id, node) in &update.nodes {
                let mut converted = convert::node(node);
                if *id == update.root {
                    m.titled.root_untitled = node.name.is_none();
                    if let Some(title) = m.titled.title.as_deref().filter(|_| node.name.is_none()) {
                        converted.set_label(title);
                    }
                }
                if muted {
                    converted.clear_live();
                }
                // a node keeps the ids of its runs, so AccessKit sees its text change rather than replaced
                let kept: Vec<accesskit::NodeId> = m
                    .nodes
                    .get(id)
                    .map(|p| p.runs.iter().map(|r| r.id).collect())
                    .unwrap_or_default();
                let last_run = &mut m.last_run;
                let runs = convert::text_runs(node, &mut converted, |i| {
                    kept.get(i).copied().unwrap_or_else(|| {
                        *last_run += 1;
                        accesskit::NodeId(*last_run)
                    })
                });
                let published = Published {
                    node: converted,
                    actions: node.actions.clone(),
                    protected: convert::protected(node),
                    runs,
                };
                m.nodes.insert(*id, published);
            }
            m.root = Some(update.root);
            m.focus = update.focus;
            TreeUpdate {
                nodes: update
                    .nodes
                    .iter()
                    .flat_map(|(id, _)| m.nodes[id].sent(*id))
                    .collect(),
                tree: None,
                tree_id: accesskit::TreeId::ROOT,
                focus: convert::node_id(update.focus.unwrap_or(update.root)),
            }
        };
        let mirror = &window.mirror;
        window.adapter.update_if_active(|| {
            let mut m = lock(mirror);
            if m.initialized {
                delta
            } else {
                m.initialized = true;
                m.full(update.root)
            }
        });
    }

    /// AccessKit announces a live region only as it is added or renamed, from the node it is sent then, so a node sent
    /// while muted announces nothing and the next one sent after is announced as usual.
    fn session_locked(&mut self, locked: bool) {
        self.muted = locked && !self.lock_screen;
    }

    fn window_title(&mut self, window: WindowId, title: &str) {
        let window = self.window(window);
        let delta = {
            let mut m = lock(&window.mirror);
            m.titled.title = Some(title.to_owned()).filter(|t| !t.is_empty());
            let (Some(root), true) = (m.root, m.titled.root_untitled) else {
                return;
            };
            let title = m.titled.title.clone();
            let Some(published) = m.nodes.get_mut(&root) else {
                return;
            };
            match title {
                Some(title) => published.node.set_label(title),
                None => published.node.clear_label(),
            }
            TreeUpdate {
                nodes: published.sent(root).collect(),
                tree: None,
                tree_id: accesskit::TreeId::ROOT,
                focus: convert::node_id(m.focus.unwrap_or(root)),
            }
        };
        let mirror = &window.mirror;
        window.adapter.update_if_active(|| {
            let mut m = lock(mirror);
            match m.root.filter(|_| !m.initialized) {
                Some(root) => {
                    m.initialized = true;
                    m.full(root)
                }
                None => delta,
            }
        });
    }

    fn window_closed(&mut self, window: WindowId) {
        self.windows.remove(&window);
    }

    fn drain_actions(&mut self, out: &mut Vec<ActionRequest>) {
        out.append(&mut lock(&self.shared.actions));
    }
}

/// A window's AccessKit handlers. They run on AccessKit's executor thread and touch only what the adapter shares
/// with them and the window's mirror, never the application.
#[derive(Clone)]
struct Handlers {
    shared: Arc<Shared>,
    mirror: Arc<Mutex<Mirror>>,
}

impl ActivationHandler for Handlers {
    fn request_initial_tree(&mut self) -> Option<TreeUpdate> {
        let mut m = lock(&self.mirror);
        if !m.active {
            m.active = true;
            self.shared.active.fetch_add(1, Ordering::AcqRel);
        }
        self.shared.waker.wake_by_ref();
        let root = m
            .root
            .filter(|_| m.epoch == self.shared.epoch.load(Ordering::Acquire))?;
        m.initialized = true;
        Some(m.full(root))
    }
}

impl DeactivationHandler for Handlers {
    fn deactivate_accessibility(&mut self) {
        let mut m = lock(&self.mirror);
        m.initialized = false;
        if m.active {
            m.active = false;
            if self.shared.active.fetch_sub(1, Ordering::AcqRel) == 1 {
                self.shared.epoch.fetch_add(1, Ordering::AcqRel);
            }
        }
        self.shared.waker.wake_by_ref();
    }
}

impl ActionHandler for Handlers {
    fn do_action(&mut self, request: accesskit::ActionRequest) {
        let m = lock(&self.mirror);
        let Some(request) =
            convert::request(&request, |id| m.nodes.get(&id).map(|p| p.runs.as_slice()))
        else {
            return;
        };
        if !m.allows(&request) {
            return;
        }
        drop(m);
        let mut queued = lock(&self.shared.actions);
        if queued.len() < MAX_QUEUED {
            queued.push(request);
        }
        drop(queued);
        self.shared.waker.wake_by_ref();
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests;
