use crate::support;

use cupertino_ui_controls::{Button, ButtonRole, ButtonStyle, Tabs};
use cupertino_ui_core::{AccessibilityNode, Application, Command, Element, Relation, Role, Window};
use cupertino_ui_layout::LayoutDirection;
use support::{window, Harness, RETURN, SPACE, TAB};

const LEFT: u32 = 0xff51;
const RIGHT: u32 = 0xff53;
const HOME: u32 = 0xff50;
const END: u32 = 0xff57;

const TABS: [&str; 3] = ["General", "Sharing", "Advanced"];

/// Three tabs, each with a button of its own in the panel. `FOLLOW` selects the tab each choice names; without it
/// the application only records the choices, so the arrows move the focus and leave the selection.
struct Page<const FOLLOW: bool>;
type App = Page<true>;

#[derive(Default)]
struct Model {
    selected: usize,
    said: Vec<usize>,
}

#[derive(Clone, Debug, PartialEq)]
struct Select(usize);

impl<const FOLLOW: bool> Application for Page<FOLLOW> {
    type Model = Model;
    type Message = Select;

    fn init() -> (Model, Vec<Command<Select>>) {
        (Model::default(), Vec::new())
    }

    fn update(model: &mut Model, Select(index): Select) -> Vec<Command<Select>> {
        model.said.push(index);
        if FOLLOW {
            model.selected = index;
        }
        Vec::new()
    }

    fn view(model: &Model) -> Vec<Window<Select>> {
        let name = TABS[model.selected];
        let content = Element::new(Button {
            label: format!("{name} option"),
            role: ButtonRole::Normal,
            style: ButtonStyle::Bordered,
            enabled: true,
            busy: false,
            on_press: None,
        })
        .key(name);
        window(Element::from(Tabs {
            label: "Settings".into(),
            tabs: TABS.map(String::from).into(),
            selected: model.selected,
            content,
            on_select: Some(Box::new(Select)),
        }))
    }
}

fn nodes<const F: bool>(h: &Harness<Page<F>>, role: Role) -> Vec<AccessibilityNode> {
    h.snapshot()
        .accessibility
        .into_iter()
        .map(|(_, n)| n)
        .filter(|n| n.role == role)
        .collect()
}

/// The name of the focused node.
fn focus<const F: bool>(h: &Harness<Page<F>>) -> Option<String> {
    nodes(h, Role::Tab)
        .into_iter()
        .chain(nodes(h, Role::Button))
        .find(|n| n.states.focused)
        .and_then(|n| n.name)
}

#[test]
fn the_tab_row_is_one_tab_stop_and_tab_moves_into_the_panel() {
    let mut h = Harness::<App>::new();
    h.tap(TAB);
    assert_eq!(focus(&h).as_deref(), Some("General"));
    h.tap(TAB);
    assert_eq!(focus(&h).as_deref(), Some("General option"));
    h.tap(TAB);
    assert_eq!(
        focus(&h).as_deref(),
        Some("General"),
        "the other tabs are no stops"
    );
}

#[test]
fn the_arrows_select_and_move_the_focus_and_home_and_end_reach_the_ends() {
    let mut h = Harness::<App>::new();
    h.tap(TAB);
    h.tap(RIGHT);
    assert_eq!(h.model().selected, 1);
    assert_eq!(focus(&h).as_deref(), Some("Sharing"));
    h.tap(END);
    assert_eq!(h.model().selected, 2);
    h.tap(RIGHT);
    assert_eq!(h.model().selected, 0, "the arrows wrap");
    h.tap(LEFT);
    assert_eq!(h.model().selected, 2);
    h.tap(HOME);
    assert_eq!(h.model().selected, 0);
    assert_eq!(focus(&h).as_deref(), Some("General"));
}

#[test]
fn in_right_to_left_the_left_arrow_selects_the_next_tab() {
    let mut h = Harness::<App>::with_direction(LayoutDirection::Rtl);
    h.tap(TAB);
    h.tap(LEFT);
    assert_eq!(h.model().selected, 1);
    h.tap(RIGHT);
    assert_eq!(h.model().selected, 0);
}

#[test]
fn an_arrow_only_asks_and_space_or_return_on_the_selected_tab_asks_nothing() {
    let mut h = Harness::<Page<false>>::new();
    h.tap(TAB);
    h.tap(RIGHT);
    assert_eq!(h.model().said, [1]);
    assert_eq!(
        focus(&h).as_deref(),
        Some("General"),
        "the focus stays on the selected tab, the one stop"
    );
    h.tap(SPACE);
    h.tap(RETURN);
    assert_eq!(h.model().said, [1]);
    assert_eq!(focus(&h).as_deref(), Some("General"));
}

#[test]
fn a_click_selects_a_tab() {
    let mut h = Harness::<App>::new();
    let advanced = nodes(&h, Role::Tab)
        .into_iter()
        .find(|n| n.name.as_deref() == Some("Advanced"))
        .unwrap()
        .bounds;
    h.click(advanced.center());
    assert_eq!(h.model().selected, 2);
}

#[test]
fn only_the_selected_tabs_content_is_mounted() {
    let mut h = Harness::<App>::new();
    assert!(h.rt.find(h.window, &"General".into()).is_some());
    h.tap(TAB);
    h.tap(RIGHT);
    assert!(h.rt.find(h.window, &"General".into()).is_none());
    assert!(h.rt.find(h.window, &"Sharing".into()).is_some());
    assert_eq!(
        nodes(&h, Role::Button)
            .into_iter()
            .map(|n| n.name.unwrap())
            .collect::<Vec<_>>(),
        ["Sharing option"]
    );
}

#[test]
fn assistive_technology_sees_a_tab_list_its_tabs_and_the_selected_panel() {
    let mut h = Harness::<App>::new();
    let list = nodes(&h, Role::TabList);
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].name.as_deref(), Some("Settings"));
    let tabs = nodes(&h, Role::Tab);
    assert_eq!(
        tabs.iter()
            .map(|n| (n.name.as_deref().unwrap(), n.states.selected))
            .collect::<Vec<_>>(),
        [("General", true), ("Sharing", false), ("Advanced", false)]
    );
    assert!(tabs.iter().all(|n| n.value.is_none()));
    h.tap(TAB);
    h.tap(END);
    let panel = nodes(&h, Role::TabPanel);
    assert_eq!(panel.len(), 1);
    assert_eq!(panel[0].name.as_deref(), Some("Advanced"));
    assert!(nodes(&h, Role::Tab)[2].states.selected);
}

#[test]
fn the_selected_tab_controls_the_panel() {
    let mut h = Harness::<App>::new();
    let controls = |h: &Harness<App>| {
        let tree = h.snapshot().accessibility;
        let panel = tree
            .iter()
            .find(|(_, n)| n.role == Role::TabPanel)
            .map(|&(id, _)| id)
            .unwrap();
        let tabs: Vec<_> = tree
            .into_iter()
            .filter(|(_, n)| n.role == Role::Tab)
            .map(|(_, n)| n.relations)
            .collect();
        (panel, tabs)
    };
    let (panel, tabs) = controls(&h);
    assert_eq!(tabs, [vec![Relation::Controls(panel)], vec![], vec![]]);
    h.tap(TAB);
    h.tap(END);
    let (next, tabs) = controls(&h);
    assert_ne!(next, panel, "the new tab's panel mounts fresh");
    assert_eq!(tabs, [vec![], vec![], vec![Relation::Controls(next)]]);
}
