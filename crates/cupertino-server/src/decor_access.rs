//! The window control buttons the server draws (close, minimize, zoom), published to assistive technology over AT-SPI.
//!
//! The client of a window publishes its own content; the buttons on the title bar are the server's, so the server
//! publishes them: each window with server-drawn buttons is a top-level object of the server's own, named for the
//! window's controls ("Notes window controls", [`controls_name`]) so it is not taken for the client's own window of the
//! same title, holding three push buttons with their names, their bounds and whether they are enabled. An enabled
//! button offers its press; a disabled one is reported disabled and offers nothing. The buttons have no long press nor
//! menu for the pointer either, so the press is the only action. A press asked by an assistive technology comes back
//! to the event loop and does what a click does ([`Cupertino::press_published_button`]): only on a button published
//! enabled, of a window published now, while the session is unlocked.
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

use accesskit::{
    Action, ActionHandler, ActionRequest, ActivationHandler, DeactivationHandler, Node, NodeId,
    Role, TreeInfo, TreeUpdate,
};
use cupertino_decorations::ButtonKind;
use cupertino_geometry::{Rect, Size};
use cupertino_wm::WindowId;
use smithay::reexports::calloop::channel;

use crate::decor::button_enabled;
use crate::state::Cupertino;

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
    pub fn tree(&self) -> TreeUpdate {
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
        tree.toolkit_name = Some("Cupertino".into());
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
    current: Arc<Mutex<WindowButtons>>,
    presses: PressRequests,
}

impl ActivationHandler for Handlers {
    fn request_initial_tree(&mut self) -> Option<TreeUpdate> {
        Some(
            self.current
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .tree(),
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
    windows: HashMap<WindowId, AtspiWindow>,
    presses: PressRequests,
}

impl AtspiSink {
    pub fn new(presses: PressRequests) -> Self {
        Self {
            windows: HashMap::new(),
            presses,
        }
    }
}

impl ButtonSink for AtspiSink {
    fn publish(&mut self, window: WindowId, buttons: &WindowButtons) {
        let entry = self.windows.entry(window).or_insert_with(|| {
            let current = Arc::new(Mutex::new(buttons.clone()));
            let handlers = || Handlers {
                window,
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
        entry.adapter.update_if_active(|| buttons.tree());
    }

    fn withdraw(&mut self, window: WindowId) {
        // dropping the adapter removes the window's object from the bus
        self.windows.remove(&window);
    }
}

impl Cupertino {
    /// Publish the window control buttons on the accessibility bus from now on, and take the presses an assistive
    /// technology asks for on the loop: a person's session on the hardware.
    pub fn start_button_access(&mut self) {
        let (presses, received) = channel::channel::<(WindowId, ButtonKind)>();
        let inserted = self.loop_handle.insert_source(received, |event, _, state| {
            if let channel::Event::Msg((window, kind)) = event {
                state.press_published_button(window, kind);
            }
        });
        if let Err(e) = inserted {
            tracing::warn!(error = %e, "the window buttons are not published to assistive technology");
            return;
        }
        let presses = Mutex::new(presses);
        self.button_access
            .set_sink(Box::new(AtspiSink::new(Arc::new(move |window, kind| {
                // the loop is gone only while the server shuts down
                let _ = presses
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .send((window, kind));
            }))));
    }

    /// Press button `kind` of `window` as an assistive technology asked: only a button published enabled, of a window
    /// whose buttons are published now, while the session is unlocked; then it does what a click does
    /// ([`Cupertino::press_window_button`], which checks the button again).
    pub fn press_published_button(&mut self, window: WindowId, kind: ButtonKind) {
        let published = self
            .button_access
            .published()
            .get(&window)
            .is_some_and(|b| b.buttons[kind.index()].enabled);
        if published && !self.session_lock.is_active() {
            self.press_window_button(window, kind);
        }
    }

    /// The buttons of every window whose buttons the server draws and shows, while the session is unlocked: a mapped
    /// window with server-side decorations, neither full screen nor minimized (nor on its way there or out); and a
    /// full-screen window while its title bar shows, revealed with the menu bar.
    pub fn window_buttons(&self) -> HashMap<WindowId, WindowButtons> {
        use cupertino_wm::LifecycleState::*;
        if self.session_lock.is_active() || !self.config.window.decorations {
            return HashMap::new();
        }
        let now = self.now();
        self.windows
            .iter()
            .filter(|e| e.mapped)
            .filter_map(|e| {
                let w = self.wm.window(e.id)?;
                if e.fullscreen {
                    return self.revealed_buttons(e, w, now);
                }
                if !e.server_side || !(e.insets().top > 0.0 || e.titlebar.client_drawn()) {
                    return None;
                }
                if matches!(
                    w.state(),
                    Created
                        | Minimizing
                        | Minimized
                        | EnterFullScreen
                        | FullScreen
                        | ExitFullScreen
                        | Closing
                        | Destroyed
                ) {
                    return None;
                }
                let geo = e.window.geometry();
                let content = Size::new(f64::from(geo.size.w), f64::from(geo.size.h));
                let layout = self.decor.for_style(e.titlebar).layout(content);
                let enabled = button_enabled(w.can_minimize(), w.can_zoom());
                let frame = w.geometry();
                Some((
                    e.id,
                    WindowButtons {
                        title: w.title.clone(),
                        frame: Rect::new(frame.x, frame.y, frame.w, frame.h),
                        buttons: ButtonKind::ALL.map(|kind| ButtonAccess {
                            kind,
                            enabled: enabled[kind.index()],
                            bounds: layout.button(kind).art,
                        }),
                    },
                ))
            })
            .collect()
    }

    /// The buttons of full-screen window `e` while its title bar shows, revealed with the menu bar over it, as the scene
    /// draws them: close and zoom (which leaves full screen) enabled, minimize disabled.
    fn revealed_buttons(
        &self,
        e: &crate::state::WindowEntry,
        w: &cupertino_wm::Window,
        now: f64,
    ) -> Option<(WindowId, WindowButtons)> {
        use crate::state::WindowEntry;
        if w.state() != cupertino_wm::LifecycleState::FullScreen {
            return None;
        }
        let strip = e.revealed_titlebar(&self.menu_bars, &self.decor, true, w.output, now)?;
        let layout = self
            .decor
            .for_style(WindowEntry::REVEALED_TITLEBAR)
            .layout(Size::new(strip.w, strip.h));
        let enabled = button_enabled(false, true);
        Some((
            e.id,
            WindowButtons {
                title: w.title.clone(),
                frame: Rect::new(strip.x, strip.y, strip.w, strip.h),
                buttons: ButtonKind::ALL.map(|kind| ButtonAccess {
                    kind,
                    enabled: enabled[kind.index()],
                    bounds: layout.button(kind).art,
                }),
            },
        ))
    }

    /// Bring what assistive technology reads of the window buttons up to date; nothing when they are not published.
    pub(crate) fn sync_button_access(&mut self) {
        if !self.button_access.enabled() {
            return;
        }
        let animating = self.animating();
        if self
            .button_access
            .due(animating, self.session_lock.is_active())
        {
            let now = self.window_buttons();
            self.button_access.sync(now);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buttons(minimize: bool, zoom: bool) -> WindowButtons {
        let at = |x: f64| Rect::new(x, 9.0, 14.0, 14.0);
        WindowButtons {
            title: "Notes".into(),
            frame: Rect::new(100.0, 50.0, 400.0, 332.0),
            buttons: [
                ButtonAccess {
                    kind: ButtonKind::Close,
                    enabled: true,
                    bounds: at(9.0),
                },
                ButtonAccess {
                    kind: ButtonKind::Minimize,
                    enabled: minimize,
                    bounds: at(29.0),
                },
                ButtonAccess {
                    kind: ButtonKind::Zoom,
                    enabled: zoom,
                    bounds: at(49.0),
                },
            ],
        }
    }

    fn node(tree: &TreeUpdate, id: NodeId) -> &Node {
        &tree.nodes.iter().find(|(n, _)| *n == id).unwrap().1
    }

    #[test]
    fn a_window_publishes_three_named_push_buttons_and_reports_the_disabled_ones() {
        let tree = buttons(false, true).tree();
        let root = node(&tree, ROOT);
        assert_eq!(root.role(), Role::Window);
        assert_eq!(
            root.label(),
            Some("Notes window controls"),
            "named for the window, apart from the client's own window of that title"
        );
        assert_eq!(root.children(), &[NodeId(2), NodeId(3), NodeId(4)]);
        assert_eq!(tree.tree.as_ref().map(|t| t.root), Some(ROOT));
        let seen: Vec<_> = root
            .children()
            .iter()
            .map(|id| {
                let n = node(&tree, *id);
                assert_eq!(n.role(), Role::Button);
                // the buttons have no long press nor menu for the pointer, so a press is all they offer
                for other in [
                    Action::ShowContextMenu,
                    Action::Expand,
                    Action::Focus,
                    Action::Increment,
                ] {
                    assert!(!n.supports_action(other), "{other:?}");
                }
                (
                    n.label().unwrap().to_owned(),
                    n.is_disabled(),
                    n.supports_action(Action::Click),
                )
            })
            .collect();
        assert_eq!(
            seen,
            [
                ("Close".to_owned(), false, true),
                ("Minimize".to_owned(), true, false),
                ("Zoom".to_owned(), false, true),
            ]
        );
        let close = node(&tree, NodeId(2)).bounds().unwrap();
        assert_eq!(
            (close.x0, close.y0, close.x1, close.y1),
            (9.0, 9.0, 23.0, 23.0),
            "frame-local"
        );
    }

    #[test]
    fn an_untitled_window_still_names_its_controls() {
        assert_eq!(controls_name("Notes"), "Notes window controls");
        assert_eq!(controls_name(""), "Window controls");
        assert_eq!(controls_name("  "), "Window controls");
    }

    #[test]
    fn the_buttons_are_computed_again_only_after_a_change_the_lock_or_an_animation() {
        let mut access = ButtonAccessibility::default();
        assert!(!access.due(true, false), "nothing without a sink");
        access.set_sink(Box::new(Log::default()));
        assert!(access.due(false, false), "the first sync");
        access.sync(HashMap::new());
        assert!(!access.due(false, false), "idle");
        assert!(!access.due(false, false), "still idle");
        access.invalidate();
        assert!(access.due(false, false), "a change");
        access.sync(HashMap::new());
        assert!(!access.due(false, false));
        assert!(access.due(false, true), "the lock");
        assert!(!access.due(false, true), "locked still");
        assert!(access.due(false, false), "unlocked");
        assert!(access.due(true, false), "an animation");
        assert!(access.due(true, false), "still animating");
        assert!(access.due(false, false), "the pass after it ended");
        assert!(!access.due(false, false), "then idle");
    }

    #[test]
    fn only_a_press_of_an_enabled_button_is_taken() {
        let published = buttons(false, true);
        let ask = |target: NodeId, action: Action| ActionRequest {
            action,
            target_tree: accesskit::TreeId::ROOT,
            target_node: target,
            data: None,
        };
        assert_eq!(
            published.pressed_by(&ask(NodeId(2), Action::Click)),
            Some(ButtonKind::Close)
        );
        assert_eq!(
            published.pressed_by(&ask(NodeId(4), Action::Click)),
            Some(ButtonKind::Zoom)
        );
        assert_eq!(
            published.pressed_by(&ask(NodeId(3), Action::Click)),
            None,
            "disabled"
        );
        assert_eq!(published.pressed_by(&ask(ROOT, Action::Click)), None);
        assert_eq!(
            published.pressed_by(&ask(NodeId(2), Action::Focus)),
            None,
            "a press only"
        );
    }

    #[test]
    fn the_handlers_hand_a_press_to_the_loop_from_the_last_published_buttons() {
        let asked = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&asked);
        let current = Arc::new(Mutex::new(buttons(true, true)));
        let mut handlers = Handlers {
            window: WindowId(7),
            current: Arc::clone(&current),
            presses: Arc::new(move |w, k| sink.lock().unwrap().push((w, k))),
        };
        let press = |target| ActionRequest {
            action: Action::Click,
            target_tree: accesskit::TreeId::ROOT,
            target_node: target,
            data: None,
        };
        handlers.do_action(press(NodeId(3)));
        *current.lock().unwrap() = buttons(false, true);
        handlers.do_action(press(NodeId(3)));
        assert_eq!(
            *asked.lock().unwrap(),
            [(WindowId(7), ButtonKind::Minimize)],
            "the second press found minimize disabled"
        );
        let initial = handlers.request_initial_tree().unwrap();
        assert!(node(&initial, NodeId(3)).is_disabled());
    }

    #[derive(Default)]
    struct Log(Arc<Mutex<Vec<String>>>);

    impl ButtonSink for Log {
        fn publish(&mut self, window: WindowId, _: &WindowButtons) {
            self.0.lock().unwrap().push(format!("publish {}", window.0));
        }
        fn withdraw(&mut self, window: WindowId) {
            self.0
                .lock()
                .unwrap()
                .push(format!("withdraw {}", window.0));
        }
    }

    #[test]
    fn only_what_changed_is_sent() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut access = ButtonAccessibility::default();
        access.sync(HashMap::from([(WindowId(1), buttons(true, true))]));
        assert!(access.published().is_empty(), "nothing without a sink");
        access.set_sink(Box::new(Log(Arc::clone(&log))));
        access.sync(HashMap::from([(WindowId(1), buttons(true, true))]));
        access.sync(HashMap::from([(WindowId(1), buttons(true, true))]));
        access.sync(HashMap::from([(WindowId(1), buttons(false, true))]));
        access.sync(HashMap::new());
        assert_eq!(
            *log.lock().unwrap(),
            ["publish 1", "publish 1", "withdraw 1"]
        );
    }
}
