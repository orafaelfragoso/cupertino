//! The share popover on CupertinoKit: the toolbar's Share button opens it below itself for the selected items.
//!
//! A header shows the item (its icon, its name and what it is, or how many items are selected), under it the
//! applications that open every selected item, one row each, and last a row that copies the selection. A row acts at
//! once and closes the popover.

use cupertino_kit::controls::{
    Icon, IconDirection, Image, ImageFit, ImageSource, Popover, Separator, Text, TextRole,
};
use cupertino_kit::core::{
    Action, Change, Element, Event, EventCx, Key, MeasureCx, PaintCx, Phase, Propagation, Role,
    SemanticsCx, Widget, Window, WindowKey,
};
use cupertino_kit::geometry::{Insets, Rect, Size};
use cupertino_kit::layout::{
    Align, Axis, Constraints, ContainerKind, Justify, LayoutSpec, Measured, ScrollState, SizeSpec,
};
use cupertino_kit::platform::{Keysym, MouseButton, PointerEvent};
use cupertino_kit::prelude::Stack;
use cupertino_kit::render::{Paint, PaintOp, Shape};
use cupertino_kit::text::{AttributedText, FontWeight, TextStyle};
use cupertino_kit::theme::{ColorRole, StateVariant};

use super::view::popup_key;
use super::{window_key, Event as WindowEvent, Message, Popup, NO_SCROLL};
use crate::context_menu::AppChoice;

/// The width of the popover's content.
const WIDTH: f64 = 230.0;
/// The room around the content: the separators start and end this far from the panel's sides.
const SIDE: f64 = 7.0;
const TOP: f64 = 12.0;
const BOTTOM: f64 = 4.0;
/// A row: its height, the room before its symbol, the symbol's side and the room after it.
const ROW_HEIGHT: f64 = 22.0;
const ROW_INSET: f64 = 1.5;
const SYMBOL: f64 = 15.0;
const SYMBOL_GAP: f64 = 8.0;
/// Between the parts of the popover, and from the separators to the rows.
const GAP: f64 = 6.0;
/// The header's icon side and the room between it and the text.
const HEADER_ICON: f64 = 36.0;
const HEADER_GAP: f64 = 10.0;
const NAME_SIZE: f32 = 13.0;
const KIND_SIZE: f32 = 11.0;
/// Most applications listed.
const MAX_APPS: usize = 8;
const RETURN: Keysym = Keysym(0xff0d);
const SPACE: Keysym = Keysym(0x20);

/// The popover while it is open.
pub(super) struct ShareState {
    pub scroll: ScrollState,
}

impl ShareState {
    pub fn new() -> ShareState {
        ShareState { scroll: NO_SCROLL }
    }
}

/// What the popover asks of its window.
#[derive(Clone, Debug, PartialEq)]
pub enum ShareEvent {
    /// Open the selection with the application of this desktop entry id.
    Open(String),
    /// Copy the selection.
    Copy,
    Close,
    Scroll(ScrollState),
}

fn send(id: u64, event: ShareEvent) -> Message {
    Message::Window(id, WindowEvent::Share(event))
}

/// What the header shows of the selection.
pub(super) struct Subject {
    pub name: String,
    pub kind: String,
    pub icon: Vec<ImageSource>,
}

fn text(content: String, size: f32, weight: FontWeight, secondary: bool) -> Element<Message> {
    Element::new(Text {
        content: AttributedText::plain(
            content,
            TextStyle {
                weight,
                ..TextStyle::new(size)
            },
        ),
        role: TextRole::Body,
        wrap: false,
        secondary,
        selected: false,
    })
}

/// The popover of window `id` for `subject`, offering `apps`.
pub(super) fn window(
    id: u64,
    state: &ShareState,
    subject: Subject,
    apps: &[AppChoice],
) -> Window<Message> {
    let header = Element::new(Stack {
        axis: Axis::Horizontal,
        padding: Insets {
            left: ROW_INSET,
            right: ROW_INSET,
            ..Insets::ZERO
        },
        spacing: HEADER_GAP,
        align: Align::Center,
        children: vec![
            Element::new(Image {
                sources: subject.icon,
                size: Size::new(HEADER_ICON, HEADER_ICON),
                fit: ImageFit::Contain,
                radius: 0.0,
                label: None,
            }),
            Element::new(Stack {
                axis: Axis::Vertical,
                padding: Insets::ZERO,
                spacing: 1.0,
                align: Align::Start,
                children: {
                    let mut lines =
                        vec![text(subject.name, NAME_SIZE, FontWeight::SEMIBOLD, false)];
                    if !subject.kind.is_empty() {
                        lines.push(text(subject.kind, KIND_SIZE, FontWeight::REGULAR, true));
                    }
                    lines
                },
            }),
        ],
    });
    let row = |key: String, symbol: &str, label: &str, on_press: Message| {
        Element::new(ShareRow {
            label: label.to_owned(),
            on_press,
            children: vec![
                Element::new(Icon {
                    name: symbol.into(),
                    size: SYMBOL,
                    direction: IconDirection::Fixed,
                    label: None,
                    tint: Some(ColorRole::SecondaryText),
                }),
                text(label.to_owned(), NAME_SIZE, FontWeight::REGULAR, false),
            ],
        })
        .key(Key::from(key))
    };
    let group = |rows: Vec<Element<Message>>| {
        // the rows of a group follow one another without gaps
        Element::new(Stack {
            axis: Axis::Vertical,
            padding: Insets::ZERO,
            spacing: 0.0,
            align: Align::Stretch,
            children: rows,
        })
    };
    let separator = || {
        Element::new(Separator {
            axis: Axis::Horizontal,
        })
    };
    let mut children = vec![header];
    if !apps.is_empty() {
        children.push(separator());
        children.push(group(
            apps.iter()
                .take(MAX_APPS)
                .map(|app| {
                    row(
                        format!("share/app/{}", app.id),
                        "symbol.applications",
                        &app.name,
                        send(id, ShareEvent::Open(app.id.clone())),
                    )
                })
                .collect(),
        ));
    }
    children.push(separator());
    children.push(group(vec![row(
        "share/copy".into(),
        "symbol.document",
        "Copy",
        send(id, ShareEvent::Copy),
    )]));
    let content = Element::new(Stack {
        axis: Axis::Vertical,
        padding: Insets::new(TOP, SIDE, BOTTOM, SIDE),
        spacing: GAP,
        align: Align::Stretch,
        children,
    })
    .width(WIDTH)
    .key("share");
    Popover {
        parent: window_key(id),
        anchor: popup_key(Popup::Share),
        label: "Share".into(),
        content,
        scroll: state.scroll,
        on_scroll: Box::new(move |s| send(id, ShareEvent::Scroll(s))),
        on_close: send(id, ShareEvent::Close),
    }
    .window(WindowKey(Key::from(format!("share/{id}"))))
}

/// A row of the popover: a symbol and a label, which highlights under the pointer and acts on a press, Return or Space.
struct ShareRow {
    label: String,
    on_press: Message,
    children: Vec<Element<Message>>,
}

impl Widget<Message> for ShareRow {
    type State = bool;

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            insets: Insets {
                top: 0.0,
                left: ROW_INSET,
                bottom: 0.0,
                right: ROW_INSET,
            },
            width: SizeSpec::Fill,
            height: SizeSpec::Fixed(ROW_HEIGHT),
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: Axis::Horizontal,
                spacing: SYMBOL_GAP,
                align: Align::Center,
                justify: Justify::Start,
            })
        }
    }

    fn take_children(&mut self) -> Vec<Element<Message>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) -> bool {
        false
    }

    fn update(&self, old: &Self, _: &mut bool) -> Change {
        if self.label != old.label {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &bool, _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::ZERO,
            baseline: None,
        }
    }

    fn event(&self, hover: &mut bool, event: &Event, cx: &mut EventCx<'_, Message>) -> Propagation {
        if cx.phase() == Phase::Capture {
            return Propagation::Continue;
        }
        match event {
            Event::Pointer(PointerEvent::Motion { .. }, _) if !*hover => {
                *hover = true;
                cx.request_paint();
                Propagation::Continue
            }
            Event::Pointer(PointerEvent::Leave, _) => {
                *hover = false;
                cx.request_paint();
                Propagation::Continue
            }
            Event::Pointer(
                PointerEvent::Button {
                    button: MouseButton::Left,
                    pressed: true,
                    ..
                },
                _,
            ) => {
                cx.emit(self.on_press.clone());
                Propagation::Stop
            }
            Event::Key(key, _)
                if key.pressed && key.mods.is_empty() && matches!(key.keysym, RETURN | SPACE) =>
            {
                cx.emit(self.on_press.clone());
                Propagation::Stop
            }
            Event::Action(request)
                if cx.node() == cx.target() && request.action == Action::Click =>
            {
                cx.emit(self.on_press.clone());
                Propagation::Stop
            }
            Event::Focus { .. } if cx.node() == cx.target() => {
                cx.request_paint();
                Propagation::Continue
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, hover: &bool, cx: &mut PaintCx<'_>) {
        if !(*hover || (cx.is_focused() && cx.focus_visible())) {
            return;
        }
        let size = cx.size();
        let theme = cx.env().theme.clone();
        if let Some(fill) = theme
            .colors
            .get(ColorRole::QuaternaryLabel, StateVariant::NORMAL)
        {
            cx.push(PaintOp::Fill {
                shape: Shape::RoundedRect {
                    rect: Rect::new(0.0, 0.0, size.width, size.height),
                    radius: 5.0,
                },
                paint: Paint::Solid(fill),
            });
        }
    }

    fn semantics(&self, _: &bool, cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Button);
        cx.name(self.label.clone());
        cx.action(Action::Click);
        cx.focusable();
    }
}
