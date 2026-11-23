//! Menus as a person uses them: the keys that move between enabled items, choose, open and close submenus and type
//! to select, what a press chooses, and a pop-up button's menu opening over its chosen option.

use crate::support;

use cupertino_geometry::Insets;
use cupertino_theme::ui::Surface;
use cupertino_ui_controls::{
    Button, ButtonRole, ButtonStyle, Menu, MenuAnchor, MenuItem, MenuStyle, PopUpButton, Stack,
};
use cupertino_ui_core::layout::{Align, Axis};
use cupertino_ui_core::platform::{
    Capabilities, GlassModifiers, GlassStyle, GlassVariant, Keysym, Mods,
};
use cupertino_ui_core::{Element, Key, Role, Window, WindowBackground, WindowKey};
use support::{Scene, View, DOWN, END, ESCAPE, HOME, LEFT, RETURN, RIGHT, UP};

#[derive(Clone, Debug, PartialEq)]
enum Msg {
    Open,
    Close,
    Chosen(&'static str),
    Submenus(Vec<usize>),
    Size(usize),
}

#[derive(Default)]
struct State {
    open: bool,
    submenus: Vec<usize>,
    chosen: Vec<&'static str>,
    closed: usize,
    size: usize,
}

fn update(state: &mut State, message: Msg) {
    match message {
        Msg::Open => state.open = true,
        Msg::Close => {
            state.open = false;
            state.submenus.clear();
            state.closed += 1;
        }
        Msg::Chosen(name) => state.chosen.push(name),
        Msg::Submenus(open) => state.submenus = open,
        Msg::Size(index) => state.size = index,
    }
}

fn parent() -> WindowKey {
    WindowKey(Key::from("probe"))
}

/// A button that opens a menu of a plain item, one that cannot be chosen, a separator, an item with its keys, one
/// that is checked and a submenu.
struct Actions;

impl Scene for Actions {
    type State = State;
    type Msg = Msg;

    fn state() -> State {
        State::default()
    }

    fn update(state: &mut State, message: Msg) {
        update(state, message);
    }

    fn view(_: &State) -> Element<Msg> {
        Element::new(Stack {
            axis: Axis::Vertical,
            padding: Insets::uniform(20.0),
            spacing: 10.0,
            align: Align::Start,
            children: vec![Element::new(Button {
                label: "Actions".into(),
                role: ButtonRole::Normal,
                style: ButtonStyle::Bordered,
                enabled: true,
                busy: false,
                on_press: Some(Msg::Open),
            })
            .key("trigger")],
        })
    }

    fn popups(state: &State) -> Vec<Window<Msg>> {
        if !state.open {
            return Vec::new();
        }
        let item = |label: &str, enabled, name| MenuItem::Item {
            label: label.into(),
            enabled,
            checked: false,
            on_select: Msg::Chosen(name),
        };
        Menu {
            parent: parent(),
            anchor: MenuAnchor::Below(Key::from("trigger")),
            label: "Actions".into(),
            items: vec![
                item("New", true, "new"),
                item("Open", false, "open"),
                MenuItem::Separator,
                MenuItem::Shortcut {
                    label: "Save".into(),
                    shortcut: "⌘S".into(),
                    enabled: true,
                    checked: false,
                    on_select: Msg::Chosen("save"),
                },
                MenuItem::Submenu {
                    label: "Share".into(),
                    enabled: true,
                    items: vec![item("Mail", true, "mail"), item("Notes", true, "notes")],
                },
                item("Sort", true, "sort"),
            ],
            current: None,
            open: state.submenus.clone(),
            on_open: Box::new(Msg::Submenus),
            on_close: Msg::Close,
        }
        .windows(WindowKey(Key::from("menu")))
    }
}

/// The menu open, the keyboard in it.
fn opened() -> View<Actions> {
    let mut view = View::<Actions>::open();
    let trigger = view.center_of(Role::Button, "Actions");
    view.click(trigger, Mods::NONE);
    let popup = view.popup().expect("the menu is shown");
    view.aim(popup);
    view
}

fn chosen(view: &View<Actions>) -> Vec<&'static str> {
    view.state().chosen.clone()
}

#[test]
fn the_arrow_keys_home_and_end_move_between_the_items_that_can_be_chosen_and_return_chooses() {
    // a menu opens with no item chosen, so the first arrow goes to the first or the last; the keys pressed in a
    // fresh menu, and what Return then chooses
    let rows: [(&str, &[Keysym], &str); 9] = [
        ("return with nothing under the keyboard", &[], ""),
        ("down", &[DOWN], "new"),
        (
            "down skips an item that cannot be chosen",
            &[DOWN, DOWN],
            "save",
        ),
        ("down past the last", &[DOWN, DOWN, DOWN, DOWN, DOWN], "new"),
        ("up", &[DOWN, DOWN, UP], "new"),
        ("up from nothing", &[UP], "sort"),
        ("end", &[END], "sort"),
        ("home", &[END, HOME], "new"),
        ("down on a submenu", &[DOWN, DOWN, DOWN, DOWN], "sort"),
    ];
    for (what, keys, expected) in rows {
        let mut view = opened();
        for key in keys {
            view.key(*key, None, Mods::NONE);
        }
        view.key(RETURN, None, Mods::NONE);
        let chosen = chosen(&view);
        assert_eq!(chosen.first().copied().unwrap_or(""), expected, "{what}");
        assert_eq!(
            view.state().open,
            expected.is_empty(),
            "{what}: choosing closes the menu"
        );
    }
}

#[test]
fn a_typed_character_moves_to_the_next_item_it_starts() {
    // S starts Save, Share and Sort: each S goes on to the next of them
    let rows = [(1, "save"), (3, "sort"), (4, "save")];
    for (typed, expected) in rows {
        let mut view = opened();
        for _ in 0..typed {
            view.key(Keysym('s' as u32), Some('s'), Mods::NONE);
        }
        view.key(RETURN, None, Mods::NONE);
        assert_eq!(chosen(&view), [expected], "S typed {typed} times");
    }
}

#[test]
fn escape_closes_the_menu_without_choosing() {
    let mut view = opened();
    view.key(DOWN, None, Mods::NONE);
    view.key(ESCAPE, None, Mods::NONE);
    assert_eq!((view.state().open, view.state().closed), (false, 1));
    assert!(chosen(&view).is_empty());
}

#[test]
fn a_submenu_opens_with_the_trailing_arrow_or_return_and_closes_with_the_leading_one_or_escape() {
    let mut view = opened();
    for key in [DOWN, DOWN, DOWN, RIGHT] {
        view.key(key, None, Mods::NONE);
    }
    assert_eq!(view.state().submenus, [4], "Share is the fifth item");
    // the submenu takes the keyboard as its own popup
    let submenu = view.popup().expect("the submenu is shown");
    view.aim(submenu);
    view.key(LEFT, None, Mods::NONE);
    assert!(view.state().submenus.is_empty());
    assert_eq!((view.state().open, view.state().closed), (true, 0));

    view.key(RETURN, None, Mods::NONE);
    assert_eq!(view.state().submenus, [4], "Return opens it too");
    let submenu = view.popup().unwrap();
    view.aim(submenu);
    view.key(ESCAPE, None, Mods::NONE);
    assert!(view.state().submenus.is_empty());
    assert!(view.state().open, "Escape closes the submenu first");

    view.key(RETURN, None, Mods::NONE);
    let submenu = view.popup().unwrap();
    view.aim(submenu);
    view.key(DOWN, None, Mods::NONE);
    view.key(RETURN, None, Mods::NONE);
    assert_eq!(chosen(&view), ["notes"]);
    assert!(!view.state().open, "an item of a submenu closes every menu");
}

#[test]
fn a_press_chooses_the_item_under_it_and_does_nothing_on_one_that_cannot_be_chosen() {
    let mut view = opened();
    let open = view.center_of(Role::MenuItem, "Open");
    view.click(open, Mods::NONE);
    assert!(chosen(&view).is_empty());
    assert!(view.state().open);

    let save = view.center_of(Role::MenuItem, "Save");
    view.click(save, Mods::NONE);
    assert_eq!(chosen(&view), ["save"]);
    assert!(!view.state().open);
}

/// A pop-up button of three sizes.
struct Sizes;

impl Scene for Sizes {
    type State = State;
    type Msg = Msg;

    fn state() -> State {
        State {
            size: 1,
            ..State::default()
        }
    }

    fn update(state: &mut State, message: Msg) {
        update(state, message);
    }

    fn view(state: &State) -> Element<Msg> {
        Element::new(PopUpButton {
            label: "Size".into(),
            options: sizes(),
            selected: state.size,
            open: state.open,
            enabled: true,
            on_open: Some(Msg::Open),
        })
        .key("size")
    }

    fn popups(state: &State) -> Vec<Window<Msg>> {
        if !state.open {
            return Vec::new();
        }
        PopUpButton {
            label: "Size".into(),
            options: sizes(),
            selected: state.size,
            open: true,
            enabled: true,
            on_open: Some(Msg::Open),
        }
        .menu(parent(), Key::from("size"), Box::new(Msg::Size), Msg::Close)
        .windows(WindowKey(Key::from("menu")))
    }
}

fn sizes() -> Vec<String> {
    ["Small", "Medium", "Large"].map(String::from).to_vec()
}

#[test]
fn a_pop_up_button_opens_its_menu_on_the_chosen_option_and_choosing_another_selects_it() {
    let mut view = View::<Sizes>::open();
    let button = view.center_of(Role::PopUpButton, "Size");
    view.click(button, Mods::NONE);
    let popup = view.popup().expect("the menu is shown");
    view.aim(popup);
    assert!(view.node(Role::MenuItem, "Medium").is_some());

    view.key(DOWN, None, Mods::NONE);
    view.key(RETURN, None, Mods::NONE);
    assert_eq!(view.state().size, 2, "down from the chosen option");
    assert!(!view.state().open);

    let mut view = View::<Sizes>::open();
    let button = view.center_of(Role::PopUpButton, "Size");
    view.click(button, Mods::NONE);
    let popup = view.popup().unwrap();
    view.aim(popup);
    view.key(RETURN, None, Mods::NONE);
    assert_eq!(
        view.state().size,
        1,
        "Return at once chooses the option shown"
    );
}

/// A menu of one item.
fn one_item() -> Menu<Msg> {
    Menu {
        parent: parent(),
        anchor: MenuAnchor::Below(Key::from("trigger")),
        label: "Actions".into(),
        items: vec![MenuItem::Item {
            label: "New".into(),
            enabled: true,
            checked: false,
            on_select: Msg::Chosen("new"),
        }],
        current: None,
        open: Vec::new(),
        on_open: Box::new(Msg::Submenus),
        on_close: Msg::Close,
    }
}

/// The menu variant without its drop shadow, which the compositor draws apart for a popup.
fn is_menu_glass(variant: Option<(GlassVariant, GlassModifiers)>) -> bool {
    variant.is_some_and(|(v, m)| v == GlassVariant::Menu && m.exclude_shadow)
}

#[test]
fn a_menus_window_background_is_the_menu_variant_without_its_shadow() {
    let windows = one_item().windows(WindowKey(Key::from("menu")));
    let WindowBackground::Surface {
        surface,
        radius,
        glass,
        ..
    } = windows[0].background
    else {
        panic!("a surface background: {:?}", windows[0].background);
    };
    assert_eq!(surface, Surface::Menu, "its material and opaque fill");
    let glass = glass.expect("named glass");
    assert_eq!(
        (glass.variant, glass.exclude_shadow),
        (Some(GlassVariant::Menu), true),
        "the compositor draws the popup's shadow apart"
    );
    // a square-cornered menu keeps its corners and the same glass
    let square = one_item().windows_styled(
        WindowKey(Key::from("menu")),
        MenuStyle {
            radius: 0.0,
            ..MenuStyle::default()
        },
    );
    assert!(matches!(
        square[0].background,
        WindowBackground::Surface { radius: 0.0, glass: Some(g), .. } if g == glass
    ));
    assert!(radius > 0.0);
}

/// A button that opens a menu with a pointer.
struct Pointed;

impl Scene for Pointed {
    type State = State;
    type Msg = Msg;

    fn state() -> State {
        State::default()
    }

    fn update(state: &mut State, message: Msg) {
        update(state, message);
    }

    fn view(state: &State) -> Element<Msg> {
        Actions::view(state)
    }

    fn popups(state: &State) -> Vec<Window<Msg>> {
        if !state.open {
            return Vec::new();
        }
        one_item().windows_styled(WindowKey(Key::from("menu")), MenuStyle::pointed(30.0))
    }
}

#[test]
fn a_pointed_menus_panel_and_pointer_are_the_menu_variant_in_one_container() {
    let capabilities = Capabilities {
        app_effects: true,
        ..Capabilities::default()
    };
    let mut view = View::<Pointed>::open_with(capabilities, false);
    let trigger = view.center_of(Role::Button, "Actions");
    view.click(trigger, Mods::NONE);
    let popup = view.popup().expect("the menu is shown");
    let active = cupertino_ui_core::platform::WindowStates {
        activated: true,
        ..Default::default()
    };
    view.driver
        .window_system_mut()
        .configure(popup, None, active)
        .unwrap();
    view.settle();
    let shown: Vec<_> = view
        .materials(popup)
        .into_iter()
        .filter_map(|m| m.glass)
        .collect();
    assert!(shown.len() >= 2, "the panel and its pointer: {shown:?}");
    for g in &shown {
        assert!(is_menu_glass(g.variant), "{shown:?}");
        assert_eq!(
            g.style,
            GlassStyle::Menu,
            "for a compositor without variants"
        );
        assert_ne!(g.container, 0, "joined: {shown:?}");
    }
}
