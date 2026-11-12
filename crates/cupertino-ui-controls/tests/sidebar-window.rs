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

#[test]
fn where_the_sidebar_meets_the_content_a_rim_is_drawn_not_a_separator_line() {
    use cupertino_ui_render::{Paint, PaintOp, Shape};
    use cupertino_ui_theme::{ColorRole, InteractionVariant};
    let h = harness();
    let separator = support::color(ColorRole::Separator, InteractionVariant::Normal, false);
    let background = support::color(
        ColorRole::WindowBackground,
        InteractionVariant::Normal,
        false,
    );
    // the fills of the one-point strip beside the sidebar, the whole window high
    let strip: Vec<_> = h
        .snapshot()
        .display_list
        .nodes
        .into_iter()
        .flat_map(|n| n.ops)
        .filter_map(|op| match op {
            PaintOp::Fill {
                shape: Shape::RoundedRect { rect: r, .. },
                paint: Paint::Solid(c),
            } if r.min_x() == SIDEBAR && r.width() <= 1.0 && r.height() == 400.0 => Some(c),
            _ => None,
        })
        .collect();
    assert!(!strip.contains(&separator), "{strip:?}");
    assert_eq!(strip.first(), Some(&background), "{strip:?}");
    let rim = strip.last().unwrap();
    assert!(rim.a > 0.0 && rim.a < 0.2, "a fine rim over it: {strip:?}");
}
