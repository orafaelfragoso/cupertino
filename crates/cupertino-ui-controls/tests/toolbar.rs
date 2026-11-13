use crate::support;

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
            under_toolbar: false,
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
    // the round button's glass, and over the resting field a second
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
    // in the light appearance each glass has a shade under its body: two fills for each; the field under the resting
    // button is unseen, so nothing needs to hide it
    assert_eq!(round(&h), 4);
}

/// The edges of the toolbar's glass the toolkit draws over the compositor's: each rim's stops, top to bottom, and how
/// many shades are cut out of their shapes.
fn glass_edges(h: &Harness<Browser>) -> (Vec<Vec<cupertino_ui_theme::Color>>, usize) {
    use cupertino_ui_render::{Effect, FillRule, Paint};
    let ops: Vec<PaintOp> = h
        .snapshot()
        .display_list
        .nodes
        .into_iter()
        .flat_map(|n| n.ops)
        .collect();
    let rims = ops
        .iter()
        .filter_map(|op| match op {
            PaintOp::Stroke {
                paint: Paint::Linear { stops, .. },
                ..
            } => Some(stops.iter().map(|s| s.color).collect()),
            _ => None,
        })
        .collect();
    let shades = ops
        .windows(2)
        .filter(|w| {
            matches!(
                &w[0],
                PaintOp::PushClip(Shape::Path {
                    fill_rule: FillRule::EvenOdd,
                    ..
                })
            ) && matches!(&w[1], PaintOp::PushLayer { effects, .. }
                    if matches!(effects.as_slice(), [Effect::DropShadow { .. }]))
        })
        .count();
    (rims, shades)
}

/// The toolkit outlines each toolbar control's glass with a rim on its outermost device pixel. In the light appearance
/// it is a grey line, stronger at the sides than at the top and bottom, with a faint shade below the control that never
/// dims the glass; Increase Contrast makes the rim firmer. In the dark appearance it is a dark line along the sides
/// only, leaving the top and bottom to the glass's line of light, and there is no shade.
#[test]
fn the_toolbar_glass_gets_an_outline_of_the_toolkit() {
    use cupertino_ui_theme::Appearance;
    let rim = |appearance| {
        let mut theme = support::theme();
        theme.preferences.appearance = appearance;
        let mut h = Harness::<Browser>::with_effects(theme);
        h.settle();
        // every control lies on toolbar glass, joined in one container
        let glass: Vec<_> = h
            .rt
            .window_system()
            .requests(h.window)
            .unwrap()
            .iter()
            .rev()
            .find_map(|request| match request {
                cupertino_ui_platform::WindowRequest::Materials(regions) => Some(regions.clone()),
                _ => None,
            })
            .unwrap_or_default()
            .into_iter()
            .filter_map(|r| r.glass)
            .collect();
        assert!(!glass.is_empty());
        assert!(
            glass
                .iter()
                .all(|g| g.style == cupertino_ui_platform::GlassStyle::Toolbar
                    && g.container == glass[0].container),
            "{glass:?}"
        );
        let (rims, shades) = glass_edges(&h);
        // the choice, the menu button, the action group, the round button, and the resting search with the field
        // under it
        assert_eq!(rims.len(), 6, "{appearance:?} {rims:?}");
        let alphas: Vec<(f32, f32)> = rims
            .iter()
            .map(|stops| {
                let [top, middle, bottom] = stops.as_slice() else {
                    panic!("{stops:?}")
                };
                for c in [top, middle, bottom] {
                    assert_eq!((c.r, c.g, c.b), (0.0, 0.0, 0.0), "a dark rim");
                }
                assert_eq!(top.a, bottom.a, "{stops:?}");
                (middle.a, top.a)
            })
            .collect();
        assert!(alphas.windows(2).all(|w| w[0] == w[1]), "{alphas:?}");
        (alphas[0], shades)
    };
    // over a face of 245 the sides come out near 181 and the top and bottom near 228
    let ((sides, ends), shades) = rim(Appearance::Light);
    assert_eq!((sides, ends), (0.26, 0.07));
    assert_eq!(shades, 6, "each casts its shade, cut out of its shape");
    let darkened = |a: f32| (245.0 * (1.0 - a)).round();
    assert!((darkened(sides) - 181.0).abs() <= 1.0 && (darkened(ends) - 228.0).abs() <= 1.0);

    assert_eq!(
        rim(Appearance::LightHighContrast),
        ((0.5, 0.3), 6),
        "firmer"
    );
    assert_eq!(
        rim(Appearance::Dark),
        ((0.6, 0.0), 0),
        "the sides only, no shade"
    );
}
