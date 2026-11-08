//! A window's tabs on CupertinoKit: each tab is a folder of its own, with its own history, view, scroll and search;
//! the window shows one at a time ([`crate::tab_strip`]).
//!
//! The tab shown lives in the window's own fields; the others are parked here and swapped in when shown, so every
//! other part of the window works on the tab shown without knowing about tabs. Every tab's folder wakes the window, which
//! applies their changes as they come.
//!
//! The tab bar sits under the toolbar while the window has more than one tab, or when View › Show Tab Bar asks for it:
//! one long rounded track the tabs share equally, the tab shown with a fill and rim of its own, and a round "+" button
//! after it that opens a tab. A tab shows its close button while the pointer is over it, and a tab dragged along the
//! track takes the place its middle is over when let go.

use cupertino_kit::controls::{Text, TextRole};
use cupertino_kit::core::{
    AccessibleStates, Action, Change, Element, Event, EventCx, MeasureCx, PaintCx, Phase,
    Propagation, Role, SemanticsCx, Widget,
};
use cupertino_kit::geometry::{Insets, Point, Rect, Size};
use cupertino_kit::layout::{
    Align, Axis, Constraints, ContainerKind, Flex, Justify, LayoutSpec, Measured, ScrollState,
    SizeSpec,
};
use cupertino_kit::platform::{Keysym, MouseButton, PointerEvent};
use cupertino_kit::render::{Paint, PaintOp, PathVerb, Shape};
use cupertino_kit::text::{AttributedText, TextStyle};
use cupertino_kit::theme::{Color, ColorRole, StateVariant};

use super::peek::{Glyph, GlyphButton};
use super::{columns, Event as WindowEvent, Message};
use crate::browser::Browser;
use crate::metrics::drag;
use crate::tab_strip::{drop_place, TabId};
use crate::window::ViewMode;

/// The height of the track the tabs share, and of the tab shown inside it.
const TRACK_HEIGHT: f64 = 28.0;
const TAB_HEIGHT: f64 = 26.0;
/// The tab shown in the dark appearance: the white over the track that lifts it, and its rim's.
const ACTIVE_FILL_DARK: f32 = 0.02;
const ACTIVE_RIM_DARK: f32 = 0.07;
/// Around the track: above and below it, and at the window's sides.
const BAR_VERTICAL: f64 = 4.0;
const BAR_SIDE: f64 = 8.0;
/// Between the track and the "+" button, and the button's side.
const PLUS_GAP: f64 = 4.0;
const PLUS: f64 = 26.0;
/// The side of a tab's close button, and how far it is in from the tab's leading edge.
const CLOSE: f64 = 16.0;
const CLOSE_INSET: f64 = 6.0;
/// The size of a tab's title.
const TITLE_SIZE: f32 = 12.0;
const RETURN: Keysym = Keysym(0xff0d);
const SPACE: Keysym = Keysym(0x20);

/// What the tab bar asks of its window.
#[derive(Clone, Debug, PartialEq)]
pub enum TabEvent {
    /// Show this tab.
    Select(TabId),
    /// Close this tab.
    Close(TabId),
    /// Move this tab to this place.
    Move(TabId, usize),
    /// Open a tab after the one shown.
    New,
}

/// A tab that is not shown: what the window keeps of each tab.
pub(super) struct Tab {
    pub browser: Browser,
    pub view: ViewMode,
    pub scroll: ScrollState,
    pub search: String,
    pub search_revision: u64,
    pub available: Option<u64>,
    pub mounted_read_only: bool,
    pub columns: columns::ColumnsView,
}

/// What the tab bar shows of one tab.
pub(super) struct Shown {
    pub id: TabId,
    pub title: String,
    pub active: bool,
}

/// The tab bar of window `id` with `tabs`, in order.
pub(super) fn bar(id: u64, tabs: Vec<Shown>) -> Element<Message> {
    let count = tabs.len();
    let items = tabs
        .into_iter()
        .enumerate()
        .map(|(place, tab)| {
            let key = format!("tab/{}", tab.id);
            Element::new(TabItem {
                window: id,
                id: tab.id,
                place,
                count,
                active: tab.active,
                title: tab.title.clone(),
                children: vec![Element::new(Text {
                    content: AttributedText::plain(tab.title, TextStyle::new(TITLE_SIZE)),
                    role: TextRole::Body,
                    wrap: false,
                    secondary: !tab.active,
                    selected: false,
                })],
            })
            .key(key)
            .fill()
        })
        .collect();
    let track = Element::new(Track { children: items }).fill();
    let plus = Element::new(GlyphButton {
        glyph: Glyph::Plus,
        label: "New Tab".into(),
        size: PLUS,
        on_press: Message::Window(id, WindowEvent::Tab(TabEvent::New)),
    })
    .key("new-tab");
    Element::new(TabBar {
        children: vec![track, plus],
    })
    .key("tab-bar")
}

fn no_measure() -> Measured {
    Measured {
        size: Size::ZERO,
        baseline: None,
    }
}

/// The bar: the track and the "+" button, read as the window's tab list.
struct TabBar {
    children: Vec<Element<Message>>,
}

impl Widget<Message> for TabBar {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            insets: Insets {
                top: BAR_VERTICAL,
                left: BAR_SIDE,
                bottom: BAR_VERTICAL,
                right: BAR_SIDE,
            },
            width: SizeSpec::Fill,
            height: SizeSpec::Fixed(TRACK_HEIGHT + 2.0 * BAR_VERTICAL),
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: Axis::Horizontal,
                spacing: PLUS_GAP,
                align: Align::Center,
                justify: Justify::Start,
            })
        }
    }

    fn take_children(&mut self) -> Vec<Element<Message>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, Message>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Group);
        cx.name("Tab Bar");
    }
}

/// The long rounded track the tabs share.
struct Track {
    children: Vec<Element<Message>>,
}

impl Widget<Message> for Track {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            insets: Insets::uniform((TRACK_HEIGHT - TAB_HEIGHT) / 2.0),
            width: SizeSpec::Fill,
            height: SizeSpec::Fixed(TRACK_HEIGHT),
            flex: Flex::grow(1.0),
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: Axis::Horizontal,
                spacing: 0.0,
                align: Align::Stretch,
                justify: Justify::Start,
            })
        }
    }

    fn take_children(&mut self) -> Vec<Element<Message>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, Message>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let size = cx.size();
        let fill = cx
            .env()
            .theme
            .colors
            .get(ColorRole::QuaternaryLabel, StateVariant::NORMAL);
        if let Some(fill) = fill {
            cx.push(PaintOp::Fill {
                shape: Shape::RoundedRect {
                    rect: Rect::new(0.0, 0.0, size.width, size.height),
                    radius: size.height / 2.0,
                },
                paint: Paint::Solid(fill),
            });
        }
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::TabList);
        cx.name("Tabs");
    }
}

/// One tab of the track.
struct TabItem {
    window: u64,
    id: TabId,
    /// Its place in the track, of `count`.
    place: usize,
    count: usize,
    active: bool,
    title: String,
    children: Vec<Element<Message>>,
}

/// The pointer over a tab, and a press on it that may drag it.
#[derive(Default)]
struct TabState {
    hover: bool,
    /// Where the press was, from the tab's leading edge, and in the window.
    press: Option<(f64, Point)>,
    dragging: bool,
}

impl TabItem {
    fn emit(&self, event: TabEvent, cx: &mut EventCx<'_, Message>) {
        cx.emit(Message::Window(self.window, WindowEvent::Tab(event)));
    }

    /// The close button's rect, in the tab's coordinates.
    fn close_rect(size: Size) -> Rect {
        Rect::new(CLOSE_INSET, (size.height - CLOSE) / 2.0, CLOSE, CLOSE)
    }
}

impl Widget<Message> for TabItem {
    type State = TabState;

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            insets: Insets {
                top: 0.0,
                left: CLOSE_INSET + CLOSE,
                bottom: 0.0,
                right: CLOSE_INSET + CLOSE,
            },
            width: SizeSpec::Fill,
            height: SizeSpec::Fill,
            flex: Flex::grow(1.0),
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: Axis::Horizontal,
                spacing: 0.0,
                align: Align::Center,
                justify: Justify::Center,
            })
        }
    }

    fn take_children(&mut self) -> Vec<Element<Message>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) -> TabState {
        TabState::default()
    }

    fn update(&self, old: &Self, _: &mut TabState) -> Change {
        if (self.active, self.place, self.count, &self.title)
            != (old.active, old.place, old.count, &old.title)
        {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &TabState, _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(
        &self,
        state: &mut TabState,
        event: &Event,
        cx: &mut EventCx<'_, Message>,
    ) -> Propagation {
        // a tab pressed follows the pointer it holds, whichever way its events come
        if cx.phase() == Phase::Capture && state.press.is_none() {
            return Propagation::Continue;
        }
        let Some(bounds) = cx.bounds() else {
            return Propagation::Continue;
        };
        match *event {
            Event::Pointer(PointerEvent::Motion { position }, _) => {
                if let Some((offset, at)) = state.press {
                    if !state.dragging && (position.x - at.x).abs() >= drag::START_DISTANCE {
                        state.dragging = true;
                    }
                    let _ = offset;
                    return Propagation::Stop;
                }
                if !state.hover {
                    state.hover = true;
                    cx.request_paint();
                }
                Propagation::Continue
            }
            Event::Pointer(PointerEvent::Leave, _) => {
                state.hover = false;
                cx.request_paint();
                Propagation::Continue
            }
            Event::Pointer(
                PointerEvent::Button {
                    position,
                    button: MouseButton::Left,
                    pressed: true,
                    ..
                },
                _,
            ) => {
                let local = Point::new(position.x - bounds.min_x(), position.y - bounds.min_y());
                if self.count > 1 && Self::close_rect(bounds.size).contains(local) {
                    self.emit(TabEvent::Close(self.id), cx);
                    return Propagation::Stop;
                }
                if !self.active {
                    self.emit(TabEvent::Select(self.id), cx);
                }
                state.press = Some((local.x, position));
                state.dragging = false;
                cx.capture_pointer();
                Propagation::Stop
            }
            Event::Pointer(
                PointerEvent::Button {
                    position,
                    button: MouseButton::Left,
                    pressed: false,
                    ..
                },
                _,
            ) => {
                let Some((offset, _)) = state.press.take() else {
                    return Propagation::Continue;
                };
                cx.release_pointer();
                if std::mem::take(&mut state.dragging) {
                    let width = bounds.width();
                    let track = bounds.min_x() - self.place as f64 * width;
                    let middle = position.x - offset + width / 2.0 - track;
                    let to = drop_place(middle, self.count, width * self.count as f64);
                    if to != self.place {
                        self.emit(TabEvent::Move(self.id, to), cx);
                    }
                }
                Propagation::Stop
            }
            Event::Cancel => {
                state.press = None;
                state.dragging = false;
                Propagation::Continue
            }
            Event::Key(key, _)
                if key.pressed && key.mods.is_empty() && matches!(key.keysym, RETURN | SPACE) =>
            {
                self.emit(TabEvent::Select(self.id), cx);
                Propagation::Stop
            }
            Event::Action(ref request)
                if cx.node() == cx.target() && request.action == Action::Click =>
            {
                self.emit(TabEvent::Select(self.id), cx);
                Propagation::Stop
            }
            Event::Focus { .. } if cx.node() == cx.target() => {
                cx.request_paint();
                Propagation::Continue
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, state: &TabState, cx: &mut PaintCx<'_>) {
        let size = cx.size();
        let rect = Rect::new(0.0, 0.0, size.width, size.height);
        let colors = cx.env().theme.colors.clone();
        if self.active {
            // light: a raised control on the track; dark: barely lifted off it, set apart by its rim
            let dark = cx.env().theme.preferences.appearance.is_dark();
            let white = Color::new(1.0, 1.0, 1.0, 1.0);
            let (fill, rim) = match dark {
                true => (
                    Some(white.with_alpha(ACTIVE_FILL_DARK)),
                    Some(white.with_alpha(ACTIVE_RIM_DARK)),
                ),
                false => (
                    colors.get(ColorRole::ControlFill, StateVariant::NORMAL),
                    colors.get(ColorRole::ControlBorder, StateVariant::NORMAL),
                ),
            };
            if let Some(fill) = fill {
                cx.push(PaintOp::Fill {
                    shape: Shape::RoundedRect {
                        rect,
                        radius: size.height / 2.0,
                    },
                    paint: Paint::Solid(fill),
                });
            }
            if let Some(rim) = rim {
                cx.push(PaintOp::Stroke {
                    shape: Shape::RoundedRect {
                        rect: Rect::new(0.5, 0.5, size.width - 1.0, size.height - 1.0),
                        radius: size.height / 2.0 - 0.5,
                    },
                    paint: Paint::Solid(rim),
                    width: 1.0,
                });
            }
        } else if self.place > 0 {
            // a hairline before an inactive tab that does not follow the tab shown
            if let Some(line) = colors.get(ColorRole::Separator, StateVariant::NORMAL) {
                cx.push(PaintOp::Fill {
                    shape: Shape::Rect(Rect::new(0.0, size.height * 0.25, 1.0, size.height * 0.5)),
                    paint: Paint::Solid(line),
                });
            }
        }
        if state.hover && self.count > 1 {
            let close = Self::close_rect(size);
            if let Some(ink) = colors.get(ColorRole::SecondaryText, StateVariant::NORMAL) {
                let c = close.center();
                let arm = CLOSE * 0.2;
                cx.push(PaintOp::Stroke {
                    shape: Shape::Path {
                        verbs: vec![
                            PathVerb::MoveTo(Point::new(c.x - arm, c.y - arm)),
                            PathVerb::LineTo(Point::new(c.x + arm, c.y + arm)),
                            PathVerb::MoveTo(Point::new(c.x + arm, c.y - arm)),
                            PathVerb::LineTo(Point::new(c.x - arm, c.y + arm)),
                        ],
                        fill_rule: Default::default(),
                    },
                    paint: Paint::Solid(ink),
                    width: 1.5,
                });
            }
        }
        if cx.is_focused() && cx.focus_visible() {
            if let Some(ring) = colors.get(ColorRole::KeyboardFocusIndicator, StateVariant::NORMAL)
            {
                cx.push(PaintOp::Stroke {
                    shape: Shape::RoundedRect {
                        rect: Rect::new(1.0, 1.0, size.width - 2.0, size.height - 2.0),
                        radius: size.height / 2.0 - 1.0,
                    },
                    paint: Paint::Solid(ring),
                    width: 2.0,
                });
            }
        }
    }

    fn semantics(&self, _: &TabState, cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Tab);
        cx.name(self.title.clone());
        cx.item_index(self.place);
        cx.item_count(self.count);
        cx.action(Action::Click);
        cx.states(AccessibleStates {
            focusable: true,
            selected: self.active,
            ..AccessibleStates::default()
        });
    }
}
