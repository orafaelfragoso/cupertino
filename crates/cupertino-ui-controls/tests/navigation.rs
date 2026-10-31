mod support;

use cupertino_geometry::Point;
use cupertino_ui_controls::{Navigation, NavigationStack, TextChange, TextField};
use cupertino_ui_core::{Application, Command, Element, Role, Window};
use cupertino_ui_layout::LayoutDirection;
use cupertino_ui_platform::Mods;
use cupertino_ui_render::{PaintOp, PathVerb, Shape};
use cupertino_ui_text::ContentPurpose;
use support::{window, Harness, SPACE, TAB};

const ESCAPE: u32 = 0xff1b;
const BRACKET_LEFT: u32 = 0x5b;
const BRACKET_RIGHT: u32 = 0x5d;

/// Three pages, starting on the second, each a text field so a shortcut can come from inside the page.
struct Pages;

#[derive(Clone, Debug)]
enum Message {
    Navigate(Navigation),
    Edit,
}

impl Application for Pages {
    type Model = usize;
    type Message = Message;

    fn init() -> (usize, Vec<Command<Message>>) {
        (1, Vec::new())
    }

    fn update(page: &mut usize, message: Message) -> Vec<Command<Message>> {
        match message {
            Message::Navigate(Navigation::Back) => *page -= 1,
            Message::Navigate(Navigation::Forward) => *page += 1,
            Message::Edit => {}
        }
        Vec::new()
    }

    fn view(page: &usize) -> Vec<Window<Message>> {
        window(Element::from(NavigationStack {
            content: Element::new(TextField {
                label: format!("Page {page}"),
                placeholder: String::new(),
                value: String::new(),
                revision: 0,
                purpose: ContentPurpose::Normal,
                enabled: true,
                on_change: Some(Box::new(|_: TextChange| Message::Edit)),
                on_submit: None,
            })
            .key("page"),
            can_go_back: *page > 0,
            can_go_forward: *page < 2,
            on_navigate: Some(Box::new(Message::Navigate)),
        }))
    }
}

fn button(h: &Harness<Pages>, name: &str) -> cupertino_ui_core::AccessibilityNode {
    h.snapshot()
        .accessibility
        .into_iter()
        .find(|(_, n)| n.role == Role::Button && n.name.as_deref() == Some(name))
        .expect("the button")
        .1
}

fn shortcut(h: &mut Harness<Pages>, keysym: u32) {
    h.key_event(keysym, true, false, Mods::PRIMARY);
    h.key_event(keysym, false, false, Mods::PRIMARY);
}

#[test]
fn a_toolbar_of_back_and_forward_buttons_disabled_at_the_ends_of_the_history() {
    let mut h = Harness::<Pages>::new();
    let toolbar = h
        .snapshot()
        .accessibility
        .into_iter()
        .find(|(_, n)| n.role == Role::Toolbar)
        .expect("a toolbar")
        .1;
    assert_eq!(toolbar.children.len(), 2);
    assert!(!button(&h, "Back").states.disabled);
    let back = button(&h, "Back").bounds;
    assert_eq!((back.width(), back.height()), (26.5, 28.0));
    h.click(Point::new(back.min_x() + 13.0, back.min_y() + 14.0));
    assert_eq!(*h.model(), 0);
    assert!(button(&h, "Back").states.disabled);
    assert!(!button(&h, "Back").states.focusable);
    h.tap(TAB);
    h.tap(SPACE);
    assert_eq!(*h.model(), 1, "the forward button is the first Tab stop");
}

#[test]
fn primary_brackets_go_back_and_forward_from_inside_the_page_and_escape_does_not() {
    let mut h = Harness::<Pages>::new();
    h.click(h.center("page"));
    shortcut(&mut h, BRACKET_LEFT);
    assert_eq!(*h.model(), 0);
    shortcut(&mut h, BRACKET_LEFT);
    assert_eq!(*h.model(), 0, "no page before the first");
    shortcut(&mut h, BRACKET_RIGHT);
    shortcut(&mut h, BRACKET_RIGHT);
    assert_eq!(*h.model(), 2);
    h.tap(ESCAPE);
    assert_eq!(*h.model(), 2);
}

#[test]
fn the_back_chevron_points_against_the_reading_direction() {
    let tip = |h: &Harness<Pages>| {
        h.snapshot()
            .display_list
            .nodes
            .into_iter()
            .flat_map(|n| n.ops)
            .find_map(|op| match op {
                PaintOp::Stroke {
                    shape: Shape::Path { verbs, .. },
                    ..
                } => match (verbs[0], verbs[1]) {
                    (PathVerb::MoveTo(from), PathVerb::LineTo(tip)) => Some(tip.x - from.x),
                    _ => None,
                },
                _ => None,
            })
            .expect("a chevron")
    };
    assert!(tip(&Harness::<Pages>::new()) < 0.0);
    assert!(tip(&Harness::<Pages>::with_direction(LayoutDirection::Rtl)) > 0.0);
}
