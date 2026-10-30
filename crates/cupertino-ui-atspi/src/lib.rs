//! Publishes a CupertinoKit application's accessibility tree to assistive technology over AT-SPI.
//!
//! [`AtspiAdapter`] is core's [`AccessibilityAdapter`] on AccessKit's Unix adapter. Each window is a top-level AT-SPI
//! object. Nothing is exported until an assistive technology is on the accessibility bus; core builds no tree until
//! then. AccessKit runs on an executor thread of its own; requests from assistive technology reach the application
//! only through [`AccessibilityAdapter::drain_actions`] on its own loop, after the window system's waker woke it.

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

/// The tree a window last sent, kept so a window that activates after core sent it can be given it whole without
/// asking core from the executor thread.
#[derive(Default)]
struct Mirror {
    root: Option<NodeId>,
    focus: Option<NodeId>,
    nodes: HashMap<NodeId, Published>,
    /// The epoch of the update that last changed the tree.
    epoch: u64,
    /// The window's AccessKit adapter is active.
    active: bool,
    /// AccessKit holds the whole tree, so updates may send only what changed.
    initialized: bool,
}

struct Published {
    node: accesskit::Node,
    actions: Vec<Action>,
    protected: bool,
}

impl Mirror {
    fn full(&self, root: NodeId) -> TreeUpdate {
        let mut tree = TreeInfo::new(convert::node_id(root));
        tree.toolkit_name = Some("CupertinoKit".into());
        tree.toolkit_version = Some(env!("CARGO_PKG_VERSION").into());
        TreeUpdate {
            nodes: self
                .nodes
                .iter()
                .map(|(id, p)| (convert::node_id(*id), p.node.clone()))
                .collect(),
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
                let published = Published {
                    node: convert::node(node),
                    actions: node.actions.clone(),
                    protected: convert::protected(node),
                };
                m.nodes.insert(*id, published);
            }
            m.root = Some(update.root);
            m.focus = update.focus;
            TreeUpdate {
                nodes: update
                    .nodes
                    .iter()
                    .map(|(id, _)| (convert::node_id(*id), m.nodes[id].node.clone()))
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
        let Some(request) = convert::request(&request) else {
            return;
        };
        if !lock(&self.mirror).allows(&request) {
            return;
        }
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
