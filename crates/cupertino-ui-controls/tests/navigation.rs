use crate::support;

use cupertino_geometry::{Insets, Point, Rect, Size};
use cupertino_ui_controls::scroll_edge::{soft_edge_opacity, soft_edge_stops, SOFT_EDGE_STOPS};
use cupertino_ui_controls::{
    unobscured_insets, Navigation, NavigationStack, ScrollView, Scroller, Spacer, Stack,
    TextChange, TextField,
};
use cupertino_ui_core::{
    Application, Change, Command, Element, Event, EventCx, MeasureCx, PaintCx, Propagation, Role,
    SemanticsCx, Widget, Window,
};
use cupertino_ui_layout::{
    Align, Axis, Constraints, ContainerKind, LayoutDirection, LayoutSpec, Measured, ScrollState,
    SizeSpec,
};
use cupertino_ui_platform::{DecorationMode, Mods, PointerEvent, ScrollSource, WindowRequest};
use cupertino_ui_render::{Paint, PaintOp, PathVerb, Shape};
use cupertino_ui_text::ContentPurpose;
use cupertino_ui_theme::Color;
use cupertino_ui_theme::{ColorRole, InteractionVariant};
use support::{window, Harness, SPACE, TAB};

const PAGE_DOWN: u32 = 0xff56;

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
            under_toolbar: false,
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
            under_toolbar: false,
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

const ROWS: [&str; 12] = [
    "row0", "row1", "row2", "row3", "row4", "row5", "row6", "row7", "row8", "row9", "row10",
    "row11",
];

/// A 40 pt row of the list.
struct Row;

impl<M: 'static> Widget<M> for Row {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fixed(40.0),
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(0.0, 40.0),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    /// Black, so a test can tell the rows from the window background behind them.
    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let size = cx.size();
        cx.push(PaintOp::Fill {
            shape: Shape::Rect(Rect::new(0.0, 0.0, size.width, size.height)),
            paint: Paint::Solid(Color::new(0.0, 0.0, 0.0, 1.0)),
        });
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// A list that scrolls behind the toolbar row: the page fills the pane, its content starts below the row by the
/// unobscured insets of a unified window, and the model keeps the scroll state.
struct Behind;

#[derive(Clone, Debug)]
enum Scrolled {
    Scroll(ScrollState),
    Back,
}

impl Application for Behind {
    type Model = (ScrollState, usize);
    type Message = Scrolled;

    fn init() -> ((ScrollState, usize), Vec<Command<Scrolled>>) {
        (
            (
                ScrollState {
                    offset: Point::ZERO,
                    content: Size::ZERO,
                    viewport: Size::ZERO,
                },
                0,
            ),
            Vec::new(),
        )
    }

    fn update(model: &mut (ScrollState, usize), message: Scrolled) -> Vec<Command<Scrolled>> {
        match message {
            Scrolled::Scroll(state) => model.0 = state,
            Scrolled::Back => model.1 += 1,
        }
        Vec::new()
    }

    fn view(model: &(ScrollState, usize)) -> Vec<Window<Scrolled>> {
        let inset = unobscured_insets(DecorationMode::Unified);
        let rows = (0..12).map(|i| Element::new(Row).key(ROWS[i])).collect();
        window(Element::from(NavigationStack {
            content: Element::from(ScrollView {
                content: Element::new(Stack {
                    axis: Axis::Vertical,
                    padding: inset,
                    spacing: 0.0,
                    align: Align::Stretch,
                    children: rows,
                }),
                state: model.0,
                horizontal: false,
                vertical: true,
                scroller: Scroller::Overlay,
                on_scroll: Box::new(Scrolled::Scroll),
            })
            .key("list"),
            toolbar: Vec::new(),
            can_go_back: true,
            can_go_forward: false,
            on_navigate: Some(Box::new(|_| Scrolled::Back)),
            under_toolbar: true,
        }))
    }
}

#[test]
fn a_page_under_the_toolbar_lays_out_below_the_row_and_scrolls_behind_it() {
    assert_eq!(unobscured_insets(DecorationMode::Unified).top, 52.0);
    assert_eq!(unobscured_insets(DecorationMode::Sidebar).top, 52.0);
    assert_eq!(unobscured_insets(DecorationMode::UnifiedCompact).top, 40.0);
    for mode in [
        DecorationMode::Preferred,
        DecorationMode::Client,
        DecorationMode::None,
    ] {
        assert_eq!(unobscured_insets(mode), Insets::ZERO, "{mode:?}");
    }

    let mut h = Harness::<Behind>::new();
    h.resize(Size::new(600.0, 300.0));
    assert_eq!(
        h.rect("list"),
        Rect::new(0.0, 0.0, 600.0, 300.0),
        "the page fills the pane, the row's height included"
    );
    assert_eq!(h.rect("row0").min_y(), 52.0, "layout starts below the row");

    h.pointer(PointerEvent::Scroll {
        position: Point::new(300.0, 200.0),
        delta: Point::new(0.0, 30.0),
        value120: None,
        source: ScrollSource::Wheel,
        stop: false,
        inverted: (false, false),
    });
    assert_eq!(h.model().0.offset.y, 30.0);
    let row = h.rect("row0");
    assert_eq!(row.min_y(), 22.0, "the first row now lies behind the row");

    // the row over the page keeps its buttons: the back button takes the click above the scrolled content
    let back = button_named(&h, "Back");
    assert_eq!(back.center().y, 26.0);
    h.click(back.center());
    assert_eq!(
        h.model().1,
        1,
        "the back button, not the page, took the click"
    );
}

fn button_named<A: Application>(h: &Harness<A>, name: &str) -> Rect {
    h.snapshot()
        .accessibility
        .into_iter()
        .find(|(_, n)| n.role == Role::Button && n.name.as_deref() == Some(name))
        .expect("the button")
        .1
        .bounds
}

fn scrolled_by(h: &mut Harness<Behind>, dy: f64) {
    h.pointer(PointerEvent::Scroll {
        position: Point::new(300.0, 200.0),
        delta: Point::new(0.0, dy),
        value120: None,
        source: ScrollSource::Wheel,
        stop: false,
        inverted: (false, false),
    });
}

/// The rectangle fills of the window's content, in window coordinates and paint order.
fn fills(h: &Harness<Behind>) -> Vec<(Rect, Paint)> {
    let mut out = Vec::new();
    for node in h.snapshot().display_list.nodes {
        let [.., x, y] = node.transform.0;
        for op in node.ops {
            if let PaintOp::Fill {
                shape: Shape::Rect(rect),
                paint,
            } = op
            {
                out.push((rect.translate(x, y), paint));
            }
        }
    }
    out
}

/// The grey level of the window's last frame, with its raised layer over it, at `(x, y)` (one byte of four: the
/// fixture's colours are greys).
fn level(h: &Harness<Behind>, x: usize, y: usize) -> u8 {
    let system = h.rt.window_system();
    let width = system.frames(h.window).unwrap().last().unwrap().width as usize;
    let pixels = system.composited(h.window).unwrap().expect("a frame");
    pixels[(y * width + x) * 4 + 1]
}

fn window_background() -> Color {
    support::color(
        ColorRole::WindowBackground,
        InteractionVariant::Normal,
        false,
    )
}

/// Where the desktop lays no material under the row, the page's own soft veil sets the row off.
#[test]
fn over_a_raised_layer_the_page_shows_under_the_row_through_a_soft_edge() {
    let mut h = Harness::<Behind>::with_layer_only(support::theme());
    h.resize(Size::new(600.0, 300.0));
    scrolled_by(&mut h, 30.0);
    let layers = h.rt.window_system().layers(h.window).unwrap();
    assert!(
        // the shade under the row's glass reaches a few points past it
        matches!(layers.last(), Some(Some(layer)) if (52..=64).contains(&layer.height)),
        "the row draws in a layer down to its bottom and its shade: {:?}",
        layers
            .iter()
            .map(|l| l.as_ref().map(|l| l.height))
            .collect::<Vec<_>>()
    );

    // the edge veils the band the row covers, after the rows: over them
    let fills = fills(&h);
    let (at, edge) = fills
        .iter()
        .enumerate()
        .find_map(|(i, (rect, paint))| match paint {
            Paint::Linear { stops, .. } if stops.len() == SOFT_EDGE_STOPS => {
                Some((i, (rect, stops)))
            }
            _ => None,
        })
        .expect("a soft edge");
    assert_eq!(*edge.0, Rect::new(0.0, 0.0, 600.0, 52.0));
    let background = window_background();
    assert_eq!(*edge.1, soft_edge_stops(background));
    let last_row = fills
        .iter()
        .rposition(|(_, p)| *p == Paint::Solid(Color::new(0.0, 0.0, 0.0, 1.0)))
        .unwrap();
    assert!(at > last_row, "the edge draws over the rows");

    // the rows show under the row's band, more the further down it: veiled at its top, plain below it
    let bg = (background.g * 255.0).round() as u8;
    let top = level(&h, 300, 2);
    let low = level(&h, 300, 48);
    assert!(top > 200 && top <= bg, "veiled at the top: {top}");
    // the veil thins down the band but still holds most of its strength at the bottom
    let thinned = (soft_edge_opacity(48.5, 52.0) * f64::from(bg)).round() as u8;
    assert!(
        low < top && low.abs_diff(thinned) <= 2,
        "thinner near the bottom: {low}, near {thinned}"
    );
    assert_eq!(level(&h, 300, 60), 0, "plain below the band");
    // the gradient is dithered by position: a pixel may sit a step either side of its value
    let mut last = top;
    for y in 2..52 {
        let l = level(&h, 300, y);
        assert!(l <= last.saturating_add(2), "at {y}: {l} after {last}");
        last = l.min(last);
    }

    // scrolling draws the page again and leaves the row's layer as it was
    let (frames, layers) = {
        let system = h.rt.window_system();
        (
            system.frames(h.window).unwrap().len(),
            system.layers(h.window).unwrap().len(),
        )
    };
    scrolled_by(&mut h, 10.0);
    let system = h.rt.window_system();
    assert!(system.frames(h.window).unwrap().len() > frames);
    assert_eq!(
        system.layers(h.window).unwrap().len(),
        layers,
        "the layer is not drawn again"
    );
}

/// Where the desktop lays the material, the row lays a band of it over the page once the page is scrolled, and the
/// page leaves its own veil out and keeps only the hairline along the band's bottom.
#[test]
fn with_the_material_the_row_lays_its_band_and_the_page_keeps_only_its_hairline() {
    let mut h = Harness::<Behind>::with_layer(support::theme());
    h.resize(Size::new(600.0, 300.0));
    let veiled = |h: &Harness<Behind>| {
        fills(h).iter().any(
            |(_, p)| matches!(p, Paint::Linear { stops, .. } if stops.len() == SOFT_EDGE_STOPS),
        )
    };
    let band = |h: &Harness<Behind>| {
        h.rt.window_system()
            .requests(h.window)
            .unwrap()
            .iter()
            .rev()
            .find_map(|r| match r {
                WindowRequest::Materials(regions) => Some(
                    regions
                        .iter()
                        .any(|m| m.raised && m.rect.min_y() == 0.0 && m.rect.width() == 600.0),
                ),
                _ => None,
            })
            .unwrap_or(false)
    };
    assert!(
        !band(&h),
        "nothing lies under the row before the page scrolls"
    );
    scrolled_by(&mut h, 30.0);
    assert!(band(&h), "the row lays its band once the page is scrolled");
    assert!(!veiled(&h), "the band carries the veil");
    assert!(
        fills(&h).iter().any(|(r, _)| r.height() <= 1.0
            && r.width() == 600.0
            && (51.0..=52.0).contains(&r.min_y())),
        "the hairline stays along the band's bottom"
    );
}

/// In a window that is not active the row recedes into a solid band of the inactive title bar colour, in place of
/// the veil: the content stops at the row as under a solid title bar, a little lighter than the window in dark.
#[test]
fn an_inactive_window_sets_its_row_on_the_inactive_title_bar_band() {
    let mut h = Harness::<Behind>::with_layer_only(support::theme());
    h.resize(Size::new(600.0, 300.0));
    scrolled_by(&mut h, 30.0);
    h.send(cupertino_ui_platform::WindowEvent::Focus(false));
    let band = support::color(
        ColorRole::InactiveTitlebar,
        InteractionVariant::Normal,
        false,
    );
    let drawn = fills(&h);
    assert!(
        !drawn.iter().any(
            |(_, p)| matches!(p, Paint::Linear { stops, .. } if stops.len() == SOFT_EDGE_STOPS)
        ),
        "no veil while the window is not active"
    );
    assert!(
        drawn
            .iter()
            .any(|(rect, paint)| *rect == Rect::new(0.0, 0.0, 600.0, 52.0)
                && *paint == Paint::Solid(band)),
        "the band is the inactive title bar colour"
    );
    let level_of = (band.g * 255.0).round() as u8;
    for y in [2, 30, 51] {
        assert_eq!(level(&h, 300, y), level_of, "solid at {y}");
    }
    assert_eq!(level(&h, 300, 60), 0, "plain below the band");
    let background = window_background();
    assert_ne!(band, background, "the band stands apart from the window");
    // active again, the veil returns
    h.send(cupertino_ui_platform::WindowEvent::Focus(true));
    assert!(fills(&h)
        .iter()
        .any(|(_, p)| matches!(p, Paint::Linear { stops, .. } if stops.len() == SOFT_EDGE_STOPS)));
}

/// With Reduce Transparency an inactive window's band is the inactive title bar colour, with the hard edge's separator.
#[test]
fn an_inactive_window_with_reduce_transparency_keeps_the_separator_under_its_band() {
    let mut rt = support::theme();
    rt.preferences.reduce_transparency = true;
    let mut h = Harness::<Behind>::with_layer(rt);
    h.resize(Size::new(600.0, 300.0));
    scrolled_by(&mut h, 30.0);
    h.send(cupertino_ui_platform::WindowEvent::Focus(false));
    let band = support::color(
        ColorRole::InactiveTitlebar,
        InteractionVariant::Normal,
        false,
    );
    let separator = support::color(ColorRole::Separator, InteractionVariant::Normal, false);
    assert_eq!(level(&h, 300, 30), (band.g * 255.0).round() as u8);
    assert_eq!(level(&h, 300, 51), (separator.g * 255.0).round() as u8);
}

#[test]
fn reduce_transparency_sets_the_row_on_a_solid_band() {
    let mut rt = support::theme();
    rt.preferences.reduce_transparency = true;
    let background = window_background();
    let bg = (background.g * 255.0).round() as u8;
    let separator = support::color(ColorRole::Separator, InteractionVariant::Normal, false);
    let sep = (separator.g * 255.0).round() as u8;
    let mut h = Harness::<Behind>::with_layer(rt);
    h.resize(Size::new(600.0, 300.0));
    scrolled_by(&mut h, 30.0);
    let fills = fills(&h);
    assert!(
        fills
            .iter()
            .any(|(rect, paint)| *rect == Rect::new(0.0, 0.0, 600.0, 52.0)
                && *paint == Paint::Solid(background)),
        "the band is filled"
    );
    assert!(!fills
        .iter()
        .any(|(_, p)| matches!(p, Paint::Linear { stops, .. } if stops.len() == SOFT_EDGE_STOPS)));
    for y in [2, 30, 50] {
        assert_eq!(level(&h, 300, y), bg, "solid at {y}");
    }
    assert_eq!(level(&h, 300, 51), sep, "a separator along its bottom");
    assert_eq!(level(&h, 300, 60), 0);
}

#[test]
fn without_a_layer_nothing_scrolls_under_the_row_and_its_glass_stays_under_the_window() {
    let mut h = Harness::<Behind>::with_effects(support::theme());
    h.resize(Size::new(600.0, 300.0));
    scrolled_by(&mut h, 30.0);
    assert_eq!(h.model().0.offset.y, 30.0);
    assert!(
        !fills(&h)
            .iter()
            .any(|(_, p)| matches!(p, Paint::Linear { .. })),
        "no edge: nothing passes under the row"
    );
    // the rows are cut at the row's bottom edge
    let row0 = h.rect("row0");
    assert_eq!(
        row0.min_y(),
        22.0,
        "the first row lies partly behind the row"
    );
    let id = cupertino_ui_render::PaintNodeId::from(h.rt.find(h.window, &"row0".into()).unwrap());
    let drawn = h
        .snapshot()
        .display_list
        .nodes
        .into_iter()
        .find(|n| n.id == id)
        .unwrap();
    // the bounds take in the anti-aliased edge of the clip
    assert!(
        drawn.bounds.min_y() >= 51.0,
        "drawn only below the row: {:?}",
        drawn.bounds
    );
    assert_eq!(
        level(&h, 300, 40),
        level(&h, 300, 2),
        "none of it shows in the row"
    );
    assert_eq!(level(&h, 300, 60), 0);
    let system = h.rt.window_system();
    assert!(system.layers(h.window).unwrap().is_empty(), "no layer");
    // the back button's glass is asked for as before, under the window
    let regions = system
        .requests(h.window)
        .unwrap()
        .iter()
        .rev()
        .find_map(|r| match r {
            cupertino_ui_platform::WindowRequest::Materials(regions) => Some(regions.clone()),
            _ => None,
        })
        .expect("the window asks for materials");
    assert!(
        regions
            .iter()
            .any(|r| r.glass.is_some() && !r.raised && r.rect.max_y() <= 52.0),
        "{regions:?}"
    );
}

#[test]
fn the_scroll_bar_paging_and_the_focus_ring_keep_below_the_row() {
    let mut h = Harness::<Behind>::with_layer(support::theme());
    h.resize(Size::new(600.0, 300.0));
    // past the back button, the view takes the focus: nothing in it is a Tab stop
    h.tap(TAB);
    h.tap(TAB);
    assert!(h.focused("list") || h.snapshot().focus.is_some());
    h.tap(PAGE_DOWN);
    assert_eq!(
        h.model().0.offset.y,
        300.0 - 52.0 - 40.0,
        "a page is what shows below the row, less a line"
    );

    // the thumb of the overlay bar, shown by the paging, lies in the part the row leaves
    let thumb = support::color(ColorRole::ScrollerThumb, InteractionVariant::Normal, false);
    let thumbs: Vec<Rect> = fills(&h)
        .into_iter()
        .chain(rounded(&h))
        .filter_map(|(rect, paint)| match paint {
            Paint::Solid(c) if c.r == thumb.r && c.a > 0.0 => Some(rect),
            _ => None,
        })
        .collect();
    assert!(!thumbs.is_empty(), "the bar shows");
    for t in thumbs {
        assert!(t.min_y() >= 52.0, "the thumb {t:?} is below the row");
        assert!(t.max_y() <= 300.0);
    }
}

/// The rounded-rectangle fills of the window's content, as their bounds in window coordinates.
fn rounded(h: &Harness<Behind>) -> Vec<(Rect, Paint)> {
    let mut out = Vec::new();
    for node in h.snapshot().display_list.nodes {
        let [.., x, y] = node.transform.0;
        for op in node.ops {
            if let PaintOp::Fill {
                shape: Shape::RoundedRect { rect, .. },
                paint,
            } = op
            {
                out.push((rect.translate(x, y), paint));
            }
        }
    }
    out
}
