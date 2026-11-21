//! The chooser of what a screen-sharing request shares, for `org.cupertino.Screenshot1.SelectSources`: the app that
//! asks, the displays and windows the portal lists, and whether the person lets the app share again without asking.
//!
//! Nothing is chosen until the person picks a row after [`ARMING`](crate::color_picker::ARMING), counted from the
//! window's first frame the server showed, so keys meant for the app cannot share; the window's focus starts on Cancel.

use std::time::Instant;

use cupertino_kit::controls::{
    AppIcon, CheckBox, ListView, Selection, TextRole, WindowDragArea, PANEL_WINDOW_BACKGROUND,
    SIDEBAR_TOP,
};
use cupertino_kit::core::{
    Change, Checked, CommandRegistry, Event as CoreEvent, EventCx, Key, MeasureCx, PaintCx, Phase,
    Propagation, SemanticsCx, Shortcut, Widget,
};
use cupertino_kit::geometry::{Insets, Point, Size};
use cupertino_kit::layout::{Constraints, ContainerKind, LayoutSpec, Measured, ScrollState};
use cupertino_kit::platform::{DecorationMode, ForeignParent, Keysym, Mods};
use cupertino_kit::prelude::*;
use cupertino_kit::text::{AttributedText, TextStyle};

use crate::color_picker::ARMING;

/// The app id of the windows that ask the person to allow something; the server keeps desktop control's input away
/// from such windows of the programs it trusts with them.
pub const PROMPT_APP_ID: &str = "org.cupertino.Portal.Prompt";
/// The window's title.
pub const TITLE: &str = "Share Screen";
/// The window's size and its least size: the prompt, the list and the bar under the title bar's room.
const SIZE: Size = Size::new(480.0, 360.0 + SIDEBAR_TOP);
const MIN_SIZE: Size = Size::new(360.0, 240.0 + SIDEBAR_TOP);
/// Title bar and content as one surface, as the portal's prompts draw.
const DECORATIONS: DecorationMode = DecorationMode::Unified;
/// The app's icon in the header, a list row's height and its icon, in points.
const APP_ICON: f64 = 32.0;
const ROW_HEIGHT: f64 = 24.0;
const ROW_ICON: f64 = 16.0;
/// Room around the header and the bar, and between their parts.
const BAR_INSETS: Insets = Insets {
    top: 12.0,
    left: 16.0,
    bottom: 12.0,
    right: 16.0,
};
const GAP: f64 = 8.0;
/// Space between a row's icon and its name.
const ROW_SPACING: f64 = 6.0;
/// The text size of the prompt and the rows.
const TEXT_SIZE: f32 = 13.0;
const RETURN: Keysym = Keysym(0xff0d);
const ESCAPE: Keysym = Keysym(0xff1b);
const KP_ENTER: Keysym = Keysym(0xff8d);
const SPACE: Keysym = Keysym(0x20);
const NO_SCROLL: ScrollState = ScrollState {
    offset: Point::ZERO,
    content: Size::ZERO,
    viewport: Size::ZERO,
};

/// What a source is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceKind {
    Display,
    Window,
}

/// A source as its row shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    pub kind: SourceKind,
    /// The display's description or connector name; the window's title and app.
    pub label: String,
}

impl Source {
    /// A display named `name` (its connector, such as `DP-1`) and `description`: the description, else the name.
    pub fn display(name: &str, description: &str) -> Self {
        let label = if description.is_empty() {
            name
        } else {
            description
        };
        Self {
            kind: SourceKind::Display,
            label: label.to_owned(),
        }
    }

    /// A window with `title` of app `app_id`: both when both are known.
    pub fn window(title: &str, app_id: &str) -> Self {
        let label = match (title.is_empty(), app_id.is_empty()) {
            (false, false) => format!("{title} ({app_id})"),
            (false, true) => title.to_owned(),
            (true, _) => app_id.to_owned(),
        };
        Self {
            kind: SourceKind::Window,
            label,
        }
    }

    fn symbol(&self) -> &'static str {
        match self.kind {
            SourceKind::Display => "symbol.desktop",
            SourceKind::Window => "symbol.applications",
        }
    }
}

/// What the chooser shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ask {
    /// The name of the app's desktop entry, else its id; `None` for an app without an id.
    pub app_name: Option<String>,
    /// The icon name of the app's desktop entry, if it has one.
    pub app_icon: Option<String>,
    /// `wayland:<handle>`, `x11:<xid>` or empty.
    pub parent_window: String,
    pub sources: Vec<Source>,
    /// The app asked to restore the share later, so the chooser offers to remember it (off until ticked).
    pub offer_remember: bool,
}

/// What the person did in a chooser, by its id.
#[derive(Clone, Debug)]
pub enum Input {
    Scroll(u64, ScrollState),
    Select(u64, Selection),
    Share(u64),
    Remember(u64, bool),
    Cancel(u64),
}

/// One chooser on screen.
pub struct Picker {
    pub id: u64,
    pub ask: Ask,
    rows: Selection,
    scroll: ScrollState,
    /// The person ticked to let the app restore the share.
    remember: bool,
    /// The arming delay has passed: a row can be picked and shared.
    armed: bool,
    /// From the window's first presented frame; `None` before it.
    arming: Option<Instant>,
}

impl Picker {
    pub fn new(id: u64, ask: Ask) -> Self {
        Self {
            id,
            ask,
            rows: Selection::default(),
            scroll: NO_SCROLL,
            remember: false,
            armed: false,
            arming: None,
        }
    }

    /// The window presented its first frame at `at`: the arming delay starts. False when it had started before.
    pub fn shown(&mut self, at: Instant) -> bool {
        if self.arming.is_some() {
            return false;
        }
        self.arming = Some(at);
        true
    }

    /// The arming delay has passed, by its timer.
    pub fn arm(&mut self) {
        self.armed = true;
    }

    /// Whether the window takes a pick: armed by its timer, and the delay has passed by the clock too.
    fn ready(&self) -> bool {
        self.armed && self.arming.is_some_and(|a| a.elapsed() >= ARMING)
    }

    /// The chosen source, by its index in the ask.
    pub fn chosen(&self) -> Option<usize> {
        self.rows.cursor().filter(|i| *i < self.ask.sources.len())
    }

    /// What `input` does to this chooser: `Some` with the answer when it ends it, the chosen index and whether to
    /// remember it, or `None` for a cancel.
    pub fn input(&mut self, input: Input) -> Option<Option<(usize, bool)>> {
        match input {
            Input::Scroll(_, scroll) => self.scroll = scroll,
            // a row is not picked before the window is armed: a key typed for the app moves nothing
            Input::Select(_, rows) if self.ready() => self.rows = rows,
            Input::Remember(_, on) if self.ready() => self.remember = on,
            Input::Share(_) if self.ready() => {
                let chosen = self.chosen()?;
                return Some(Some((chosen, self.remember && self.ask.offer_remember)));
            }
            Input::Cancel(_) => return Some(None),
            Input::Select(..) | Input::Remember(..) | Input::Share(_) => {}
        }
        None
    }
}

/// The id of the chooser `input` is for.
pub fn input_id(input: &Input) -> u64 {
    match input {
        Input::Scroll(id, _)
        | Input::Select(id, _)
        | Input::Share(id)
        | Input::Remember(id, _)
        | Input::Cancel(id) => *id,
    }
}

/// The key of chooser `id`'s window.
pub fn window_key(id: u64) -> WindowKey {
    WindowKey(Key::from(format!("share/{id}")))
}

/// What the window asks: the app by its name, else an unnamed app.
pub fn prompt(ask: &Ask) -> String {
    let app = match &ask.app_name {
        Some(name) => format!("“{name}” wants to share your screen."),
        None => "An application wants to share your screen.".to_owned(),
    };
    if ask.sources.is_empty() {
        format!("{app} There is no display or window to share.")
    } else {
        format!("{app} Choose a display or a window.")
    }
}

fn text<M: 'static>(content: String, wrap: bool, selected: bool) -> Element<M> {
    Element::new(Text {
        content: AttributedText::plain(content, TextStyle::new(TEXT_SIZE)),
        role: TextRole::Body,
        wrap,
        secondary: false,
        selected,
    })
}

fn symbol<M: 'static>(name: &str, size: f64) -> Element<M> {
    Element::new(Icon {
        name: name.to_owned(),
        size,
        direction: Default::default(),
        label: None,
        tint: None,
    })
}

fn row<M: 'static>(source: &Source, selected: bool) -> Element<M> {
    Element::new(Stack {
        axis: Axis::Horizontal,
        padding: Insets {
            top: 0.0,
            left: GAP,
            bottom: 0.0,
            right: GAP,
        },
        spacing: ROW_SPACING,
        align: Align::Center,
        children: vec![
            symbol(source.symbol(), ROW_ICON),
            text(source.label.clone(), false, selected),
        ],
    })
}

fn button<M: Clone + 'static>(
    label: &str,
    role: ButtonRole,
    enabled: bool,
    on_press: M,
) -> Element<M> {
    Element::new(Button {
        label: label.to_owned(),
        role,
        style: ButtonStyle::Bordered,
        enabled,
        busy: false,
        on_press: Some(on_press),
    })
}

/// The header: the requesting app's icon (by its desktop entry's icon name, else the applications symbol) above the
/// prompt, which wraps inside the window.
fn header<M: 'static>(ask: &Ask) -> Element<M> {
    let icon = match &ask.app_icon {
        Some(name) => Element::new(AppIcon {
            name: name.clone(),
            size: APP_ICON,
        }),
        None => symbol("symbol.applications", APP_ICON),
    }
    .key("app-icon");
    Element::new(Stack {
        axis: Axis::Vertical,
        padding: BAR_INSETS,
        spacing: GAP,
        align: Align::Stretch,
        children: vec![
            // at the leading edge, not stretched across the header
            Element::new(Stack {
                axis: Axis::Horizontal,
                padding: Insets::ZERO,
                spacing: 0.0,
                align: Align::Center,
                children: vec![icon, Element::new(Spacer { min: 0.0 })],
            }),
            text(prompt(ask), true, false).key("prompt"),
        ],
    })
}

/// The window of chooser `p`, its messages made by `to`.
pub fn window<M: Clone + 'static>(p: &Picker, to: fn(Input) -> M) -> Window<M> {
    let id = p.id;
    let sources = p.ask.sources.clone();
    let names: Vec<String> = sources.iter().map(|s| s.label.clone()).collect();
    let list = Element::from(ListView {
        label: "Displays and Windows".into(),
        count: sources.len(),
        row_height: ROW_HEIGHT,
        scroll: p.scroll,
        selection: p.rows.clone(),
        multiple: false,
        initial: false,
        row: Box::new(move |i, selected| row(&sources[i], selected)),
        key: Some(Box::new(|i| Key::Int(i as u64))),
        type_select: Some(Box::new(move |i| names[i].clone())),
        on_scroll: Box::new(move |s| Some(to(Input::Scroll(id, s)))),
        on_select: Some(Box::new(move |s| Some(to(Input::Select(id, s))))),
        // Return reaches the window's shortcut, which a held key's repeats do not send
        double_click_activates: true,
        on_activate: None,
    })
    .key("sources");
    let remember = p.ask.offer_remember.then(|| {
        let app = p.ask.app_name.as_deref().unwrap_or("the app");
        Element::new(CheckBox {
            label: format!("Let “{app}” share it again without asking"),
            checked: if p.remember {
                Checked::True
            } else {
                Checked::False
            },
            enabled: p.armed,
            on_toggle: Some(Box::new(move |on: bool| to(Input::Remember(id, on)))),
        })
        .key("remember")
    });
    let mut bar: Vec<Element<M>> = remember.into_iter().collect();
    bar.push(Element::new(Spacer { min: GAP }));
    bar.push(Element::new(Deny {
        armed: p.armed,
        children: vec![
            button("Cancel", ButtonRole::Normal, true, to(Input::Cancel(id))).key("cancel"),
        ],
    }));
    bar.push(
        button(
            "Share",
            ButtonRole::Default,
            p.armed && p.chosen().is_some(),
            to(Input::Share(id)),
        )
        .key("share"),
    );
    Window {
        key: window_key(id),
        spec: WindowSpec {
            title: TITLE.to_owned(),
            app_id: PROMPT_APP_ID.into(),
            size: SIZE,
            min_size: Some(MIN_SIZE),
            max_size: None,
            decorations: DECORATIONS,
            foreign_parent: foreign_parent(&p.ask.parent_window),
            input_region: None,
        },
        // Return presses the focused button, Cancel when the window opens; with the focus in the list it is the default
        // button once a row is picked and the window is armed. Only a fresh press of Return shares, never a key held
        // since before. Escape cancels wherever the focus is
        shortcuts: CommandRegistry::new()
            .with_fresh(
                Shortcut {
                    mods: Mods::NONE,
                    keysym: RETURN,
                },
                to(Input::Share(id)),
            )
            .with(
                Shortcut {
                    mods: Mods::NONE,
                    keysym: ESCAPE,
                },
                to(Input::Cancel(id)),
            ),
        on_close: Some(to(Input::Cancel(id))),
        background: PANEL_WINDOW_BACKGROUND,
        popup: None,
        menus: None,
        content: Element::from(WindowDragArea {
            content: Element::new(Stack {
                axis: Axis::Vertical,
                padding: Insets {
                    top: SIDEBAR_TOP,
                    ..Insets::ZERO
                },
                spacing: 0.0,
                align: Align::Stretch,
                children: vec![
                    header(&p.ask),
                    Element::new(Separator {
                        axis: Axis::Horizontal,
                    }),
                    list,
                    Element::new(Separator {
                        axis: Axis::Horizontal,
                    }),
                    Element::new(Stack {
                        axis: Axis::Horizontal,
                        padding: BAR_INSETS,
                        spacing: GAP,
                        align: Align::Center,
                        children: bar,
                    }),
                ],
            }),
        }),
    }
}

/// The xdg-foreign parent of a request's `parent_window` (`wayland:<handle>`), which the window opens as a child of.
/// An X11 window (`x11:<xid>`), an empty one or an empty handle names none, and the window opens on its own.
fn foreign_parent(parent_window: &str) -> Option<ForeignParent> {
    parent_window
        .strip_prefix("wayland:")
        .filter(|handle| !handle.is_empty())
        .map(|handle| ForeignParent {
            handle: handle.to_owned(),
            modal: false,
        })
}

/// Holds the Cancel button: the window's keyboard focus starts on it, so the answer a stray key reaches is the one that
/// denies. Until the window is `armed` the keys that press a button (Return, Enter and Space) are taken before they
/// reach it, so a key typed for the app does nothing at all; Tab and Escape still work, and a click on Cancel denies at
/// once.
struct Deny<M> {
    armed: bool,
    children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for Deny<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Overlay)
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.armed == old.armed {
            Change::None
        } else {
            Change::Paint
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::ZERO,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), event: &CoreEvent, cx: &mut EventCx<'_, M>) -> Propagation {
        match event {
            CoreEvent::Key(key, _)
                if !self.armed
                    && cx.phase() == Phase::Capture
                    && matches!(key.keysym, RETURN | KP_ENTER | SPACE) =>
            {
                Propagation::Stop
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.initial_focus();
    }
}
