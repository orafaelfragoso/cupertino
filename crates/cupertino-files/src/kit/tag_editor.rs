//! The tags popover on CupertinoKit: the toolbar's Tags button opens it below itself for the selected items.
//!
//! A field at the top filters the tags listed and, with Return, adds the tag typed to every selected item, which makes
//! a tag of that name if there was none. Under it the tags are listed, each with its colour's dot: the seven colour
//! tags, then the other tags the window's folders carry, by name. A tag every selected item carries is checked; a
//! click on a tag takes it off them all if so, else adds it to those without it. Tags are kept as the items'
//! extended attributes ([`crate::tags`]).

use std::collections::HashMap;
use std::rc::Rc;

use cupertino_kit::controls::{BareField, Popover, Text, TextField, TextRole};
use cupertino_kit::core::{
    AccessibleValue, Action, Change, Checked, Element, Event, EventCx, Key, MeasureCx, PaintCx,
    Phase, Propagation, Role, SemanticsCx, Widget, Window, WindowKey,
};
use cupertino_kit::geometry::{Insets, Point, Rect, Size};
use cupertino_kit::layout::{
    Align, Axis, Constraints, ContainerKind, Justify, LayoutSpec, Measured, ScrollState, SizeSpec,
};
use cupertino_kit::platform::{Keysym, MouseButton, PointerEvent};
use cupertino_kit::prelude::{Stack, TextChange};
use cupertino_kit::render::{Paint, PaintOp, PathVerb, Shape};
use cupertino_kit::text::{AttributedText, ContentPurpose, FontWeight, TextStyle};
use cupertino_kit::theme::{Appearance, Color, ColorRole, StateVariant};

use super::view::popup_key;
use super::{window_key, Event as WindowEvent, Message, Popup, NO_SCROLL};
use crate::context_menu::TAG_ROW_ORDER;
use crate::tags::TagColor;

/// The popover's width.
const WIDTH: f64 = 217.0;
/// A tag's row: its height, the side of its colour's dot and the room after the dot.
const ROW_HEIGHT: f64 = 19.0;
/// Between the title and the field, and from the field to the first row.
const TITLE_GAP: f64 = 6.0;
/// The height of the field that filters the tags.
const FIELD_HEIGHT: f64 = 26.0;
/// The room between the field's frame and its text, and the frame's corner radius.
const FIELD_INSET: f64 = 8.0;
const FIELD_RADIUS: f64 = 7.0;
/// How much of the theme's field border shows on the frame.
const FIELD_EDGE_ALPHA: f32 = 0.5;
const ROWS_GAP: f64 = 11.0;
const DOT: f64 = 10.0;
const DOT_GAP: f64 = 4.5;
/// From the row's leading edge to the dot.
const ROW_INSET: f64 = 14.5;
/// The side of the check mark of a tag every selected item carries.
const CHECK: f64 = 10.0;
const TITLE_SIZE: f32 = 13.0;
/// The size of the heading over the field.
const HEADING_SIZE: f32 = 11.0;
const RETURN: Keysym = Keysym(0xff0d);
const SPACE: Keysym = Keysym(0x20);

/// The popover while it is open.
pub(super) struct TagEditor {
    pub filter: String,
    pub revision: u64,
    pub scroll: ScrollState,
}

impl TagEditor {
    pub fn new() -> TagEditor {
        TagEditor {
            filter: String::new(),
            revision: 0,
            scroll: NO_SCROLL,
        }
    }
}

/// What the popover asks of its window.
#[derive(Clone, Debug, PartialEq)]
pub enum TagEvent {
    /// The field's text changed.
    Filter(TextChange),
    /// Take the tag of this name off every selected item, or add it to them.
    Toggle(String),
    /// Return in the field: add the tag typed.
    Submit,
    Close,
    Scroll(ScrollState),
}

/// The colour of each colour tag's dot, light and dark, from the theme's system colours.
pub(super) type TagColors = Rc<HashMap<TagColor, (Option<Color>, Option<Color>)>>;

/// The dots' colours of the installed theme; none when it cannot be read, and the dots are then drawn in the secondary
/// text colour.
pub(super) fn tag_colors() -> TagColors {
    let theme = cupertino_theme::Theme::load()
        .map_err(|e| eprintln!("cupertino-files: no theme for the tags' colours: {e}"))
        .ok();
    let colors = TagColor::ALL
        .into_iter()
        .map(|color| {
            let of = |appearance| {
                theme
                    .as_ref()
                    .and_then(|t| t.tokens.color(appearance, color.system_color_name()))
            };
            (color, (of(Appearance::Light), of(Appearance::Dark)))
        })
        .collect();
    Rc::new(colors)
}

/// The tags the popover lists: the colour tags, then `known` (the other tags seen), each with whether every selected
/// item carries it (`on`), keeping those whose names contain `filter`.
pub(super) fn listed(
    known: &[String],
    on: impl Fn(&str) -> bool,
    filter: &str,
) -> Vec<(String, Option<TagColor>, bool)> {
    let filter = filter.to_lowercase();
    TAG_ROW_ORDER
        .iter()
        .map(|c| c.title().to_owned())
        .chain(
            known
                .iter()
                .filter(|name| TagColor::from_title(name).is_none())
                .cloned(),
        )
        .filter(|name| name.to_lowercase().contains(&filter))
        .map(|name| {
            let color = TagColor::from_title(&name);
            let checked = on(&name);
            (name, color, checked)
        })
        .collect()
}

fn send(id: u64, event: TagEvent) -> Message {
    Message::Window(id, WindowEvent::Tags(event))
}

/// The popover of window `id` for `subject` (the selected item's name, or how many are selected), listing `tags`.
pub(super) fn window(
    id: u64,
    editor: &TagEditor,
    subject: &str,
    tags: Vec<(String, Option<TagColor>, bool)>,
    colors: &TagColors,
) -> Window<Message> {
    let title = Element::new(Text {
        content: AttributedText::plain(
            format!("Tags for {subject}"),
            TextStyle {
                weight: FontWeight::SEMIBOLD,
                ..TextStyle::new(HEADING_SIZE)
            },
        ),
        role: TextRole::Body,
        wrap: false,
        secondary: true,
        selected: false,
    });
    let input = TextField {
        label: "Tag".into(),
        placeholder: String::new(),
        value: editor.filter.clone(),
        revision: editor.revision,
        purpose: ContentPurpose::Normal,
        enabled: true,
        on_change: Some(Box::new(move |change| send(id, TagEvent::Filter(change)))),
        on_submit: Some(send(id, TagEvent::Submit)),
        selection: None,
        initial: true,
    };
    // the field's own chrome is shorter than this popover's, so the popover draws the frame and the text stands bare
    let field = Element::new(FieldFrame {
        children: vec![Element::new(BareField {
            field: input,
            size: f64::from(TITLE_SIZE),
            hold_caret: false,
        })
        .key("tag-field")
        // the field takes the frame's width, so its text has room and it can be pressed anywhere in the frame
        .fill()],
    });
    let rows = tags.into_iter().map(|(name, color, checked)| {
        let dot = color.and_then(|c| colors.get(&c).copied());
        Element::new(TagRow {
            name: name.clone(),
            dot,
            colored: color.is_some(),
            checked,
            on_press: send(id, TagEvent::Toggle(name.clone())),
            children: vec![Element::new(Text {
                content: AttributedText::plain(name.clone(), TextStyle::new(TITLE_SIZE)),
                role: TextRole::Body,
                wrap: false,
                secondary: false,
                selected: false,
            })],
        })
        .key(Key::from(format!("tag/{name}")))
    });
    // the rows follow one another without gaps, set off from the field above them
    let rows = Element::new(Stack {
        axis: Axis::Vertical,
        padding: Insets {
            top: ROWS_GAP,
            ..Insets::ZERO
        },
        spacing: 0.0,
        align: Align::Stretch,
        children: rows.collect(),
    });
    // the title centred over the field
    let title = Element::new(Stack {
        axis: Axis::Vertical,
        padding: Insets::ZERO,
        spacing: 0.0,
        align: Align::Center,
        children: vec![title],
    });
    let content = Element::new(Stack {
        axis: Axis::Vertical,
        padding: Insets::ZERO,
        spacing: TITLE_GAP,
        align: Align::Stretch,
        children: vec![title, field, rows],
    })
    .width(WIDTH)
    .key("tags");
    Popover {
        parent: window_key(id),
        anchor: popup_key(Popup::Tags),
        label: "Tags".into(),
        content,
        scroll: editor.scroll,
        on_scroll: Box::new(move |s| send(id, TagEvent::Scroll(s))),
        on_close: send(id, TagEvent::Close),
    }
    .window(WindowKey(Key::from(format!("tags/{id}"))))
}

/// The frame of the filter field: a well of the text background, `FIELD_HEIGHT` high, with the bare field inside.
struct FieldFrame {
    children: Vec<Element<Message>>,
}

impl Widget<Message> for FieldFrame {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            insets: Insets {
                top: 0.0,
                left: FIELD_INSET,
                bottom: 0.0,
                right: FIELD_INSET,
            },
            width: SizeSpec::Fill,
            height: SizeSpec::Fixed(FIELD_HEIGHT),
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: Axis::Horizontal,
                spacing: 0.0,
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
        Measured {
            size: Size::ZERO,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, Message>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let size = cx.size();
        let theme = cx.env().theme.clone();
        let rect = Rect::new(0.0, 0.0, size.width, size.height);
        if let Some(fill) = theme
            .colors
            .get(ColorRole::TextBackground, StateVariant::NORMAL)
        {
            cx.push(PaintOp::Fill {
                shape: Shape::RoundedRect {
                    rect,
                    radius: FIELD_RADIUS,
                },
                paint: Paint::Solid(fill),
            });
        }
        if let Some(edge) = theme
            .colors
            .get(ColorRole::TextFieldBorder, StateVariant::NORMAL)
        {
            cx.push(PaintOp::Stroke {
                shape: Shape::RoundedRect {
                    rect: rect.inset(Insets::uniform(0.5)),
                    radius: FIELD_RADIUS - 0.5,
                },
                paint: Paint::Solid(Color {
                    a: edge.a * FIELD_EDGE_ALPHA,
                    ..edge
                }),
                width: 1.0,
            });
        }
    }

    // the frame has nothing of its own to read; the field inside it is read and focused as any field
    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// A tag of the list: its colour's dot, its name, and a check mark when every selected item carries it.
struct TagRow {
    name: String,
    /// The dot's colour, light and dark.
    dot: Option<(Option<Color>, Option<Color>)>,
    /// A colour tag: its dot is filled; another tag's dot is an outline.
    colored: bool,
    checked: bool,
    on_press: Message,
    children: Vec<Element<Message>>,
}

impl Widget<Message> for TagRow {
    type State = bool;

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            insets: Insets {
                top: 0.0,
                left: ROW_INSET + DOT + DOT_GAP,
                bottom: 0.0,
                right: ROW_INSET + CHECK + DOT_GAP,
            },
            width: SizeSpec::Fill,
            height: SizeSpec::Fixed(ROW_HEIGHT),
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: Axis::Horizontal,
                spacing: 0.0,
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
        if (self.checked, &self.name, self.dot.is_some())
            != (old.checked, &old.name, old.dot.is_some())
        {
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
        let size = cx.size();
        let theme = cx.env().theme.clone();
        let dark = matches!(
            theme.preferences.appearance,
            Appearance::Dark | Appearance::DarkHighContrast
        );
        if *hover || (cx.is_focused() && cx.focus_visible()) {
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
        let secondary = theme
            .colors
            .get(ColorRole::SecondaryText, StateVariant::NORMAL);
        let dot = self
            .dot
            .and_then(|(light, night)| if dark { night } else { light })
            .or(secondary);
        let rect = Rect::new(ROW_INSET, (size.height - DOT) / 2.0, DOT, DOT);
        if let Some(dot) = dot {
            let shape = Shape::RoundedRect {
                rect,
                radius: DOT / 2.0,
            };
            if self.colored {
                cx.push(PaintOp::Fill {
                    shape,
                    paint: Paint::Solid(dot),
                });
            } else {
                cx.push(PaintOp::Stroke {
                    shape: Shape::RoundedRect {
                        rect: Rect::new(
                            rect.min_x() + 0.5,
                            rect.min_y() + 0.5,
                            DOT - 1.0,
                            DOT - 1.0,
                        ),
                        radius: DOT / 2.0 - 0.5,
                    },
                    paint: Paint::Solid(dot),
                    width: 1.0,
                });
            }
        }
        if self.checked {
            if let Some(ink) = theme.colors.get(ColorRole::Text, StateVariant::NORMAL) {
                let x = size.width - ROW_INSET - CHECK;
                let y = (size.height - CHECK) / 2.0;
                cx.push(PaintOp::Stroke {
                    shape: Shape::Path {
                        verbs: vec![
                            PathVerb::MoveTo(Point::new(x, y + CHECK * 0.55)),
                            PathVerb::LineTo(Point::new(x + CHECK * 0.38, y + CHECK * 0.9)),
                            PathVerb::LineTo(Point::new(x + CHECK, y + CHECK * 0.1)),
                        ],
                        fill_rule: Default::default(),
                    },
                    paint: Paint::Solid(ink),
                    width: 1.6,
                });
            }
        }
    }

    fn semantics(&self, _: &bool, cx: &mut SemanticsCx<'_>) {
        cx.role(Role::CheckBox);
        cx.name(self.name.clone());
        cx.value(AccessibleValue::Toggle(if self.checked {
            Checked::True
        } else {
            Checked::False
        }));
        cx.action(Action::Click);
        cx.focusable();
        // a press on a tag leaves the field above the list editing, so a name can still be typed
        cx.keeps_editing();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_colours_come_first_then_the_other_tags_in_use_and_the_field_narrows_them() {
        let known = ["Red".to_owned(), "Errands".to_owned(), "Project".to_owned()];
        let on = |name: &str| name == "Project";
        let rows = listed(&known, on, "");
        let names: Vec<&str> = rows.iter().map(|(n, _, _)| n.as_str()).collect();
        assert_eq!(
            names,
            ["Red", "Orange", "Yellow", "Green", "Blue", "Purple", "Gray", "Errands", "Project"],
            "a colour in use is listed once, with the colours"
        );
        assert_eq!(rows[0].1, Some(TagColor::Red));
        assert_eq!(rows[8], ("Project".to_owned(), None, true));
        let narrowed: Vec<String> = listed(&known, on, "RE")
            .into_iter()
            .map(|(n, _, _)| n)
            .collect();
        assert_eq!(
            narrowed,
            ["Red", "Green"],
            "by any part of the name, in any case"
        );
    }
}
