use cupertino_ui_core::{
    AccessibilityAdapter, AccessibilityNode, AccessibilityUpdate, AccessibleValue, Action,
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
}

struct Held {
    root: NodeId,
    focus: Option<NodeId>,
    nodes: HashMap<NodeId, AccessibilityNode>,
    /// Each live node's last text, so only a change is said again.
    live: HashMap<NodeId, String>,
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

    /// Queues an action on `target` as a screen reader would ask for it; the runtime takes it on its next turn.
    pub fn ask(&self, target: NodeId, action: Action, data: Option<ActionData>) {
        self.lock().asked.push(ActionRequest {
            target,
            action,
            data,
        });
    }

    /// The whole tree the reader holds for `window`, as a full update, or `None` before the window's first update or
    /// after it closed.
    pub fn tree(&self, window: WindowId) -> Option<AccessibilityUpdate> {
        let state = self.lock();
        let held = state.trees.get(&window)?;
        let mut nodes: Vec<(NodeId, AccessibilityNode)> =
            held.nodes.iter().map(|(id, n)| (*id, n.clone())).collect();
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

    /// Everything live regions said, oldest first: each live node when it reaches the tree with something to say and
    /// whenever what it says changes.
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

/// What a screen reader reads of a live node: its name, its description and its text or number value, in that order.
fn spoken(node: &AccessibilityNode) -> String {
    let value = match &node.value {
        Some(AccessibleValue::Text { value, .. }) => Some(value.clone()),
        Some(AccessibleValue::Number { value, .. }) => Some(value.to_string()),
        _ => None,
    };
    [node.name.clone(), node.description.clone(), value]
        .into_iter()
        .flatten()
        .filter(|s| !s.trim().is_empty())
        .collect::<Vec<_>>()
        .join(", ")
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
            live: HashMap::new(),
            focus_moves: 0,
        });
        if update.full {
            held.nodes.clear();
        }
        for id in &update.removed {
            held.nodes.remove(id);
            held.live.remove(id);
        }
        let mut said = Vec::new();
        for (id, node) in &update.nodes {
            held.nodes.insert(*id, node.clone());
            let Some(live) = node.live else {
                held.live.remove(id);
                continue;
            };
            let text = spoken(node);
            if held.live.get(id) != Some(&text) {
                held.live.insert(*id, text.clone());
                if !text.is_empty() {
                    said.push(Announcement {
                        window: update.window,
                        live,
                        text,
                    });
                }
            }
        }
        held.live.retain(|id, _| held.nodes.contains_key(id));
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
        if state.focused_window == Some(window) {
            state.focused_window = None;
        }
    }

    fn session_locked(&mut self, locked: bool) {
        self.lock().locked = locked;
    }
}

#[cfg(test)]
#[path = "assistive-tests.rs"]
mod tests;
