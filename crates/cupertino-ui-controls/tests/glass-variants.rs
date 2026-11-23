//! Each control's glass is named by the system variant it stands for, so the compositor draws that variant's recipe
//! (and its inactive branch while the window is not active): the sidebar panel's abutted sidebar glass.

use crate::support;

use cupertino_geometry::{Insets, Rect};
use cupertino_ui_controls::{SidebarWindow, SplitLayout, Stack};
use cupertino_ui_core::layout::{Align, Axis};
use cupertino_ui_core::platform::{Capabilities, GlassRegion, GlassVariant, WindowId};
use cupertino_ui_core::Element;
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
