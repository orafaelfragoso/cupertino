//! The frame's geometry and what lies under the pointer: layout per titlebar style, hit-testing with its priorities,
//! the resize zones and the double-click action.

use cupertino_decorations::chrome::Direction;
use cupertino_decorations::layout::title_slot;
use cupertino_decorations::{
    resize_hit, ButtonKind, DecorationMetrics, DoubleClickAction, FrameLayout, Hit, ResizeEdge,
    TitleAlignment, TitlebarStyle,
};
use cupertino_geometry::{Point, Rect, Size};

const CONTENT: Size = Size {
    width: 400.0,
    height: 300.0,
};
const RADIUS: f64 = 12.0;

fn layout(style: TitlebarStyle) -> FrameLayout {
    FrameLayout::new(CONTENT, style, &DecorationMetrics::default(), RADIUS)
}

fn hit(layout: &FrameLayout, x: f64, y: f64, resizable: bool) -> Hit {
    cupertino_decorations::hit::hit_test(
        layout,
        &DecorationMetrics::default(),
        Point::new(x, y),
        resizable,
    )
}

#[test]
fn a_titled_frame_adds_the_title_bar_above_the_content_and_the_other_styles_do_not() {
    let metrics = DecorationMetrics::default();
    for style in TitlebarStyle::ALL {
        let l = layout(style);
        let reserved = metrics.titlebar_reserved(style);
        assert_eq!(
            l.frame.size,
            Size::new(400.0, 300.0 + reserved),
            "{style:?}"
        );
        assert_eq!(l.content.min_y(), reserved, "{style:?}");
        assert_eq!(
            l.titlebar.height(),
            metrics.titlebar_height(style),
            "{style:?}"
        );
        assert_eq!(style.client_drawn(), reserved == 0.0, "{style:?}");

        let [close, minimize, zoom] = l.buttons;
        assert!(close.center.x < minimize.center.x && minimize.center.x < zoom.center.x);
        for b in l.buttons {
            assert_eq!(
                b.center.y,
                l.titlebar.height() / 2.0,
                "{style:?} centred in the row"
            );
            assert!(l.titlebar.contains(b.center));
            assert!(l.button_group.contains(b.center));
        }
        let from_frame = FrameLayout::for_frame(l.frame.size, style, &metrics, RADIUS);
        assert_eq!(
            from_frame.content, l.content,
            "{style:?} frame and content sizes agree"
        );
    }
}

#[test]
fn a_right_to_left_window_mirrors_its_buttons() {
    let mut metrics = DecorationMetrics::default();
    let ltr = FrameLayout::new(CONTENT, TitlebarStyle::Standard, &metrics, RADIUS);
    metrics.direction = Direction::RightToLeft;
    let rtl = FrameLayout::new(CONTENT, TitlebarStyle::Standard, &metrics, RADIUS);
    for kind in ButtonKind::ALL {
        assert_eq!(
            rtl.button(kind).center.x,
            CONTENT.width - ltr.button(kind).center.x
        );
    }
    assert_eq!(rtl.group_reach(), ltr.group_reach());
}

#[test]
fn the_pointer_hits_what_lies_under_it() {
    let standard = layout(TitlebarStyle::Standard);
    let unified = layout(TitlebarStyle::Unified);
    let frame_h = standard.frame.height();
    let close = standard.button(ButtonKind::Close).center;
    let zoom = unified.button(ButtonKind::Zoom).center;
    // (what, layout, x, y, resizable, expected)
    let cases = [
        (
            "close button",
            &standard,
            close.x,
            close.y,
            true,
            Hit::Button(ButtonKind::Close),
        ),
        ("title bar", &standard, 200.0, 16.0, true, Hit::Titlebar),
        ("content", &standard, 200.0, 150.0, true, Hit::Content),
        (
            "left band outside the frame",
            &standard,
            -2.0,
            150.0,
            true,
            Hit::Resize(ResizeEdge::Left),
        ),
        (
            "right band inside the frame",
            &standard,
            399.0,
            150.0,
            true,
            Hit::Resize(ResizeEdge::Right),
        ),
        (
            "top band",
            &standard,
            200.0,
            -2.0,
            true,
            Hit::Resize(ResizeEdge::Top),
        ),
        (
            "bottom band",
            &standard,
            200.0,
            frame_h + 2.0,
            true,
            Hit::Resize(ResizeEdge::Bottom),
        ),
        (
            "top-left corner",
            &standard,
            -2.0,
            -2.0,
            true,
            Hit::Resize(ResizeEdge::TopLeft),
        ),
        (
            "left band near the top",
            &standard,
            -2.0,
            8.0,
            true,
            Hit::Resize(ResizeEdge::TopLeft),
        ),
        (
            "bottom-right corner",
            &standard,
            402.0,
            frame_h + 2.0,
            true,
            Hit::Resize(ResizeEdge::BottomRight),
        ),
        ("beyond the band", &standard, -10.0, 150.0, true, Hit::None),
        (
            "band of a fixed-size window",
            &standard,
            -2.0,
            150.0,
            false,
            Hit::None,
        ),
        (
            "title bar of a fixed-size window",
            &standard,
            200.0,
            1.0,
            false,
            Hit::Titlebar,
        ),
        (
            "a unified bar belongs to the client",
            &unified,
            200.0,
            20.0,
            true,
            Hit::Content,
        ),
        (
            "buttons of a unified bar",
            &unified,
            zoom.x,
            zoom.y,
            true,
            Hit::Button(ButtonKind::Zoom),
        ),
    ];
    for (what, layout, x, y, resizable, expected) in cases {
        assert_eq!(hit(layout, x, y, resizable), expected, "{what}");
    }
}

#[test]
fn the_input_region_and_the_button_group_follow_the_layout() {
    let metrics = DecorationMetrics::default();
    let l = layout(TitlebarStyle::Standard);
    let region = cupertino_decorations::hit::input_region(&l, &metrics);
    assert_eq!(region, l.frame.outset(metrics.resize_outset));
    let inside = l.button(ButtonKind::Minimize).center;
    assert!(cupertino_decorations::hit::in_button_group(&l, inside));
    assert!(!cupertino_decorations::hit::in_button_group(
        &l,
        Point::new(300.0, 16.0)
    ));
}

#[test]
fn untitled_windows_resize_from_the_inside_band_alone() {
    let metrics = DecorationMetrics::default();
    let frame = Rect::new(0.0, 0.0, 200.0, 100.0);
    use ResizeEdge::Left;
    // (what, x, y, titled, untitled)
    let cases = [
        ("outside", -2.0, 50.0, Some(Left), None),
        ("inside the band", 1.0, 50.0, Some(Left), Some(Left)),
        ("deeper inside", 20.0, 50.0, None, None),
    ];
    for (what, x, y, titled, untitled) in cases {
        let p = Point::new(x, y);
        assert_eq!(
            resize_hit(frame, &metrics, p, true),
            titled,
            "{what} titled"
        );
        assert_eq!(
            resize_hit(frame, &metrics, p, false),
            untitled,
            "{what} untitled"
        );
    }
}

#[test]
fn only_the_title_bar_runs_the_double_click_action() {
    let cases = [
        (
            Hit::Titlebar,
            DoubleClickAction::Minimize,
            DoubleClickAction::Minimize,
        ),
        (
            Hit::Titlebar,
            DoubleClickAction::Zoom,
            DoubleClickAction::Zoom,
        ),
        (
            Hit::Content,
            DoubleClickAction::Zoom,
            DoubleClickAction::None,
        ),
        (
            Hit::Button(ButtonKind::Close),
            DoubleClickAction::Zoom,
            DoubleClickAction::None,
        ),
        (
            Hit::Resize(ResizeEdge::Top),
            DoubleClickAction::Zoom,
            DoubleClickAction::None,
        ),
    ];
    for (hit, configured, expected) in cases {
        assert_eq!(
            cupertino_decorations::hit::double_click(hit, configured),
            expected
        );
    }
}

#[test]
fn a_title_keeps_clear_of_the_buttons() {
    let metrics = DecorationMetrics::default();
    let reach = layout(TitlebarStyle::Standard).group_reach();
    // A centred title stays centred, however long, and leaves the group's reach on both sides.
    let (left, max) = title_slot(400.0, reach, 1000.0, TitleAlignment::Center, &metrics).unwrap();
    assert_eq!(left, (400.0 - max) / 2.0);
    assert!(left >= reach);
    // A leading title starts after the group.
    let (left, max) = title_slot(400.0, reach, 50.0, TitleAlignment::Leading, &metrics).unwrap();
    assert!(left >= reach && left + max <= 400.0);
    // No room, no title.
    assert_eq!(
        title_slot(reach, reach, 50.0, TitleAlignment::Center, &metrics),
        None
    );
    assert_eq!(
        title_slot(reach, reach, 50.0, TitleAlignment::Leading, &metrics),
        None
    );
}
