mod support;

use cupertino_ui_controls::{
    nested_radius, NavigationStack, Spacer, TextField, ToolbarButton, ToolbarGlyph, ToolbarGroup,
    ToolbarItem, ToolbarMenuButton, ToolbarSearch, ToolbarTitle, TOOLBAR_CHOICE_INSET,
    TOOLBAR_CHOICE_SEGMENT, TOOLBAR_CONTROL, TOOLBAR_EDGE, TOOLBAR_GAP, TOOLBAR_MENU_BUTTON,
    TOOLBAR_ROW, TOOLBAR_SEGMENT,
};
use cupertino_ui_core::{
    AccessibleValue, Application, Checked, Command, Element, Key, Role, Window,
};
use cupertino_ui_render::{PaintOp, Shape};
use cupertino_ui_text::ContentPurpose;
use support::{window, Harness};

/// A browser's toolbar: the title, a choice of three views, a menu button, a group of two actions and the search.
struct Browser;

#[derive(Clone, Debug, PartialEq)]
enum Message {
    View(usize),
    Menu,
    Share,
    Search,
}

#[derive(Default)]
struct Model {
    view: usize,
    sent: Vec<Message>,
    collapsed: bool,
}

fn item(
    key: &'static str,
    symbol: &str,
    label: &str,
    message: Option<Message>,
) -> ToolbarItem<Message> {
    ToolbarItem {
        key: Some(Key::from(key)),
        glyph: ToolbarGlyph::Symbol(symbol.into()),
        label: label.into(),
        on_press: message,
        menu: None,
    }
}

impl Application for Browser {
    type Model = Model;
    type Message = Message;

    fn init() -> (Model, Vec<Command<Message>>) {
        (
            Model {
                collapsed: true,
                ..Model::default()
            },
            Vec::new(),
        )
    }

    fn update(model: &mut Model, message: Message) -> Vec<Command<Message>> {
        if let Message::View(view) = message {
            model.view = view;
        }
        model.sent.push(message);
        Vec::new()
    }

    fn view(model: &Model) -> Vec<Window<Message>> {
        let field = Element::new(TextField {
            label: "Search".into(),
            placeholder: "Search".into(),
            value: String::new(),
            revision: 0,
            purpose: ContentPurpose::Normal,
            enabled: true,
            on_change: None,
            on_submit: Some(Message::Search),
            selection: None,
            initial: false,
        })
        .key("field");
        window(Element::from(NavigationStack {
            content: Element::new(Spacer { min: 0.0 }).key("page"),
            toolbar: vec![
                Element::new(ToolbarTitle {
                    text: "Documents".into(),
                })
                .key("title"),
                Element::from(ToolbarGroup {
                    label: "View".into(),
                    items: vec![
                        item("icons", "symbol.grid", "Icons", Some(Message::View(0))),
                        item("list", "symbol.list", "List", Some(Message::View(1))),
                        item("columns", "symbol.columns", "Columns", None),
                    ],
                    selected: Some(model.view),
                    dividers: true,
                }),
                Element::from(ToolbarMenuButton {
                    key: Some(Key::from("menu")),
                    symbol: "symbol.grid".into(),
                    label: "Group".into(),
                    on_press: Some(Message::Menu),
                    expanded: false,
                }),
                Element::from(ToolbarGroup {
                    label: String::new(),
                    items: vec![
                        item("share", "symbol.share", "Share", Some(Message::Share)),
                        ToolbarItem {
                            glyph: ToolbarGlyph::More,
                            ..item("more", "", "More", None)
                        },
                    ],
                    selected: None,
                    dividers: false,
                }),
                Element::from(ToolbarButton {
                    key: Some(Key::from("circle")),
                    glyph: ToolbarGlyph::Symbol("symbol.magnifyingglass".into()),
                    label: "Find".into(),
                    on_press: Some(Message::Search),
                }),
                Element::from(ToolbarSearch {
                    field,
                    collapsed: model.collapsed,
                })
                .key("search")
                .width(TOOLBAR_CONTROL),
            ],
            can_go_back: false,
            can_go_forward: false,
            on_navigate: None,
        }))
    }
}

#[test]
fn the_controls_lie_in_the_middle_of_the_row_at_their_widths() {
    let h = Harness::<Browser>::new();
    let mid = TOOLBAR_ROW / 2.0;
    for (key, width) in [
        ("icons", TOOLBAR_CHOICE_SEGMENT),
        ("share", TOOLBAR_SEGMENT),
        ("menu", TOOLBAR_MENU_BUTTON),
        ("circle", TOOLBAR_CONTROL),
        ("search", TOOLBAR_CONTROL),
    ] {
        let rect = h.rect(key);
        assert_eq!(rect.width(), width, "{key}");
        assert_eq!(rect.height(), TOOLBAR_CONTROL, "{key}");
        assert_eq!(rect.min_y() + rect.height() / 2.0, mid, "{key}");
    }
    let circle = h.rect("circle");
    let search = h.rect("search");
    assert_eq!(search.min_x() - circle.max_x(), TOOLBAR_GAP);
    assert_eq!(
        h.rect("list").min_x() - h.rect("icons").min_x(),
        TOOLBAR_CHOICE_SEGMENT
    );
    let back = h
        .snapshot()
        .accessibility
        .into_iter()
        .find(|(_, n)| n.name.as_deref() == Some("Back"))
        .expect("the back button")
        .1
        .bounds;
    assert_eq!(back.min_x(), TOOLBAR_EDGE);
}

#[test]
fn a_choice_is_a_radio_group_whose_chosen_pill_nests_inside_the_capsule() {
    let mut h = Harness::<Browser>::new();
    let group = h
        .snapshot()
        .accessibility
        .into_iter()
        .find(|(_, n)| n.role == Role::RadioGroup)
        .expect("a radio group")
        .1;
    assert_eq!(group.name.as_deref(), Some("View"));
    assert!(h.accessible("icons").states.selected);
    let on = |h: &Harness<Browser>, key| h.accessible(key).value;
    assert_eq!(
        on(&h, "icons"),
        Some(AccessibleValue::Toggle(Checked::True))
    );
    assert_eq!(
        on(&h, "list"),
        Some(AccessibleValue::Toggle(Checked::False))
    );
    assert!(h.accessible("columns").states.disabled);
    h.click(h.center("list"));
    assert_eq!(h.model().view, 1);
    assert!(h.accessible("list").states.selected);
    // the pill: inset from the capsule, its corners the capsule's less the inset
    let pill = nested_radius(TOOLBAR_CONTROL / 2.0, TOOLBAR_CHOICE_INSET);
    let list = h.rect("list");
    let found = h
        .snapshot()
        .display_list
        .nodes
        .into_iter()
        .flat_map(|n| n.ops)
        .any(|op| {
            matches!(op, PaintOp::Fill { shape: Shape::RoundedRect { rect, radius }, .. }
            if radius == pill && rect.width() == list.width() - 2.0 * TOOLBAR_CHOICE_INSET)
        });
    assert!(found, "a pill of radius {pill}");
}

#[test]
fn a_disabled_segment_sends_nothing_and_an_action_group_has_no_group_of_its_own() {
    let mut h = Harness::<Browser>::new();
    h.click(h.center("columns"));
    h.click(h.center("more"));
    assert!(h.model().sent.is_empty());
    h.click(h.center("share"));
    h.click(h.center("menu"));
    h.click(h.center("circle"));
    assert_eq!(
        h.model().sent,
        [Message::Share, Message::Menu, Message::Search]
    );
    assert_eq!(h.accessible("menu").role, Role::PopUpButton);
    let groups = h
        .snapshot()
        .accessibility
        .into_iter()
        .filter(|(_, n)| n.role == Role::Group)
        .count();
    assert_eq!(groups, 0);
}

#[test]
fn the_resting_search_is_a_round_button_whose_press_reaches_the_field() {
    let mut h = Harness::<Browser>::new();
    h.click(h.center("search"));
    assert!(
        h.focused("field"),
        "the field under the button takes the keyboard"
    );
    assert_eq!(h.accessible("field").role, Role::TextInput);
    // the round button's glass, and over the resting field a second, its background under it
    let round = |h: &Harness<Browser>| {
        h.snapshot()
            .display_list
            .nodes
            .into_iter()
            .flat_map(|n| n.ops)
            .filter(|op| {
                matches!(op, PaintOp::Fill { shape: Shape::RoundedRect { rect, radius }, .. }
                    if *radius == TOOLBAR_CONTROL / 2.0 && rect.width() == TOOLBAR_CONTROL)
            })
            .count()
    };
    // in the light appearance each glass has a shade under its body: two fills for the round search button, three
    // for the resting field's (its background, the shade, the body)
    assert_eq!(round(&h), 5);
}
