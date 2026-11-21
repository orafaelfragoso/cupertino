//! The window control buttons the server draws (close, minimize, zoom), published to assistive technology over AT-SPI.
//!
//! The client of a window publishes its own content; the buttons on the title bar are the server's, so the server
//! publishes them: each window with server-drawn buttons is a top-level object of the server's own, named for the
//! window's controls ("Notes window controls", [`controls_name`]) so it is not taken for the client's own window of the
//! same title, holding three push buttons with their names, their bounds and whether they are enabled. An enabled
//! button offers its press; a disabled one is reported disabled and offers nothing. The buttons have no long press nor
//! menu for the pointer either, so the press is the only action. A press asked by an assistive technology comes back
//! to the event loop and does what a click does: only on a button published enabled, of a window published now, while
//! the session is unlocked.
//!
//! A full-screen window's buttons are published while its title bar shows, revealed with the menu bar over it, and
//! withdrawn when it hides again.
//!
//! [`ButtonAccessibility`] keeps what is published and sends only what changed to its [`ButtonSink`]; the sink of a
//! session is [`AtspiSink`], one AccessKit Unix adapter per window. The buttons are computed again only when something
//! may have changed them (a model command, a redraw, the lock, or an animation running), not at every pass of the loop.
//! Nothing is sent while the session is locked: the windows are hidden then, and so are their buttons.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use crate::decorations::ButtonKind;
use accesskit::{
    Action, ActionHandler, ActionRequest, ActivationHandler, DeactivationHandler, Node, NodeId,
    Role, TreeInfo, TreeUpdate,
};
use cupertino_geometry::Rect;
use cupertino_wm::WindowId;

/// The name an assistive technology speaks for a window control button.
pub fn button_name(kind: ButtonKind) -> &'static str {
    match kind {
        ButtonKind::Close => "Close",
        ButtonKind::Minimize => "Minimize",
        ButtonKind::Zoom => "Zoom",
    }
}

/// The name of a window's object: its controls, named for the window, so the client's own window of the same title is
/// told apart from it.
pub fn controls_name(title: &str) -> String {
    if title.trim().is_empty() {
        "Window controls".to_owned()
    } else {
        format!("{title} window controls")
    }
}

/// One window control button as published.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ButtonAccess {
    pub kind: ButtonKind,
    /// The button takes a press; a disabled one stays in place, dimmed.
    pub enabled: bool,
    /// Its hit area, in points from the top-left corner of the window's frame.
    pub bounds: Rect,
}

/// The buttons of one window, as published.
#[derive(Clone, Debug, PartialEq)]
pub struct WindowButtons {
    /// The window's title, which names its object ([`controls_name`]).
    pub title: String,
    /// Where the buttons' bounds are measured from, in global logical points: the window's frame (title bar included),
    /// or the title bar revealed over a full-screen window.
    pub frame: Rect,
    /// Close, minimize and zoom, in that order.
    pub buttons: [ButtonAccess; 3],
}

/// The id of a window's own node.
const ROOT: NodeId = NodeId(1);

/// The id of a button's node.
fn button_node(kind: ButtonKind) -> NodeId {
    NodeId(2 + kind.index() as u64)
}

/// The button whose node is `id`.
fn button_of(id: NodeId) -> Option<ButtonKind> {
    ButtonKind::ALL.into_iter().find(|k| button_node(*k) == id)
}

fn rect(r: Rect) -> accesskit::Rect {
    accesskit::Rect::new(r.min_x(), r.min_y(), r.max_x(), r.max_y())
}

impl WindowButtons {
    /// The whole tree of the window: its own node with the three buttons in reading order.
    pub fn tree(&self, toolkit_name: &str) -> TreeUpdate {
        let mut root = Node::new(Role::Window);
        root.set_label(controls_name(&self.title));
        root.set_bounds(accesskit::Rect::new(
            0.0,
            0.0,
            self.frame.width(),
            self.frame.height(),
        ));
        root.set_children(
            self.buttons
                .iter()
                .map(|b| button_node(b.kind))
                .collect::<Vec<_>>(),
        );
        let mut nodes = vec![(ROOT, root)];
        for b in &self.buttons {
            let mut node = Node::new(Role::Button);
            node.set_label(button_name(b.kind));
            node.set_bounds(rect(b.bounds));
            if b.enabled {
                node.add_action(Action::Click);
            } else {
                node.set_disabled();
            }
            nodes.push((button_node(b.kind), node));
        }
        let mut tree = TreeInfo::new(ROOT);
        tree.toolkit_name = Some(toolkit_name.into());
        tree.toolkit_version = Some(env!("CARGO_PKG_VERSION").into());
        TreeUpdate {
            nodes,
            tree: Some(tree),
            tree_id: accesskit::TreeId::ROOT,
            focus: ROOT,
        }
    }

    /// The enabled button an assistive technology asked to press with `request`; `None` for anything else.
    pub fn pressed_by(&self, request: &ActionRequest) -> Option<ButtonKind> {
        if request.action != Action::Click {
            return None;
        }
        let kind = button_of(request.target_node)?;
        self.buttons[kind.index()].enabled.then_some(kind)
    }
}

/// Where the published buttons go.
pub trait ButtonSink {
    /// Publish the window's buttons whole, the first time or again after a change.
    fn publish(&mut self, window: WindowId, buttons: &WindowButtons);
    /// The window has no buttons of the server's any more (closed, minimized, full screen, client-decorated, or the
    /// session locked).
    fn withdraw(&mut self, window: WindowId);
}

/// The published buttons of every window, and where they go. Without a sink nothing is computed nor sent: offscreen and
/// nested runs, and the login screen.
#[derive(Default)]
pub struct ButtonAccessibility {
    sink: Option<Box<dyn ButtonSink>>,
    published: HashMap<WindowId, WindowButtons>,
    /// Something may have changed the buttons since they were last computed ([`Self::invalidate`]).
    stale: bool,
    /// The loop was animating at the last [`Self::due`]: one more computation follows the end of an animation.
    animating: bool,
    /// The session was locked at the last [`Self::due`].
    locked: bool,
    /// How many times the buttons were computed (tests).
    computed: u64,
}

impl ButtonAccessibility {
    /// Whether the buttons are published at all.
    pub fn enabled(&self) -> bool {
        self.sink.is_some()
    }

    /// Publish from now on to `sink`, which starts empty.
    pub fn set_sink(&mut self, sink: Box<dyn ButtonSink>) {
        self.published.clear();
        self.sink = Some(sink);
        self.stale = true;
    }

    /// Something that may change the buttons happened (a model command, a redraw): compute them at the next sync.
    pub fn invalidate(&mut self) {
        self.stale = true;
    }

    /// Whether the buttons must be computed again: they are published, and something changed since the last sync, the
    /// lock changed, or an animation runs now or ran at the last check (a state an animation ends in, such as leaving
    /// full screen, is caught the pass after).
    pub fn due(&mut self, animating: bool, locked: bool) -> bool {
        let due = self.sink.is_some()
            && (self.stale || animating || self.animating || locked != self.locked);
        self.animating = animating;
        self.locked = locked;
        due
    }

    /// How many times the buttons were computed since the server started.
    pub fn computed(&self) -> u64 {
        self.computed
    }

    /// What is published now.
    pub fn published(&self) -> &HashMap<WindowId, WindowButtons> {
        &self.published
    }

    /// Make the published buttons `now`: windows that are gone are withdrawn, new or changed ones published again.
    pub fn sync(&mut self, now: HashMap<WindowId, WindowButtons>) {
        let Some(sink) = self.sink.as_mut() else {
            return;
        };
        self.stale = false;
        self.computed += 1;
        self.published.retain(|id, _| {
            let keep = now.contains_key(id);
            if !keep {
                sink.withdraw(*id);
            }
            keep
        });
        for (id, buttons) in now {
            if self.published.get(&id) != Some(&buttons) {
                sink.publish(id, &buttons);
                self.published.insert(id, buttons);
            }
        }
    }
}

/// Hands a press an assistive technology asked for to the event loop. Called from AccessKit's thread.
pub type PressRequests = Arc<dyn Fn(WindowId, ButtonKind) + Send + Sync>;

/// AccessKit's handlers for one window, on its executor thread: they read the buttons from the copy the loop keeps
/// current, and never call into the loop but through [`PressRequests`].
struct Handlers {
    window: WindowId,
    toolkit_name: &'static str,
    current: Arc<Mutex<WindowButtons>>,
    presses: PressRequests,
}

impl ActivationHandler for Handlers {
    fn request_initial_tree(&mut self) -> Option<TreeUpdate> {
        Some(
            self.current
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .tree(self.toolkit_name),
        )
    }
}

impl ActionHandler for Handlers {
    fn do_action(&mut self, request: ActionRequest) {
        // only a press of a button that is enabled in what was last published; the loop checks again before it acts
        let pressed = self
            .current
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pressed_by(&request);
        if let Some(kind) = pressed {
            (self.presses)(self.window, kind);
        }
    }
}

impl DeactivationHandler for Handlers {
    fn deactivate_accessibility(&mut self) {}
}

struct AtspiWindow {
    adapter: accesskit_unix::Adapter,
    current: Arc<Mutex<WindowButtons>>,
}

/// The session's sink: one AccessKit Unix adapter per window, a top-level object of the server's application on the
/// accessibility bus. AccessKit exports nothing until an assistive technology turns accessibility on.
pub struct AtspiSink {
    toolkit_name: &'static str,
    windows: HashMap<WindowId, AtspiWindow>,
    presses: PressRequests,
}

impl AtspiSink {
    pub fn new(toolkit_name: &'static str, presses: PressRequests) -> Self {
        Self {
            toolkit_name,
            windows: HashMap::new(),
            presses,
        }
    }
}

impl ButtonSink for AtspiSink {
    fn publish(&mut self, window: WindowId, buttons: &WindowButtons) {
        let toolkit_name = self.toolkit_name;
        let entry = self.windows.entry(window).or_insert_with(|| {
            let current = Arc::new(Mutex::new(buttons.clone()));
            let handlers = || Handlers {
                window,
                toolkit_name,
                current: Arc::clone(&current),
                presses: Arc::clone(&self.presses),
            };
            AtspiWindow {
                adapter: accesskit_unix::Adapter::new(handlers(), handlers(), handlers()),
                current,
            }
        });
        *entry.current.lock().unwrap_or_else(PoisonError::into_inner) = buttons.clone();
        // the server knows where its windows are, which a client on Wayland does not
        entry
            .adapter
            .set_root_window_bounds(rect(buttons.frame), rect(buttons.frame));
        entry
            .adapter
            .update_if_active(|| buttons.tree(toolkit_name));
    }

    fn withdraw(&mut self, window: WindowId) {
        // dropping the adapter removes the window's object from the bus
        self.windows.remove(&window);
    }
}
