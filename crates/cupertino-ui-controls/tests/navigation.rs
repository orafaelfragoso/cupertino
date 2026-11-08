mod support;

use cupertino_geometry::{Insets, Point, Rect, Size};
use cupertino_ui_controls::{
    Navigation, NavigationStack, ScrollView, Scroller, Spacer, Stack, TextChange, TextField,
};
use cupertino_ui_core::{Application, Command, Element, Role, Window};
use cupertino_ui_layout::{Align, Axis, LayoutDirection, ScrollState};
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
                selection: None,
                initial: false,
            })
            .key("page"),
            toolbar: Vec::new(),
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
    assert_eq!((back.width(), back.height()), (36.5, 36.0));
    h.click(Point::new(back.min_x() + 18.0, back.min_y() + 18.0));
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

/// A scrolling list under a toolbar row whose search field a spacer pushes to the end.
struct Browser;

impl Application for Browser {
    type Model = ();
    type Message = ();

    fn init() -> ((), Vec<Command<()>>) {
        ((), Vec::new())
    }

    fn update(_: &mut (), _: ()) -> Vec<Command<()>> {
        Vec::new()
    }

    fn view(_: &()) -> Vec<Window<()>> {
        let search = Element::new(TextField {
            label: "Search".into(),
            placeholder: String::new(),
            value: String::new(),
            revision: 0,
            purpose: ContentPurpose::Normal,
            enabled: true,
            on_change: None,
            on_submit: None,
            selection: None,
            initial: false,
        })
        .key("search")
        .width(120.0);
        window(Element::from(NavigationStack {
            content: Element::from(ScrollView {
                content: Element::new(Stack {
                    axis: Axis::Vertical,
                    padding: Insets::ZERO,
                    spacing: 0.0,
                    align: Align::Stretch,
                    children: Vec::new(),
                }),
                state: ScrollState {
                    offset: Point::ZERO,
                    content: Size::ZERO,
                    viewport: Size::ZERO,
                },
                horizontal: false,
                vertical: true,
                scroller: Scroller::Overlay,
                on_scroll: Box::new(|_| ()),
            })
            .key("list"),
            toolbar: vec![Element::new(Spacer { min: 0.0 }), search],
            can_go_back: false,
            can_go_forward: false,
            on_navigate: None,
        }))
    }
}

#[test]
fn the_page_fills_the_height_under_the_toolbar_row_and_a_spacer_puts_the_search_at_its_end() {
    for direction in [LayoutDirection::Ltr, LayoutDirection::Rtl] {
        let mut h = Harness::<Browser>::with_direction(direction);
        h.resize(Size::new(600.0, 300.0));
        assert_eq!(h.rect("list"), Rect::new(0.0, 52.0, 600.0, 248.0));
        let search = h.rect("search");
        assert_eq!(search.width(), 120.0);
        let tree = h.snapshot().accessibility;
        let toolbar = &tree
            .iter()
            .find(|(_, n)| n.role == Role::Toolbar)
            .expect("a toolbar")
            .1;
        let items: Vec<Option<&str>> = toolbar
            .children
            .iter()
            .map(|id| {
                let (_, child) = tree.iter().find(|(i, _)| i == id).expect("a child");
                child.name.as_deref()
            })
            .collect();
        assert_eq!(items, [Some("Back"), Some("Forward"), Some("Search")]);
        let back = tree
            .iter()
            .find(|(_, n)| n.name.as_deref() == Some("Back"))
            .expect("the back button")
            .1
            .bounds;
        assert_eq!(back.center().y, 26.0, "the group is centred in the row too");
        assert_eq!(search.center().y, 26.0, "centred in the row");
        match direction {
            LayoutDirection::Ltr => assert_eq!(search.max_x(), 592.0),
            LayoutDirection::Rtl => assert_eq!(search.min_x(), 8.0),
        }
    }
}
