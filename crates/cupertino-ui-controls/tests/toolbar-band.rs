//! The band a toolbar row lays over a page scrolled under it: the compositor's material, whose blur fades out down the
//! row as macOS's scroll edge does, shown only once the page is scrolled; with Reduce Transparency the row keeps its
//! solid fill and asks for no material.

use crate::support;

use cupertino_geometry::Size;
use cupertino_ui_controls::scroll_edge::TOOLBAR_BAND_RAMP;
use cupertino_ui_controls::NavigationStack;
use cupertino_ui_core::layout::{Constraints, ContainerKind, LayoutSpec, Measured, SizeSpec};
use cupertino_ui_core::platform::{BlurRamp, Capabilities, MaterialRegion, WindowRequest};
use cupertino_ui_core::{
    Change, Element, Event, EventCx, MeasureCx, PaintCx, Propagation, SemanticsCx, Widget,
};
use support::{Scene, View};

/// A page that tells its window whether it is scrolled under the row, as a scroll view does.
struct Page {
    scrolled: bool,
}

impl<M: 'static> Widget<M> for Page {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fill,
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.scrolled != old.scrolled {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::ZERO,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        cx.set_page_scrolled(self.scrolled);
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// A page under a toolbar row, scrolled under it when `SCROLLED`.
struct Pane<const SCROLLED: bool>;

impl<const SCROLLED: bool> Scene for Pane<SCROLLED> {
    type State = ();
    type Msg = ();

    fn state() {}

    fn update(_: &mut (), _: ()) {}

    fn view(_: &()) -> Element<()> {
        Element::from(NavigationStack {
            content: Element::new(Page { scrolled: SCROLLED }),
            toolbar: Vec::new(),
            can_go_back: false,
            can_go_forward: false,
            on_navigate: None,
            under_toolbar: true,
        })
    }
}

/// A window system with the desktop's effects and a raised layer, so the row lies over the page.
fn effects() -> Capabilities {
    Capabilities {
        app_effects: true,
        raised_layer: true,
        ..Capabilities::default()
    }
}

/// The toolbar band among the materials the window asked for last, opened scrolled or not and with Reduce
/// Transparency on or off.
fn band<const SCROLLED: bool>(reduce_transparency: bool) -> Option<MaterialRegion> {
    let mut view = View::<Pane<SCROLLED>>::open_with(effects(), reduce_transparency);
    let window = view.window;
    view.driver
        .window_system_mut()
        .requests(window)
        .unwrap()
        .iter()
        .rev()
        .find_map(|r| match r {
            WindowRequest::Materials(m) => Some(m.clone()),
            _ => None,
        })
        .unwrap_or_default()
        .into_iter()
        .find(|m| m.material == "ToolbarBand")
}

#[test]
fn the_band_over_a_scrolled_page_asks_for_a_blur_that_fades_out_down_the_row() {
    assert_eq!(band::<false>(false), None, "nothing lies under the row");

    let shown = band::<true>(false).expect("the scrolled page has the row's band");
    assert_eq!(shown.blur_ramp, TOOLBAR_BAND_RAMP);
    assert!(
        shown.blur_ramp.top > shown.blur_ramp.bottom,
        "the band blurs most under the row's top: {:?}",
        shown.blur_ramp
    );
    assert!(
        shown.blur_ramp != BlurRamp::EVEN && shown.blur_ramp.bottom > 0.0,
        "a slight lift of the blur is left at the band's bottom"
    );
    assert!(
        shown.raised,
        "the band lies in the raised layer, under the row's controls"
    );
}

#[test]
fn with_reduce_transparency_the_row_asks_for_no_band() {
    assert_eq!(band::<true>(true), None);
}
