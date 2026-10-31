//! What the lock screen tells assistive technology: one window per output with the clock, the user, the password field
//! and what the screen shows about the last attempt. The password never leaves the process: the field's value is
//! protected, so neither its text nor its length is told, and assistive technology can't type into it.

use std::num::NonZeroU32;

use cupertino_geometry::Rect;
use cupertino_ui_core::{
    AccessibilityNode, AccessibilityUpdate, AccessibleStates, AccessibleValue, Action, Live,
    NodeId, Role,
};
use cupertino_ui_platform::WindowId;

use crate::layout::Layout;
use crate::model::{LockScreen, Phase};
use crate::panel::{Decision, Frames, Kind};

/// What every rejected attempt says, whatever the reason: a wrong password, an unknown account or an authentication
/// error look alike.
pub const DENIED: &str = "Incorrect password. Try again.";

/// The lock screen of one output as the person sees it.
pub struct LockView<'a> {
    pub screen: &'a LockScreen,
    pub layout: &'a Layout,
    /// The output's size in points.
    pub size: (f64, f64),
    pub user_name: &'a str,
    pub time: &'a str,
    pub date: Option<&'a str>,
    pub caps_lock: bool,
    /// The password hint the screen shows after failed attempts.
    pub help: Option<&'a str>,
    /// The screen has a power item.
    pub power: bool,
}

/// The nodes of a lock screen; their ids stay the same across updates, so assistive technology keeps its place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LockNode {
    Window = 1,
    Time,
    Date,
    User,
    Password,
    Denied,
    Notice,
    CapsLock,
    Help,
    Power,
}

impl LockNode {
    pub fn id(self) -> NodeId {
        NodeId::new(self as u32, NonZeroU32::MIN)
    }

    /// The node `id` names, when it is one of a lock screen's.
    pub fn of(id: NodeId) -> Option<LockNode> {
        use LockNode::*;
        [
            Window, Time, Date, User, Password, Denied, Notice, CapsLock, Help, Power,
        ]
        .into_iter()
        .find(|n| n.id() == id)
    }
}

/// The whole tree of a lock screen on `window`.
pub fn lock_tree(window: WindowId, view: &LockView<'_>) -> AccessibilityUpdate {
    let layout = view.layout;
    let (width, height) = view.size;
    let line = |baseline: f64, size: f64| Rect::new(0.0, baseline - size, width, size * 1.25);
    let mut children = Vec::new();
    let mut nodes = Vec::new();
    let mut add = |node: LockNode, accessible: AccessibilityNode| {
        children.push(node.id());
        nodes.push((node.id(), accessible));
    };
    add(
        LockNode::Time,
        text(view.time, line(layout.time_baseline, layout.time_size)),
    );
    if let Some(date) = view.date {
        add(
            LockNode::Date,
            text(date, line(layout.date_baseline, layout.date_size)),
        );
    }
    add(
        LockNode::User,
        text(
            view.user_name,
            Rect::new(
                layout.avatar.x(),
                layout.avatar.y(),
                layout.avatar.width(),
                layout.name_baseline - layout.avatar.y(),
            ),
        ),
    );
    add(LockNode::Password, password(view.screen, layout.pill));
    let below = line(layout.notice_baseline, layout.time_size / 4.0);
    if view.screen.failure_outline() {
        add(LockNode::Denied, announced(DENIED, below));
    }
    if let Some(notice) = view.screen.notice() {
        add(LockNode::Notice, announced(notice, below));
    }
    if view.caps_lock {
        add(LockNode::CapsLock, text("Caps Lock is on", below));
    }
    if let Some(help) = view.help {
        add(
            LockNode::Help,
            text(help, line(layout.help_baseline, layout.time_size / 4.0)),
        );
    }
    if view.power {
        add(
            LockNode::Power,
            AccessibilityNode {
                actions: vec![Action::Click],
                states: AccessibleStates {
                    focusable: true,
                    ..AccessibleStates::default()
                },
                ..node(Role::Button, Some("Power"), layout.power_item)
            },
        );
    }
    let mut root = node(
        Role::Window,
        Some("Locked"),
        Rect::new(0.0, 0.0, width, height),
    );
    root.description = Some(view.user_name.to_owned());
    root.children = children;
    nodes.insert(0, (LockNode::Window.id(), root));
    AccessibilityUpdate {
        window,
        root: LockNode::Window.id(),
        focus: Some(LockNode::Password.id()),
        nodes,
        removed: Vec::new(),
        full: true,
    }
}

/// The first node id of a panel's buttons; button `i` is this plus `i`.
const PANEL_BUTTONS: u32 = 10;

fn panel_id(index: u32) -> NodeId {
    NodeId::new(index, NonZeroU32::MIN)
}

/// The whole tree of a confirmation panel of `kind` laid out in `frames` on `window`, with `seconds_left` on its
/// countdown. The button Return presses has the focus; each button's only action is a click.
pub fn panel_tree(
    window: WindowId,
    kind: Kind,
    frames: &Frames,
    seconds_left: Option<u32>,
) -> AccessibilityUpdate {
    let text = kind.text();
    let mut nodes = vec![(panel_id(2), self::text(text.title, frames.title))];
    let info = seconds_left.and_then(|s| kind.countdown_message(s));
    if let (Some(info), Some(frame)) = (info, frames.info) {
        nodes.push((panel_id(3), self::text(&info, frame)));
    }
    let default = Decision::Do(kind.default_action());
    let mut focus = None;
    for (i, button) in frames.buttons.iter().enumerate() {
        let id = panel_id(PANEL_BUTTONS + i as u32);
        let focused = button.decision == default;
        if focused {
            focus = Some(id);
        }
        nodes.push((
            id,
            AccessibilityNode {
                actions: vec![Action::Click],
                states: AccessibleStates {
                    focusable: true,
                    focused,
                    ..AccessibleStates::default()
                },
                ..node(Role::Button, Some(button.label), button.frame)
            },
        ));
    }
    let mut root = node(
        Role::AlertDialog,
        Some(text.title),
        Rect::new(0.0, 0.0, frames.width, frames.height),
    );
    root.states.modal = true;
    root.children = nodes.iter().map(|(id, _)| *id).collect();
    nodes.insert(0, (panel_id(1), root));
    AccessibilityUpdate {
        window,
        root: panel_id(1),
        focus,
        nodes,
        removed: Vec::new(),
        full: true,
    }
}

/// The decision a click from assistive technology on node `id` of a panel laid out in `frames` makes: the same as a
/// click on that button.
pub fn panel_click(frames: &Frames, id: NodeId) -> Option<Decision> {
    let index = id.index().checked_sub(PANEL_BUTTONS)?;
    (id.generation() == NonZeroU32::MIN)
        .then(|| frames.buttons.get(index as usize))
        .flatten()
        .map(|button| button.decision)
}

/// The password field: its value is protected in every phase, and the only action it offers is focus.
fn password(screen: &LockScreen, frame: Rect) -> AccessibilityNode {
    AccessibilityNode {
        value: Some(AccessibleValue::Protected),
        states: AccessibleStates {
            focusable: true,
            focused: true,
            busy: matches!(screen.phase(), Phase::Authenticating { .. }),
            invalid: screen.failure_outline(),
            ..AccessibleStates::default()
        },
        actions: vec![Action::Focus],
        ..node(Role::PasswordInput, Some("Password"), frame)
    }
}

/// Text that assistive technology reads out as soon as it shows: the answer to an attempt.
fn announced(content: &str, bounds: Rect) -> AccessibilityNode {
    AccessibilityNode {
        live: Some(Live::Assertive),
        ..text(content, bounds)
    }
}

fn text(content: &str, bounds: Rect) -> AccessibilityNode {
    node(Role::StaticText, Some(content), bounds)
}

fn node(role: Role, name: Option<&str>, bounds: Rect) -> AccessibilityNode {
    AccessibilityNode {
        role,
        name: name.map(str::to_owned),
        description: None,
        value: None,
        states: AccessibleStates::default(),
        bounds,
        actions: Vec::new(),
        relations: Vec::new(),
        children: Vec::new(),
        live: None,
        item_count: None,
        item_index: None,
    }
}

#[cfg(test)]
mod tests;
