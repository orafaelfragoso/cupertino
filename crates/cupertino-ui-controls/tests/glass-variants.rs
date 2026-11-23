//! Each control's glass is named by the system variant it stands for, so the compositor draws that variant's recipe
//! (and its inactive branch while the window is not active): the sidebar panel's abutted sidebar glass, the toolbar
//! controls' toolbar item glass with the chosen segment's pill as tab glass in their container, an anchored popover's
//! cartouche popover glass at the medium size.

use crate::support;

use cupertino_geometry::{Insets, Point, Rect, Size};
use cupertino_ui_controls::{
    Button, ButtonRole, ButtonStyle, Dialog, DialogAction, DialogButton, DialogKind, Modal,
    Popover, SidebarWindow, SplitLayout, Stack, ToolbarButton, ToolbarGlyph, ToolbarGroup,
    ToolbarItem, TOOLBAR_CHOICE_INSET,
};
use cupertino_ui_core::layout::{Align, Axis, ScrollState};
use cupertino_ui_core::platform::{
    Capabilities, GlassRegion, GlassSize, GlassVariant, Mods, MouseButton, WindowId, WindowStates,
};
use cupertino_ui_core::{Element, Key, Role, Window, WindowKey};
use support::{Scene, View};

/// A window system with the desktop's effects and a raised layer, whose glass takes variants, tints and scales.
fn effects() -> Capabilities {
    Capabilities {
        glass_variants: true,
        glass_tint: true,
        glass_scale: true,
        ..older()
    }
}

/// A window system with the desktop's effects and a raised layer, whose glass is drawn by its style alone: untinted
/// and at its shape.
fn older() -> Capabilities {
    Capabilities {
        app_effects: true,
        raised_layer: true,
        ..Capabilities::default()
    }
}

/// The pixel of `view`'s window at `at`, as the compositor shows it without its glass, straight RGBA.
fn pixel<S: Scene>(view: &mut View<S>, at: Point) -> [u8; 4] {
    let system = view.driver.window_system_mut();
    let frame = system.frames(view.window).unwrap().last().unwrap();
    let pixels = system.composited(view.window).unwrap().unwrap();
    let scale = f64::from(frame.width) / support::SIZE.width;
    let stride = pixels.len() / frame.height as usize;
    let (x, y) = ((at.x * scale) as usize, (at.y * scale) as usize);
    let p = &pixels[y * stride + x * 4..][..4];
    [p[0], p[1], p[2], p[3]]
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

/// `window`, a popup, configured by its window system as a compositor places one, and drawn.
fn configured<S: Scene>(view: &mut View<S>, window: WindowId) {
    let active = WindowStates {
        activated: true,
        ..WindowStates::default()
    };
    view.driver
        .window_system_mut()
        .configure(window, None, active)
        .unwrap();
    view.settle();
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

#[test]
fn a_single_panel_window_is_abutted_sidebar_glass_over_the_sidebar_material() {
    let cupertino_ui_core::WindowBackground::Surface { surface, glass, .. } =
        cupertino_ui_controls::PANEL_WINDOW_BACKGROUND
    else {
        panic!("a surface background");
    };
    assert_eq!(
        surface,
        cupertino_theme::ui::Surface::Sidebar,
        "its material and its opaque fill"
    );
    assert_eq!(
        glass.and_then(|g| g.variant),
        Some(GlassVariant::AbuttedSidebar)
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

struct Anchored;

impl Scene for Anchored {
    type State = ();
    type Msg = ();

    fn state() {}

    fn update(_: &mut (), _: ()) {}

    fn view(_: &()) -> Element<()> {
        Element::new(Stack {
            axis: Axis::Vertical,
            padding: Insets::uniform(20.0),
            spacing: 0.0,
            align: Align::Start,
            children: vec![Element::new(Button {
                label: "Info".into(),
                role: ButtonRole::Normal,
                style: ButtonStyle::Bordered,
                enabled: true,
                busy: false,
                on_press: None,
            })
            .key("anchor")],
        })
    }

    fn popups(_: &()) -> Vec<Window<()>> {
        vec![Popover {
            parent: WindowKey(Key::from("probe")),
            anchor: Key::from("anchor"),
            label: "Info".into(),
            content: blank(),
            scroll: ScrollState {
                offset: Point::ZERO,
                content: Size::ZERO,
                viewport: Size::ZERO,
            },
            on_scroll: Box::new(|_| ()),
            on_close: (),
        }
        .window(WindowKey(Key::from("popover")))]
    }
}

#[test]
fn an_anchored_popover_and_its_pointer_are_one_cartouche_popover_glass_at_the_medium_size() {
    let mut view = View::<Anchored>::open_with(effects(), false);
    let popup = view.popup().expect("the popover's window");
    configured(&mut view, popup);
    let shown = glass(&mut view, popup);
    // the panel, and the pointer as strips of its outline
    assert!(shown.len() >= 2, "the panel and its pointer: {shown:?}");
    let container = shown[0].1.container;
    assert!(container != 0, "{shown:?}");
    for (_, g) in &shown {
        assert_eq!(
            g.variant.map(|(v, m)| (v, m.size)),
            Some((GlassVariant::CartouchePopover, GlassSize::Medium)),
            "{shown:?}"
        );
        assert_eq!(g.container, container, "one container: {shown:?}");
    }
}

/// A window with a sheet up when `SHEET`, else an alert.
struct Asking<const SHEET: bool>;

impl<const SHEET: bool> Scene for Asking<SHEET> {
    type State = ();
    type Msg = ();

    fn state() {}

    fn update(_: &mut (), _: ()) {}

    fn view(_: &()) -> Element<()> {
        Element::from(Modal {
            content: blank(),
            dialog: Some(Dialog {
                kind: if SHEET {
                    DialogKind::Dialog
                } else {
                    DialogKind::Alert
                },
                icon: None,
                title: "Replace it?".into(),
                message: String::new(),
                body: None,
                buttons: vec![
                    DialogButton {
                        label: "Cancel".into(),
                        action: DialogAction::Cancel,
                        on_press: (),
                    },
                    DialogButton {
                        label: "Replace".into(),
                        action: DialogAction::Default,
                        on_press: (),
                    },
                ],
            }),
        })
    }
}

/// The glass window glass of the panel a window with a sheet (`SHEET`) or an alert shows once it has settled, with
/// Reduce Transparency on or off.
fn window_glass<const SHEET: bool>(reduce_transparency: bool) -> Vec<(Rect, GlassRegion)> {
    window_glass_on::<SHEET>(effects(), reduce_transparency)
}

/// [`window_glass`] on a window system of `capabilities`.
fn window_glass_on<const SHEET: bool>(
    capabilities: Capabilities,
    reduce_transparency: bool,
) -> Vec<(Rect, GlassRegion)> {
    let mut view = View::<Asking<SHEET>>::open_with(capabilities, reduce_transparency);
    // past an alert's pop-in
    view.wait(1.0);
    let window = view.window;
    glass(&mut view, window)
        .into_iter()
        .filter(|(_, g)| variant(g) == Some(GlassVariant::GlassWindow))
        .collect()
}

#[test]
fn alerts_and_sheets_are_glass_window_glass_and_opaque_with_reduce_transparency() {
    for (sheet, shown) in [
        (false, window_glass::<false>(false)),
        (true, window_glass::<true>(false)),
    ] {
        let [(rect, g)] = shown[..] else {
            panic!("sheet {sheet}: one panel of the glass window variant, not {shown:?}")
        };
        assert_eq!(
            g.document_modal, sheet,
            "a sheet casts the sheet's shadow, an alert the alert's"
        );
        assert!(
            rect.width() > 200.0,
            "sheet {sheet}: the whole panel: {rect:?}"
        );
    }
    assert!(window_glass::<false>(true).is_empty(), "the alert's grey");
    assert!(
        window_glass::<true>(true).is_empty(),
        "the sheet's solid body"
    );
}

struct GlassButtons;

impl Scene for GlassButtons {
    type State = ();
    type Msg = ();

    fn state() {}

    fn update(_: &mut (), _: ()) {}

    fn view(_: &()) -> Element<()> {
        let button = |label: &str, role| {
            Element::new(Button {
                label: label.into(),
                role,
                style: ButtonStyle::Glass,
                enabled: true,
                busy: false,
                on_press: Some(()),
            })
        };
        Element::new(Stack {
            axis: Axis::Horizontal,
            padding: Insets::uniform(20.0),
            spacing: 20.0,
            align: Align::Start,
            children: vec![
                button("Share", ButtonRole::Normal),
                button("Done", ButtonRole::Default),
            ],
        })
    }
}

#[test]
fn glass_buttons_are_regular_glass_at_their_size_and_a_default_one_is_tinted_with_the_accent() {
    let mut view = View::<GlassButtons>::open_with(effects(), false);
    let window = view.window;
    let shown = glass(&mut view, window);
    assert_eq!(shown.len(), 2, "{shown:?}");
    for (_, g) in &shown {
        assert_eq!(
            g.variant.map(|(v, m)| (v, m.interactive)),
            Some((GlassVariant::Regular, false)),
            "the button grows its glass itself, so the compositor does not: {shown:?}"
        );
        assert_eq!(g.scale, 1.0, "at rest: {shown:?}");
    }
    let at = |name: &str| view.bounds_of(Role::Button, name).center();
    let (share, done) = (at("Share"), at("Done"));
    let tint_at = |p: Point| {
        shown
            .iter()
            .find(|(r, _)| r.contains(p))
            .map(|(_, g)| g.tint)
            .unwrap()
    };
    assert_eq!(tint_at(share)[3], 0.0, "an ordinary button is untinted");
    let accent = tint_at(done);
    assert_eq!(accent[3], 1.0, "the accent at full strength: {accent:?}");
    assert!(
        accent[..3].iter().any(|&c| c > 0.0),
        "the accent: {accent:?}"
    );
}

/// How much a glass button's glass grows under a press.
const PRESSED: f64 = 1.043;

/// The scale of the glass of the button `name` as `view` shows it now.
fn button_scale(view: &mut View<GlassButtons>, name: &str) -> f64 {
    let window = view.window;
    let at = view.bounds_of(Role::Button, name).center();
    glass(view, window)
        .into_iter()
        .find(|(r, _)| r.contains(at))
        .map(|(_, g)| g.scale)
        .unwrap_or_else(|| panic!("{name} shows glass"))
}

#[test]
fn a_glass_buttons_glass_grows_under_a_press_on_its_spring_and_settles_back_on_the_release() {
    let mut view = View::<GlassButtons>::open_moving(effects(), false, false);
    let at = view.center_of(Role::Button, "Share");
    view.press(at, MouseButton::Left, Mods::NONE);
    // it grows past its size a little and settles there in about 512 ms
    let mut peak: f64 = 1.0;
    for _ in 0..10 {
        view.wait(0.05);
        peak = peak.max(button_scale(&mut view, "Share"));
    }
    assert!(peak > PRESSED && peak < PRESSED + 0.01, "peaks at {peak}");
    view.wait(0.03);
    let settled = button_scale(&mut view, "Share");
    assert!((settled - PRESSED).abs() < 1e-4, "settled at {settled}");
    assert_eq!(
        button_scale(&mut view, "Done"),
        1.0,
        "only the pressed button grows"
    );
    // the release takes about 965 ms to settle back
    view.release(at, MouseButton::Left);
    view.wait(0.5);
    let going = button_scale(&mut view, "Share");
    assert!(
        (going - 1.0).abs() > 1e-4,
        "still settling at 500 ms: {going}"
    );
    view.wait(0.5);
    let back = button_scale(&mut view, "Share");
    assert!((back - 1.0).abs() < 1e-4, "back at {back}");
}

#[test]
fn under_reduce_motion_a_glass_buttons_glass_does_not_grow() {
    let mut view = View::<GlassButtons>::open_moving(effects(), false, true);
    let at = view.center_of(Role::Button, "Share");
    view.press(at, MouseButton::Left, Mods::NONE);
    for _ in 0..4 {
        view.wait(0.1);
        assert_eq!(button_scale(&mut view, "Share"), 1.0);
    }
    view.release(at, MouseButton::Left);
}

#[test]
fn where_glass_takes_no_variants_the_chosen_segments_pill_is_painted() {
    let mut glass_view = View::<Toolbar>::open_with(effects(), false);
    let mut older_view = View::<Toolbar>::open_with(older(), false);
    let window = older_view.window;
    let shown = glass(&mut older_view, window);
    assert!(
        shown
            .iter()
            .all(|(_, g)| variant(g) != Some(GlassVariant::Tab)),
        "no tab glass, which would be the toolbar's glass inside it: {shown:?}"
    );
    // just inside the pill's leading end, clear of its glyph
    let pill = older_view
        .bounds_of(Role::RadioButton, "List")
        .inset(Insets::uniform(TOOLBAR_CHOICE_INSET));
    let at = Point::new(pill.min_x() + 3.0, pill.center().y);
    assert_ne!(
        pixel(&mut older_view, at),
        pixel(&mut glass_view, at),
        "the pill is painted where the compositor would draw its glass"
    );
}

#[test]
fn where_glass_takes_no_tint_a_default_glass_button_fills_the_accent_and_does_not_grow() {
    let mut glass_view = View::<GlassButtons>::open_with(effects(), false);
    let mut older_view = View::<GlassButtons>::open_moving(older(), false, false);
    let window = older_view.window;
    let shown = glass(&mut older_view, window);
    assert_eq!(shown.len(), 2, "both buttons still lie on glass: {shown:?}");
    assert!(
        shown.iter().all(|(_, g)| g.tint[3] == 0.0),
        "no tint is asked for: {shown:?}"
    );
    // beside the label, inside the capsule's straight run
    let beside = |view: &View<GlassButtons>, name: &str| {
        let b = view.bounds_of(Role::Button, name);
        Point::new(b.min_x() + b.height() / 2.0, b.center().y)
    };
    let done = beside(&older_view, "Done");
    assert_ne!(
        pixel(&mut older_view, done),
        pixel(&mut glass_view, done),
        "the accent is filled over the glass"
    );
    let share = beside(&older_view, "Share");
    assert_eq!(
        pixel(&mut older_view, share),
        pixel(&mut glass_view, share),
        "an ordinary button is as it was"
    );

    let press = older_view.center_of(Role::Button, "Share");
    older_view.press(press, MouseButton::Left, Mods::NONE);
    for _ in 0..4 {
        older_view.wait(0.1);
        assert_eq!(
            button_scale(&mut older_view, "Share"),
            1.0,
            "glass the window system draws at its shape is asked for at it"
        );
    }
    older_view.release(press, MouseButton::Left);
}

#[test]
fn where_glass_takes_no_variants_alerts_and_sheets_are_painted() {
    // glass of the style alone would cast no window shadow
    assert!(
        window_glass_on::<false>(older(), false).is_empty(),
        "the alert's grey"
    );
    assert!(
        window_glass_on::<true>(older(), false).is_empty(),
        "the sheet's solid body"
    );
}
