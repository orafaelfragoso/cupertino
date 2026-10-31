mod support;

use cupertino_geometry::{Insets, Point, Rect, Size};
use cupertino_ui_controls::{Badge, ListView, Selection, Stack, Text, TextRole};
use cupertino_ui_core::{
    Application, Change, Command, Element, Event, EventCx, MeasureCx, PaintCx, Propagation, Role,
    SemanticsCx, Widget, Window,
};
use cupertino_ui_layout::{
    Align, Axis, Constraints, ContainerKind, LayoutSpec, Measured, ScrollState,
};
use cupertino_ui_platform::PointerEvent;
use cupertino_ui_render::{Paint, PaintOp, Shape};
use cupertino_ui_text::{AttributedText, TextStyle};
use cupertino_ui_theme::{ColorRole, InteractionVariant};
use support::{color, label_width, window, Harness};

const BADGES: [(&str, Option<&str>); 3] =
    [("one", Some("3")), ("many", Some("99+")), ("dot", None)];

/// Three mailboxes, each with a badge: one unread, many unread, and a dot.
struct Mailboxes;

impl Application for Mailboxes {
    type Model = ScrollState;
    type Message = ScrollState;

    fn init() -> (ScrollState, Vec<Command<ScrollState>>) {
        let scroll = ScrollState {
            offset: Point::ZERO,
            content: Size::ZERO,
            viewport: Size::ZERO,
        };
        (scroll, Vec::new())
    }

    fn update(scroll: &mut ScrollState, new: ScrollState) -> Vec<Command<ScrollState>> {
        *scroll = new;
        Vec::new()
    }

    fn view(scroll: &ScrollState) -> Vec<Window<ScrollState>> {
        window(Element::from(ListView {
            label: "Mailboxes".into(),
            count: BADGES.len(),
            row_height: 24.0,
            scroll: *scroll,
            selection: Selection::default(),
            multiple: false,
            row: Box::new(|index, selected| {
                let (key, text) = BADGES[index];
                let badge = Badge {
                    text: text.map(str::to_owned),
                    description: match text {
                        Some(n) => format!("{n} unread"),
                        None => "New".into(),
                    },
                };
                Element::new(Stack {
                    axis: Axis::Horizontal,
                    padding: Insets::ZERO,
                    spacing: 8.0,
                    align: Align::Center,
                    children: vec![
                        Element::new(Text {
                            content: AttributedText::plain("Inbox", TextStyle::new(13.0)),
                            role: TextRole::Body,
                            wrap: false,
                            secondary: false,
                            selected,
                        }),
                        Element::new(badge).key(key),
                    ],
                })
            }),
            type_select: None,
            on_scroll: Box::new(|s| s),
            on_select: None,
            on_activate: None,
        }))
    }
}

#[test]
fn a_badge_is_no_node_of_its_own_and_its_description_joins_the_row_it_sits_in() {
    let h = Harness::<Mailboxes>::new();
    let nodes = h.snapshot().accessibility;
    let rows: Vec<Option<&str>> = nodes
        .iter()
        .filter(|(_, n)| n.role == Role::ListItem)
        .map(|(_, n)| n.description.as_deref())
        .collect();
    assert_eq!(rows.len(), 3);
    for description in ["3 unread", "99+ unread", "New"] {
        assert!(rows.contains(&Some(description)), "{rows:?}");
    }
}

#[test]
fn a_count_is_a_red_pill_at_least_as_wide_as_high_and_without_one_a_dot() {
    let h = Harness::<Mailboxes>::new();
    assert_eq!(
        h.rect("one").size,
        Size::new(16.0, 16.0),
        "a circle for one digit"
    );
    let many = h.rect("many").size;
    assert_eq!(many.height, 16.0);
    assert!(
        many.width > 16.0 && many.width < label_width("99+") + 10.0,
        "{many:?}"
    );
    assert_eq!(h.rect("dot").size, Size::new(4.0, 4.0));
    let fill = |key| {
        h.ops(key).into_iter().find_map(|op| match op {
            PaintOp::Fill {
                shape: Shape::RoundedRect { rect, radius },
                paint: Paint::Solid(color),
            } => Some((rect, radius, color)),
            _ => None,
        })
    };
    assert_eq!(
        fill("one"),
        Some((
            Rect::new(0.0, 0.0, 16.0, 16.0),
            8.0,
            color(
                ColorRole::DestructiveFill,
                InteractionVariant::Normal,
                false
            )
        ))
    );
    assert_eq!(
        fill("dot").map(|f| f.2),
        Some(color(
            ColorRole::SecondaryText,
            InteractionVariant::Normal,
            false
        ))
    );
}

/// Sends its message on a press and changes nothing on screen, so no repaint hides a stale accessibility tree.
struct Ping;

impl Widget<()> for Ping {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Leaf)
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(24.0, 24.0),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, ()>) -> Propagation {
        match event {
            Event::Pointer(PointerEvent::Button { pressed: true, .. }, _) => {
                cx.emit(());
                Propagation::Stop
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// A dot badge whose words change from "unread" to "mentioned" on a press; nothing else changes.
struct Mention;

impl Application for Mention {
    type Model = bool;
    type Message = ();

    fn init() -> (bool, Vec<Command<()>>) {
        (false, Vec::new())
    }

    fn update(mentioned: &mut bool, _: ()) -> Vec<Command<()>> {
        *mentioned = true;
        Vec::new()
    }

    fn view(mentioned: &bool) -> Vec<Window<()>> {
        window(Element::new(Stack {
            axis: Axis::Horizontal,
            padding: Insets::ZERO,
            spacing: 8.0,
            align: Align::Center,
            children: vec![
                Element::new(Ping).key("mention"),
                Element::new(Badge {
                    text: None,
                    description: if *mentioned { "mentioned" } else { "unread" }.into(),
                }),
            ],
        }))
    }
}

#[test]
fn a_new_description_alone_reaches_assistive_technology() {
    let mut h = Harness::<Mention>::new();
    let description = |h: &Harness<Mention>| {
        h.snapshot()
            .accessibility
            .into_iter()
            .find(|(_, n)| n.role == Role::Window)
            .and_then(|(_, n)| n.description)
    };
    assert_eq!(description(&h).as_deref(), Some("unread"));
    h.click(h.center("mention"));
    assert!(*h.model());
    assert_eq!(description(&h).as_deref(), Some("mentioned"));
}
