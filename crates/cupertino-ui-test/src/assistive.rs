use cupertino_ui_core::{
    AccessibilityAdapter, AccessibilityNode, AccessibilityUpdate, Action,
    ActionData, ActionRequest, Live, NodeId,
};
use cupertino_ui_platform::WindowId;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, MutexGuard};

/// What a live region said: its window, how urgently, and the text a screen reader reads of it.
#[derive(Clone, Debug, PartialEq)]
pub struct Announcement {
    pub window: WindowId,
    pub live: Live,
    pub text: String,
}

/// A stand-in for a screen reader that is always listening: the adapter a test hands the runtime
/// ([`adapter`](Self::adapter)), the tree it was sent for each window, put together from whole and partial updates as
/// a real adapter does, what live regions said, and the actions the test asks for on the reader's behalf
/// ([`ask`](Self::ask)). Clones share one reader.
#[derive(Clone, Default)]
pub struct AssistiveTechnology {
    state: Arc<Mutex<State>>,
}

#[derive(Default)]
struct State {
    trees: BTreeMap<WindowId, Held>,
    asked: Vec<ActionRequest>,
    said: Vec<Announcement>,
    focused_window: Option<WindowId>,
    locked: bool,
    titles: BTreeMap<WindowId, String>,
}

struct Held {
    root: NodeId,
    focus: Option<NodeId>,
    nodes: HashMap<NodeId, AccessibilityNode>,
    /// How many times the focus moved since the window opened.
    focus_moves: usize,
}

impl AssistiveTechnology {
    pub fn new() -> Self {
        Self::default()
    }

    /// The adapter to put in `RuntimeOptions::accessibility`.
    pub fn adapter(&self) -> Box<dyn AccessibilityAdapter> {
        Box::new(self.clone())
    }

    /// Queues an action on `target` as a screen reader would ask for it; the runtime takes it on its next turn, once
    /// something wakes it ([`TestDriver::ask`](crate::TestDriver::ask) asks and wakes it).
    pub fn ask(&self, target: NodeId, action: Action, data: Option<ActionData>) {
        self.lock().asked.push(ActionRequest {
            target,
            action,
            data,
        });
    }

    /// The whole tree the reader holds for `window`, as a full update, or `None` before the window's first update or
    /// after it closed. A root without a name of its own is named by the window's title, as the AT-SPI bridge names it.
    pub fn tree(&self, window: WindowId) -> Option<AccessibilityUpdate> {
        let state = self.lock();
        let held = state.trees.get(&window)?;
        let mut nodes: Vec<(NodeId, AccessibilityNode)> =
            held.nodes.iter().map(|(id, n)| (*id, n.clone())).collect();
        if let Some((_, root)) = nodes.iter_mut().find(|(id, _)| *id == held.root) {
            if root.name.is_none() {
                root.name = state.titles.get(&window).cloned();
            }
        }
        nodes.sort_by_key(|(id, _)| *id);
        Some(AccessibilityUpdate {
            window,
            root: held.root,
            focus: held.focus,
            nodes,
            removed: Vec::new(),
            full: true,
        })
    }

    /// The node the reader was last told has the focus in `window`.
    pub fn focus(&self, window: WindowId) -> Option<NodeId> {
        self.lock().trees.get(&window)?.focus
    }

    /// How many times the focus moved in `window` since its tree arrived: an announcement that moved it would show
    /// here.
    pub fn focus_moves(&self, window: WindowId) -> usize {
        self.lock().trees.get(&window).map_or(0, |h| h.focus_moves)
    }

    /// The window the reader was last told has the keyboard.
    pub fn focused_window(&self) -> Option<WindowId> {
        self.lock().focused_window
    }

    /// Whether the session is locked, as the runtime last said.
    pub fn locked(&self) -> bool {
        self.lock().locked
    }

    /// Everything live regions said, oldest first: each live node's name when the node reaches the tree with one and
    /// whenever its name changes, as the AT-SPI bridge announces them. A change of value or description alone is not
    /// said.
    pub fn announcements(&self) -> Vec<Announcement> {
        self.lock().said.clone()
    }

    /// The announcements since the last call, oldest first.
    pub fn take_announcements(&self) -> Vec<Announcement> {
        std::mem::take(&mut self.lock().said)
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// What the AT-SPI bridge announces of a live node: its name, which AccessKit sends as the announcement when the node
/// is added or renamed.
fn spoken(node: &AccessibilityNode) -> &str {
    node.name.as_deref().unwrap_or("").trim()
}

impl AccessibilityAdapter for AssistiveTechnology {
    fn is_active(&self) -> bool {
        true
    }

    fn window_focus(&mut self, window: WindowId, focused: bool) {
        let mut state = self.lock();
        if focused {
            state.focused_window = Some(window);
        } else if state.focused_window == Some(window) {
            state.focused_window = None;
        }
    }

    fn update(&mut self, update: &AccessibilityUpdate) {
        let mut state = self.lock();
        let held = state.trees.entry(update.window).or_insert_with(|| Held {
            root: update.root,
            focus: update.focus,
            nodes: HashMap::new(),
            focus_moves: 0,
        });
        // a whole tree replaces the held one, but a node it sends again is the same node, renamed or not
        let mut before = if update.full {
            std::mem::take(&mut held.nodes)
        } else {
            HashMap::new()
        };
        for id in &update.removed {
            held.nodes.remove(id);
        }
        let mut said = Vec::new();
        for (id, node) in &update.nodes {
            let old = held.nodes.remove(id).or_else(|| before.remove(id));
            if let Some(live) = node.live {
                let text = spoken(node);
                if !text.is_empty() && old.as_ref().is_none_or(|old| spoken(old) != text) {
                    said.push(Announcement {
                        window: update.window,
                        live,
                        text: text.to_owned(),
                    });
                }
            }
            held.nodes.insert(*id, node.clone());
        }
        held.root = update.root;
        if held.focus != update.focus {
            held.focus_moves += 1;
            held.focus = update.focus;
        }
        state.said.append(&mut said);
    }

    fn drain_actions(&mut self, out: &mut Vec<ActionRequest>) {
        out.append(&mut self.lock().asked);
    }

    fn window_closed(&mut self, window: WindowId) {
        let mut state = self.lock();
        state.trees.remove(&window);
        state.titles.remove(&window);
        if state.focused_window == Some(window) {
            state.focused_window = None;
        }
    }

    fn session_locked(&mut self, locked: bool) {
        self.lock().locked = locked;
    }

    fn window_title(&mut self, window: WindowId, title: &str) {
        self.lock().titles.insert(window, title.to_owned());
    }
}

#[cfg(test)]
#[path = "assistive-tests.rs"]
mod tests;
