//! A toolbar control answers a press as the system's does: its whole glass capsule grows by a fixed width about its
//! centre on the toolbar item's spring and settles back on the release, a menu button's lets go by itself, and the
//! pressed segment's fill comes on over 46 ms and goes 50 ms after the release.

use crate::support;

use cupertino_geometry::{Insets, Point, Rect};
use cupertino_ui_controls::{
    Stack, ToolbarButton, ToolbarGlyph, ToolbarGroup, ToolbarItem, ToolbarMenuButton,
    TOOLBAR_CHOICE_INSET, TOOLBAR_MENU_PRESS_HOLD, TOOLBAR_PRESS_GROWTH,
};
use cupertino_ui_core::layout::{Align, Axis};
use cupertino_ui_core::platform::{Capabilities, GlassVariant, Keysym, Mods, MouseButton};
use cupertino_ui_core::{Element, Role};
use support::{Scene, View};

const SPACE: Keysym = Keysym(0x20);

/// A window system whose glass takes variants and scales.
fn effects() -> Capabilities {
    Capabilities {
        app_effects: true,
        raised_layer: true,
        glass_variants: true,
        glass_tint: true,
        glass_scale: true,
        ..Capabilities::default()
    }
}

struct Toolbar;

impl Scene for Toolbar {
    type State = ();
    type Msg = ();

    fn state() {}

    fn update(_: &mut (), _: ()) {}

    fn view(_: &()) -> Element<()> {
        let item = |label: &str, enabled: bool| ToolbarItem {
            key: None,
            glyph: ToolbarGlyph::Symbol("symbol.square.grid.2x2".into()),
            label: label.into(),
            on_press: enabled.then_some(()),
            menu: None,
        };
        Element::new(Stack {
            axis: Axis::Horizontal,
            padding: Insets::uniform(20.0),
            spacing: 8.0,
            align: Align::Start,
            children: vec![
                Element::from(ToolbarGroup {
                    label: "Actions".into(),
                    items: vec![
                        item("Share", true),
                        item("Tags", true),
                        item("Locked", false),
                    ],
                    selected: None,
                    dividers: false,
                }),
                Element::from(ToolbarMenuButton {
                    key: None,
                    symbol: "symbol.square.grid.2x2".into(),
                    label: "View".into(),
                    on_press: Some(()),
                    expanded: false,
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

/// Each toolbar capsule's glass `view` shows now, with where it lies and the scale it is drawn at.
fn capsules(view: &mut View<Toolbar>) -> Vec<(Rect, f64)> {
    let window = view.window;
    view.materials(window)
        .into_iter()
        .filter_map(|m| m.glass.map(|g| (m.rect, g)))
        .filter(|(_, g)| g.variant.map(|(v, _)| v) == Some(GlassVariant::ToolbarItem))
        .map(|(rect, g)| (rect, g.scale))
        .collect()
}

/// The capsule's glass that holds `at`, with its scale.
fn capsule_at(view: &mut View<Toolbar>, at: Point) -> (Rect, f64) {
    capsules(view)
        .into_iter()
        .find(|(r, _)| r.contains(at))
        .unwrap_or_else(|| panic!("no capsule at {at:?}"))
}

/// The scale that widens a capsule `width` points wide by the press's growth.
fn grown(width: f64) -> f64 {
    1.0 + TOOLBAR_PRESS_GROWTH / width
}

/// The capsule's scale every 10 ms for `seconds`.
fn samples(view: &mut View<Toolbar>, at: Point, seconds: f64) -> Vec<f64> {
    (0..(seconds * 100.0).round() as usize)
        .map(|_| {
            view.wait(0.01);
            capsule_at(view, at).1
        })
        .collect()
}

#[test]
fn a_toolbar_groups_whole_glass_grows_two_and_a_half_points_under_a_press_and_settles_back_on_the_release(
) {
    let mut view = View::<Toolbar>::open_moving(effects(), false, false);
    let share = view.center_of(Role::Button, "Share");
    let more = view.center_of(Role::Button, "More");
    let (rect, rest) = capsule_at(&mut view, share);
    assert_eq!(rest, 1.0, "at rest");
    let target = grown(rect.width());
    view.press(share, MouseButton::Left, Mods::NONE);
    let pressing = samples(&mut view, share, 0.3);
    let (peak_at, peak) =
        pressing
            .iter()
            .enumerate()
            .fold((0, 1.0), |a, (i, &s)| if s > a.1 { (i, s) } else { a });
    // past the growth a little about a tenth of a second in, settled by a fifth
    let overshoot = (peak - 1.0) / (target - 1.0);
    assert!(
        (1.03..1.1).contains(&overshoot),
        "peaks at {overshoot} of its growth: {pressing:?}"
    );
    assert!(
        (6..14).contains(&peak_at),
        "peaks {peak_at}0 ms in: {pressing:?}"
    );
    let settled = pressing[pressing.len() - 1];
    assert!(
        (settled - target).abs() < 1e-3 * (target - 1.0) + 1e-6,
        "settled at {settled}, {target} grown"
    );
    assert_eq!(
        capsule_at(&mut view, more).1,
        1.0,
        "only the pressed capsule grows"
    );
    // the release is quick, a little past its rest, and done within 0.3 s
    view.release(share, MouseButton::Left);
    let releasing = samples(&mut view, share, 0.3);
    assert!(
        releasing[3] < target && releasing[3] > 1.0,
        "on its way 40 ms in, the capsule asking its segment every frame: {releasing:?}"
    );
    assert!(
        releasing.iter().any(|&s| s < 1.0),
        "dips past its rest: {releasing:?}"
    );
    let back = releasing[releasing.len() - 1];
    assert!((back - 1.0).abs() < 1e-4, "back at {back}");
}

#[test]
fn a_drag_off_the_segment_keeps_the_capsule_grown_until_the_release() {
    let mut view = View::<Toolbar>::open_moving(effects(), false, false);
    let share = view.center_of(Role::Button, "Share");
    let (rect, _) = capsule_at(&mut view, share);
    view.press(share, MouseButton::Left, Mods::NONE);
    view.move_to(Point::new(rect.center().x, rect.max_y() + 60.0));
    view.wait(0.3);
    let held = capsule_at(&mut view, share).1;
    assert!((held - grown(rect.width())).abs() < 1e-4, "held at {held}");
    view.release(
        Point::new(rect.center().x, rect.max_y() + 60.0),
        MouseButton::Left,
    );
    let going = samples(&mut view, share, 0.35);
    assert!(going[2] < held, "on its way back within a frame: {going:?}");
    assert!((going[going.len() - 1] - 1.0).abs() < 1e-4, "{going:?}");
}

#[test]
fn a_menu_buttons_glass_lets_go_by_itself_after_the_press() {
    let mut view = View::<Toolbar>::open_moving(effects(), false, false);
    let menu = view.center_of(Role::PopUpButton, "View");
    let (rect, _) = capsule_at(&mut view, menu);
    view.press(menu, MouseButton::Left, Mods::NONE);
    view.wait(TOOLBAR_MENU_PRESS_HOLD - 0.01);
    let held = capsule_at(&mut view, menu).1;
    assert!(
        (held - grown(rect.width())).abs() < 0.02 * (grown(rect.width()) - 1.0),
        "grown while held: {held}"
    );
    // the menu takes the release; the glass goes back on its own
    let going = samples(&mut view, menu, 0.35);
    assert!(
        going[2] < held,
        "on its way back just after the hold: {going:?}"
    );
    let back = going[going.len() - 1];
    assert!((back - 1.0).abs() < 1e-4, "back at {back}");
}

#[test]
fn a_disabled_segment_and_reduce_motion_grow_nothing() {
    let mut view = View::<Toolbar>::open_moving(effects(), false, false);
    let locked = view.center_of(Role::Button, "Locked");
    view.press(locked, MouseButton::Left, Mods::NONE);
    for s in samples(&mut view, locked, 0.2) {
        assert_eq!(s, 1.0, "a disabled segment's press");
    }
    view.release(locked, MouseButton::Left);

    let mut still = View::<Toolbar>::open_moving(effects(), false, true);
    let share = still.center_of(Role::Button, "Share");
    still.press(share, MouseButton::Left, Mods::NONE);
    for s in samples(&mut still, share, 0.2) {
        assert_eq!(s, 1.0, "under Reduce Motion");
    }
    still.release(share, MouseButton::Left);
}

#[test]
fn space_on_a_focused_segment_grows_its_capsule_while_held() {
    let mut view = View::<Toolbar>::open_moving(effects(), false, false);
    view.focus(Role::Button, "Tags");
    let tags = view.center_of(Role::Button, "Tags");
    let (rect, _) = capsule_at(&mut view, tags);
    view.key_down(SPACE, Some(' '), Mods::NONE);
    view.wait(0.3);
    let held = capsule_at(&mut view, tags).1;
    assert!((held - grown(rect.width())).abs() < 1e-4, "held at {held}");
    view.key_up(SPACE, Mods::NONE);
    view.wait(0.3);
    assert!((capsule_at(&mut view, tags).1 - 1.0).abs() < 1e-4);
}

/// The pixel of `view`'s window at `at`, as the window draws it under its glass, straight RGBA.
fn pixel(view: &mut View<Toolbar>, at: Point) -> [u8; 4] {
    let window = view.window;
    let system = view.driver.window_system_mut();
    let frame = system.frames(window).unwrap().last().unwrap();
    let pixels = system.composited(window).unwrap().unwrap();
    let scale = f64::from(frame.width) / support::SIZE.width;
    let stride = pixels.len() / frame.height as usize;
    let (x, y) = ((at.x * scale) as usize, (at.y * scale) as usize);
    let p = &pixels[y * stride + x * 4..][..4];
    [p[0], p[1], p[2], p[3]]
}

#[test]
fn a_segments_fill_is_quinary_under_the_pointer_and_comes_to_quaternary_over_46_ms_of_a_press() {
    let mut view = View::<Toolbar>::open_moving(effects(), false, false);
    let segment = view
        .bounds_of(Role::Button, "Share")
        .inset(Insets::uniform(TOOLBAR_CHOICE_INSET));
    let share = view.center_of(Role::Button, "Share");
    // just inside the fill's leading end, clear of the glyph
    let probe = Point::new(segment.min_x() + 3.0, segment.center().y);
    let rest = pixel(&mut view, probe);
    view.move_to(share);
    let hover = pixel(&mut view, probe);
    assert_ne!(hover, rest, "the quinary fill under the pointer");
    view.press(share, MouseButton::Left, Mods::NONE);
    view.wait(0.02);
    let coming = pixel(&mut view, probe);
    view.wait(0.03);
    let pressed = pixel(&mut view, probe);
    assert!(
        coming != hover && coming != pressed,
        "on its way 20 ms in: {hover:?} {coming:?} {pressed:?}"
    );
    view.wait(0.2);
    assert_eq!(
        pixel(&mut view, probe),
        pressed,
        "at the quaternary level by 46 ms"
    );
    // the release waits 50 ms, then the fill goes back to the hover level
    view.release(share, MouseButton::Left);
    view.wait(0.04);
    assert_eq!(
        pixel(&mut view, probe),
        pressed,
        "still pressed 40 ms after the release"
    );
    view.wait(0.05);
    assert_eq!(
        pixel(&mut view, probe),
        hover,
        "back to quinary 90 ms after it"
    );
}
