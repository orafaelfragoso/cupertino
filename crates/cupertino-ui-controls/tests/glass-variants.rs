//! Each control's glass is named by the system variant it stands for, so the compositor draws that variant's recipe
//! (and its inactive branch while the window is not active): the sidebar panel's abutted sidebar glass, the toolbar
//! controls' toolbar item glass with the chosen segment's pill as tab glass in their container.

use crate::support;

use cupertino_geometry::{Insets, Rect};
use cupertino_ui_controls::{
    SidebarWindow, SplitLayout, Stack, ToolbarButton, ToolbarGlyph, ToolbarGroup, ToolbarItem,
    TOOLBAR_CHOICE_INSET,
};
use cupertino_ui_core::layout::{Align, Axis};
use cupertino_ui_core::platform::{Capabilities, GlassRegion, GlassVariant, WindowId};
use cupertino_ui_core::{Element, Role};
use support::{Scene, View};

/// A window system with the desktop's effects and a raised layer.
fn effects() -> Capabilities {
    Capabilities {
        app_effects: true,
        raised_layer: true,
        ..Capabilities::default()
    }
}

/// An empty stack that fills what it is given.
fn blank<M: 'static>() -> Element<M> {
    Element::new(Stack {
        axis: Axis::Vertical,
        padding: Insets::ZERO,
        spacing: 0.0,
        align: Align::Start,
        children: Vec::new(),
    })
}

/// The glass `window` of `view` asked for last, with where it lies.
fn glass<S: Scene>(view: &mut View<S>, window: WindowId) -> Vec<(Rect, GlassRegion)> {
    view.materials(window)
        .into_iter()
        .filter_map(|m| m.glass.map(|g| (m.rect, g)))
        .collect()
}

/// The variant a glass region is named by.
fn variant(region: &GlassRegion) -> Option<GlassVariant> {
    region.variant.map(|(v, _)| v)
}

struct Sidebar;

/// Where the sidebar's divider stands.
const SIDEBAR_WIDTH: f64 = 150.0;

impl Scene for Sidebar {
    type State = ();
    type Msg = ();

    fn state() {}

    fn update(_: &mut (), _: ()) {}

    fn view(_: &()) -> Element<()> {
        Element::from(SidebarWindow {
            label: "Sidebar".into(),
            layout: SplitLayout {
                position: SIDEBAR_WIDTH,
                collapsed: None,
            },
            min_sidebar: 100.0,
            max_sidebar: 300.0,
            min_content: 100.0,
            on_change: None,
            sidebar_header: None,
            sidebar: blank(),
            content: blank(),
        })
    }
}

#[test]
fn the_sidebar_panel_is_abutted_sidebar_glass_and_opaque_with_reduce_transparency() {
    let mut view = View::<Sidebar>::open_with(effects(), false);
    let window = view.window;
    let shown = glass(&mut view, window);
    let panel = shown
        .iter()
        .find(|(_, g)| variant(g) == Some(GlassVariant::AbuttedSidebar))
        .unwrap_or_else(|| panic!("the sidebar's glass is the abutted sidebar's: {shown:?}"));
    assert_eq!(panel.0.min_x(), 0.0, "against the leading edge");
    assert!(
        panel.0.width() >= SIDEBAR_WIDTH - 1.0 && panel.0.width() <= SIDEBAR_WIDTH + 1.0,
        "as wide as the sidebar: {:?}",
        panel.0
    );

    let mut opaque = View::<Sidebar>::open_with(effects(), true);
    let window = opaque.window;
    assert!(
        glass(&mut opaque, window)
            .iter()
            .all(|(_, g)| variant(g) != Some(GlassVariant::AbuttedSidebar)),
        "with Reduce Transparency the sidebar paints its opaque fill"
    );
}

struct Toolbar;

impl Scene for Toolbar {
    type State = ();
    type Msg = ();

    fn state() {}

    fn update(_: &mut (), _: ()) {}

    fn view(_: &()) -> Element<()> {
        let item = |label: &str| ToolbarItem {
            key: None,
            glyph: ToolbarGlyph::Symbol("symbol.square.grid.2x2".into()),
            label: label.into(),
            on_press: Some(()),
            menu: None,
        };
        Element::new(Stack {
            axis: Axis::Horizontal,
            padding: Insets::uniform(20.0),
            spacing: 8.0,
            align: Align::Start,
            children: vec![
                Element::from(ToolbarGroup {
                    label: "View".into(),
                    items: vec![item("Icons"), item("List")],
                    selected: Some(1),
                    dividers: false,
                }),
                Element::from(ToolbarButton {
                    key: None,
                    glyph: ToolbarGlyph::More,
                    label: "More".into(),
                    on_press: Some(()),
                }),
            ],
        })
    }
}

#[test]
fn toolbar_controls_are_toolbar_item_glass_and_the_chosen_segment_is_tab_glass_in_their_container()
{
    let mut view = View::<Toolbar>::open_with(effects(), false);
    let window = view.window;
    let shown = glass(&mut view, window);
    let items: Vec<_> = shown
        .iter()
        .filter(|(_, g)| variant(g) == Some(GlassVariant::ToolbarItem))
        .collect();
    assert_eq!(items.len(), 2, "the group and the button: {shown:?}");
    let container = items[0].1.container;
    assert!(container != 0, "the toolbar's controls share a container");
    assert!(items.iter().all(|(_, g)| g.container == container));

    let tabs: Vec<_> = shown
        .iter()
        .filter(|(_, g)| variant(g) == Some(GlassVariant::Tab))
        .collect();
    let [(pill, tab)] = tabs[..] else {
        panic!("one pill of tab glass: {shown:?}")
    };
    assert_eq!(tab.container, container, "a shape of the same container");
    let chosen = view.bounds_of(Role::RadioButton, "List");
    let inset = chosen.inset(Insets::uniform(TOOLBAR_CHOICE_INSET));
    for (a, b) in [
        (pill.min_x(), inset.min_x()),
        (pill.min_y(), inset.min_y()),
        (pill.width(), inset.width()),
        (pill.height(), inset.height()),
    ] {
        assert!(
            (a - b).abs() < 0.5,
            "the pill {pill:?} lies inside the chosen segment, at {inset:?}"
        );
    }
}
