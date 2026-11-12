//! A window in the full-height-sidebar style: the sidebar panel from the window's top edge to its bottom edge, its top
//! the window's drag region with room for the window control buttons, then the header and the sidebar's content;
//! beside it the content pane up to the window's top edge.

use crate::support;

use cupertino_geometry::{Point, Size};
use cupertino_ui_controls::{
    NavigationStack, SidebarWindow, Spacer, SplitLayout, SIDEBAR_TOP, TOOLBAR_ROW,
};
use cupertino_ui_core::{Application, Command, Element, Window};
use cupertino_ui_platform::{
    Mods, MouseButton, PointerEvent, Timestamp, WindowEvent, WindowRequest,
};
use support::{window, Harness};

const SIDEBAR: f64 = 200.0;

struct Shell;

impl Application for Shell {
    type Model = ();
    type Message = ();

    fn init() -> ((), Vec<Command<()>>) {
        ((), Vec::new())
    }

    fn update(_: &mut (), _: ()) -> Vec<Command<()>> {
        Vec::new()
    }

    fn view(_: &()) -> Vec<Window<()>> {
        let spacer = |key: &'static str, min: f64| Element::new(Spacer { min }).key(key);
        window(Element::from(SidebarWindow {
            label: "Sidebar".into(),
            layout: SplitLayout {
                position: SIDEBAR,
                collapsed: None,
            },
            min_sidebar: 100.0,
            min_content: 200.0,
            on_change: None,
            sidebar_header: Some(spacer("header", 30.0)),
            sidebar: spacer("sidebar", 0.0),
            content: Element::from(NavigationStack {
                content: spacer("page", 0.0),
                toolbar: Vec::new(),
                can_go_back: false,
                can_go_forward: false,
                on_navigate: None,
                under_toolbar: false,
            }),
        }))
    }
}

fn harness() -> Harness<Shell> {
    let mut h = Harness::<Shell>::new();
    h.resize(Size::new(700.0, 400.0));
    h
}

fn pointer(h: &mut Harness<Shell>, event: PointerEvent, time: u64) {
    h.send(WindowEvent::Pointer(event, Timestamp(time)));
}

fn button(h: &mut Harness<Shell>, position: Point, pressed: bool, time: u64) {
    pointer(
        h,
        PointerEvent::Button {
            position,
            button: MouseButton::Left,
            pressed,
            serial: 3,
            mods: Mods::NONE,
        },
        time,
    );
}

fn requests(h: &Harness<Shell>) -> Vec<WindowRequest> {
    h.rt.window_system().requests(h.window).unwrap().to_vec()
}

#[test]
fn the_sidebar_runs_the_window_height_its_header_below_the_window_control_buttons() {
    let h = harness();
    assert_eq!(SIDEBAR_TOP, TOOLBAR_ROW);
    let header = h.rect("header");
    assert_eq!((header.min_x(), header.min_y()), (0.0, SIDEBAR_TOP));
    assert_eq!(header.width(), SIDEBAR);
    let sidebar = h.rect("sidebar");
    assert_eq!(sidebar.min_y(), header.max_y());
    assert_eq!(sidebar.max_y(), 400.0, "to the window's bottom edge");
    // the content pane begins beside the sidebar, its page under the toolbar row
    let page = h.rect("page");
    assert!(page.min_x() >= SIDEBAR, "{page:?}");
    assert_eq!(page.min_y(), TOOLBAR_ROW);
    assert_eq!(page.max_x(), 700.0);
}

#[test]
fn the_top_of_the_sidebar_moves_the_window_and_a_double_click_zooms_it() {
    let mut h = harness();
    let top = Point::new(SIDEBAR - 20.0, SIDEBAR_TOP / 2.0);
    pointer(&mut h, PointerEvent::Motion { position: top }, 0);
    button(&mut h, top, true, 0);
    let to = Point::new(top.x, top.y + 30.0);
    pointer(&mut h, PointerEvent::Motion { position: to }, 1);
    button(&mut h, to, false, 2);
    pointer(&mut h, PointerEvent::Leave, 3);
    assert!(requests(&h).contains(&WindowRequest::Move));
    let at = 1_000_000_000;
    for time in [at, at + 100] {
        pointer(&mut h, PointerEvent::Motion { position: top }, time);
        button(&mut h, top, true, time);
        button(&mut h, top, false, time + 1);
    }
    assert!(
        requests(&h)
            .iter()
            .any(|r| matches!(r, WindowRequest::Maximize(true))),
        "{:?}",
        requests(&h)
    );
}

/// The solid fills over the strip beside the sidebar, the whole window high and at most two points wide.
fn strip_fills(h: &Harness<Shell>) -> Vec<cupertino_ui_theme::Color> {
    use cupertino_ui_render::{Paint, PaintOp, Shape};
    h.snapshot()
        .display_list
        .nodes
        .into_iter()
        .flat_map(|n| n.ops)
        .filter_map(|op| match op {
            PaintOp::Fill {
                shape: Shape::RoundedRect { rect: r, .. } | Shape::Rect(r),
                paint: Paint::Solid(c),
            } if r.min_x() == SIDEBAR && r.width() <= 2.0 && r.height() == 400.0 => Some(c),
            _ => None,
        })
        .collect()
}

#[test]
fn where_the_sidebar_meets_the_content_a_rim_is_drawn_not_a_separator_line() {
    use cupertino_ui_theme::{ColorRole, InteractionVariant};
    let h = harness();
    let separator = support::color(ColorRole::Separator, InteractionVariant::Normal, false);
    let strip = strip_fills(&h);
    assert!(!strip.contains(&separator), "{strip:?}");
    assert_eq!(strip.len(), 1, "only the rim: {strip:?}");
    assert!(
        strip[0].a > 0.0 && strip[0].a < 0.2,
        "a fine rim: {strip:?}"
    );
}

#[test]
fn the_divider_strip_shows_the_contents_own_background_in_light_and_dark() {
    use cupertino_ui_render::{Paint, PaintOp, Shape};
    use cupertino_ui_theme::Appearance;
    for (appearance, rim) in [(Appearance::Light, 0.0), (Appearance::Dark, 1.0)] {
        let mut theme = support::theme();
        theme.preferences.appearance = appearance;
        // without the desktop's materials the content pane paints its background from the sidebar's edge, under the
        // line, and the line adds only its translucent rim, black in the light appearance and white in the dark
        let mut h = Harness::<Shell>::with_theme(theme.clone());
        h.resize(Size::new(700.0, 400.0));
        let strip = strip_fills(&h);
        assert_eq!(strip.len(), 1, "{appearance:?}: only the rim: {strip:?}");
        let line = strip[0];
        assert!(
            (line.r, line.g, line.b) == (rim, rim, rim) && line.a > 0.0 && line.a < 0.2,
            "{appearance:?}: {line:?}"
        );
        // the pane's own fill, in its coordinates, the pane beginning at the sidebar's edge
        let content = h.snapshot().display_list.nodes.into_iter().any(|n| {
            n.bounds.min_x() == SIDEBAR
                && n.ops.iter().any(|op| {
                    matches!(op, PaintOp::Fill {
                        shape: Shape::Rect(r),
                        paint: Paint::Solid(c),
                    } if r.min_x() == 0.0 && r.width() == 700.0 - SIDEBAR && r.height() == 400.0 && c.a == 1.0)
                })
        });
        assert!(
            content,
            "{appearance:?}: the content's background runs under the line"
        );

        // with them the content's material starts at the sidebar's edge and nothing opaque covers the strip, so the
        // strip is tinted as the content is
        let mut h = Harness::<Shell>::with_effects(theme);
        h.resize(Size::new(700.0, 400.0));
        h.settle();
        let regions = requests(&h)
            .into_iter()
            .rev()
            .find_map(|r| match r {
                WindowRequest::Materials(regions) => Some(regions),
                _ => None,
            })
            .expect("the window lays materials");
        assert!(
            regions.iter().any(|m| m.rect.min_x() == SIDEBAR
                && m.rect.max_x() == 700.0
                && m.rect.max_y() == 400.0),
            "{appearance:?}: {regions:?}"
        );
        let strip = strip_fills(&h);
        assert!(
            strip.iter().all(|c| c.a < 0.2),
            "{appearance:?}: no opaque fill over the content's material: {strip:?}"
        );
    }
}

#[test]
fn the_sidebars_glass_and_the_contents_background_cover_every_device_pixel_across_the_window() {
    let mut h = Harness::<Shell>::with_effects(support::theme());
    h.resize(Size::new(700.0, 400.0));
    h.settle();
    let regions = requests(&h)
        .into_iter()
        .rev()
        .find_map(|r| match r {
            WindowRequest::Materials(regions) => Some(regions),
            _ => None,
        })
        .expect("the window lays materials");
    // what lies under a row through the middle of the window, beside the toolbar row
    let row = 300.0;
    let spans: Vec<(f64, f64)> = regions
        .iter()
        .filter(|m| m.rect.min_y() <= row && m.rect.max_y() > row)
        .map(|m| (m.rect.min_x(), m.rect.max_x()))
        .collect();
    assert!(
        spans.len() >= 2,
        "the sidebar's and the content's: {spans:?}"
    );
    // each region on device pixels by its own edges, wherever on the display the window lies; a pixel no region covers
    // has only what the window paints over it, and the desktop shows through anything short of opaque there
    for scale in [1.0, 1.25, 1.5, 1.75, 2.0] {
        for origin in [0.0, 0.25, 1.0 / 3.0, 0.5, 2.0 / 3.0, 0.75, 41.0, 41.5] {
            let px = |x: f64| ((origin + x) * scale).round() as i64;
            let (left, right) = (px(0.0), px(700.0));
            let bare: Vec<i64> = (left..right)
                .filter(|&p| !spans.iter().any(|&(a, b)| px(a) <= p && p < px(b)))
                .collect();
            assert!(
                bare.is_empty(),
                "at scale {scale}, the window at {origin}: no material under pixels {bare:?} of {left}..{right}: \
                 {spans:?}"
            );
        }
    }
}
