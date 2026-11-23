//! Glass a node asks for ([`PaintCx::glass`]) as the window asks its window system for it: a legacy style alone goes
//! as the style; a system variant goes with its size class, whether it responds to a press, steps back or draws no shadow
//! of its own, and its tint, the style kept
//! for a window system without variants; interactive glass of a style alone is named by the style's own variant so the
//! compositor can grow it.

use crate::support::{options, turns};

use cupertino_geometry::Size;
use cupertino_theme::ui::Surface;
use cupertino_ui_core::layout::{Constraints, ContainerKind, LayoutSpec, Measured};
use cupertino_ui_core::platform::{
    Backdrop, Capabilities, Color, DecorationMode, GlassModifiers, GlassRegion, GlassSize,
    GlassStyle, GlassVariant, Timestamp, WindowRequest, WindowSpec, WindowStates,
};
use cupertino_ui_core::{
    Application, Change, Command, CommandRegistry, Element, Event, EventCx, Glass, MeasureCx,
    PaintCx, Propagation, SemanticsCx, Widget, Window, WindowBackground, WindowKey,
};
use cupertino_ui_test::{ManualClock, TestDriver};
use std::cell::{Cell, RefCell};

thread_local! {
    /// The glass the next window's node asks for.
    static GLASS: RefCell<Option<Glass>> = const { RefCell::new(None) };
    /// Whether the node last painted was told the window's glass takes variants, tints and scales.
    static TAKES: Cell<Option<(bool, bool, bool)>> = const { Cell::new(None) };
}

/// One window whose one node is the glass the test set.
struct Pane;

impl Application for Pane {
    type Model = Glass;
    type Message = ();

    fn init() -> (Glass, Vec<Command<()>>) {
        (
            GLASS.with(|g| g.borrow().expect("the test sets the glass")),
            Vec::new(),
        )
    }

    fn update(_: &mut Glass, _: ()) -> Vec<Command<()>> {
        Vec::new()
    }

    fn view(glass: &Glass) -> Vec<Window<()>> {
        vec![Window {
            key: WindowKey("main".into()),
            spec: WindowSpec {
                title: "Glass".into(),
                app_id: "org.cupertino.tests.Glass".into(),
                size: Size::new(200.0, 100.0),
                min_size: None,
                max_size: None,
                decorations: DecorationMode::None,
                foreign_parent: None,
                input_region: None,
            },
            shortcuts: CommandRegistry::new(),
            on_close: None,
            background: WindowBackground::None,
            popup: None,
            content: Element::new(Pad(*glass)),
            menus: None,
        }]
    }
}

/// A leaf of 80 by 30 points that asks for its glass.
struct Pad(Glass);

impl Widget<()> for Pad {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Leaf)
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::Paint
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(80.0, 30.0),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, ()>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let takes = (cx.glass_variants(), cx.glass_tint(), cx.glass_scale());
        TAKES.with(|t| t.set(Some(takes)));
        assert!(cx.glass(self.0), "the window shows the glass");
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// The glass region the window asks for when its node asks for `glass`, on a window system with the desktop's effects.
fn region_of(glass: Glass) -> GlassRegion {
    region_on(
        Capabilities {
            app_effects: true,
            ..Capabilities::default()
        },
        glass,
    )
}

/// [`region_of`] on a window system of `capabilities`.
fn region_on(capabilities: Capabilities, glass: Glass) -> GlassRegion {
    GLASS.with(|g| *g.borrow_mut() = Some(glass));
    let mut driver =
        TestDriver::<Pane>::new(capabilities, options(1000), ManualClock::new(Timestamp(0)));
    driver.run_until_idle(turns()).unwrap();
    let window = driver.runtime().windows()[0].1;
    let states = WindowStates {
        activated: true,
        ..WindowStates::default()
    };
    driver
        .window_system_mut()
        .configure(window, None, states)
        .unwrap();
    driver.run_until_idle(turns()).unwrap();
    let materials = driver
        .window_system_mut()
        .requests(window)
        .unwrap()
        .iter()
        .rev()
        .find_map(|r| match r {
            WindowRequest::Materials(m) => Some(m.clone()),
            _ => None,
        })
        .expect("the window asks for its materials");
    assert_eq!(materials.len(), 1, "{materials:?}");
    materials[0].glass.expect("the material is glass")
}

#[test]
fn a_node_is_told_whether_the_window_systems_glass_takes_variants_tints_and_scales() {
    let glass = Glass::clear().variant(GlassVariant::Clear);
    region_of(glass);
    assert_eq!(
        TAKES.get(),
        Some((false, false, false)),
        "an older compositor draws glass by its style alone"
    );
    for (variants, tint, scale) in [
        (true, false, false),
        (false, true, false),
        (false, false, true),
    ] {
        region_on(
            Capabilities {
                app_effects: true,
                glass_variants: variants,
                glass_tint: tint,
                glass_scale: scale,
                ..Capabilities::default()
            },
            glass,
        );
        assert_eq!(TAKES.get(), Some((variants, tint, scale)));
    }
}

#[test]
fn glass_of_a_style_alone_goes_as_the_style_untinted() {
    let region = region_of(Glass::large_panel().radius(12.0).container(2, 6.0));
    assert_eq!(
        region,
        GlassRegion::of_style(GlassStyle::LargePanel, 2, 6.0)
    );
}

#[test]
fn a_variant_goes_with_its_size_class_its_press_response_and_its_tint_and_keeps_the_style() {
    let region = region_of(
        Glass::clear()
            .variant(GlassVariant::ControlCenter)
            .size_class(GlassSize::Large)
            .interactive(true)
            .tint(Color::new(0.25, 0.5, 1.0, 0.4))
            .capsule(),
    );
    assert_eq!(
        region.style,
        GlassStyle::Clear,
        "what a window system without variants draws"
    );
    assert_eq!(
        region.variant,
        Some((
            GlassVariant::ControlCenter,
            GlassModifiers {
                size: GlassSize::Large,
                interactive: true,
                ..GlassModifiers::DEFAULT
            }
        ))
    );
    assert_eq!(region.tint, [0.25, 0.5, 1.0, 0.4]);
}

#[test]
fn interactive_glass_of_a_style_is_named_by_the_styles_own_variant() {
    let region = region_of(Glass::regular().interactive(true));
    let (variant, modifiers) = GlassStyle::Regular.variant();
    assert_eq!(
        region.variant,
        Some((
            variant,
            GlassModifiers {
                interactive: true,
                ..modifiers
            }
        ))
    );
    assert_eq!(region.tint, [0.0; 4]);
}

#[test]
fn a_variant_goes_subdued_and_without_its_shadow_when_asked() {
    let region = region_of(
        Glass::regular()
            .variant(GlassVariant::NotificationCenter)
            .size_class(GlassSize::Medium)
            .subdued(true)
            .exclude_shadow(true),
    );
    assert_eq!(
        region.variant,
        Some((
            GlassVariant::NotificationCenter,
            GlassModifiers {
                size: GlassSize::Medium,
                subdued: true,
                exclude_shadow: true,
                ..GlassModifiers::DEFAULT
            }
        ))
    );
}

#[test]
fn a_tint_out_of_range_is_held_to_it() {
    let region = region_of(Glass::regular().tint(Color::new(1.5, -0.5, f32::NAN, 0.5)));
    assert_eq!(region.tint, [1.0, 0.0, 0.0, 0.5]);
    assert_eq!(region.variant, None, "a tint alone keeps the style");
}

#[test]
fn a_glass_scale_goes_with_its_region_and_one_not_above_0_draws_it_at_the_node() {
    assert_eq!(region_of(Glass::regular()).scale, 1.0);
    assert_eq!(region_of(Glass::regular().scale(1.043)).scale, 1.043);
    for bad in [0.0, -2.0, f64::NAN, f64::INFINITY] {
        assert_eq!(region_of(Glass::regular().scale(bad)).scale, 1.0, "{bad}");
    }
}

#[test]
fn a_sheets_variant_goes_document_modal_and_a_style_alone_does_not() {
    let sheet = region_of(
        Glass::dense()
            .variant(GlassVariant::GlassWindow)
            .document_modal(true),
    );
    assert!(sheet.document_modal);
    assert_eq!(
        sheet.variant.map(|(v, _)| v),
        Some(GlassVariant::GlassWindow)
    );
    let alert = region_of(Glass::dense().variant(GlassVariant::GlassWindow));
    assert!(!alert.document_modal);
    let style = region_of(Glass::dense().document_modal(true));
    assert!(
        !style.document_modal,
        "only a variant carries its modifiers"
    );
}

thread_local! {
    /// The background glass the next window of [`Panel`] asks for.
    static BACKGROUND: RefCell<Option<Glass>> = const { RefCell::new(None) };
}

/// One window whose background is the sidebar surface, drawn as the glass the test set.
struct Panel;

impl Application for Panel {
    type Model = Option<Glass>;
    type Message = ();

    fn init() -> (Option<Glass>, Vec<Command<()>>) {
        (BACKGROUND.with(|g| *g.borrow()), Vec::new())
    }

    fn update(_: &mut Option<Glass>, _: ()) -> Vec<Command<()>> {
        Vec::new()
    }

    fn view(glass: &Option<Glass>) -> Vec<Window<()>> {
        let mut windows = Pane::view(&Glass::regular());
        windows[0].background = WindowBackground::Surface {
            surface: Surface::Sidebar,
            radius: 16.0,
            alpha: 1.0,
            glass: *glass,
        };
        windows[0].content = Element::new(Plain);
        windows
    }
}

/// A leaf of 80 by 30 points that paints nothing.
struct Plain;

impl Widget<()> for Plain {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Leaf)
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(80.0, 30.0),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, ()>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// The backdrop the window of [`Panel`] asks for with `glass` as its background's, on a window system with the
/// desktop's effects.
fn backdrop_of(glass: Option<Glass>) -> Backdrop {
    BACKGROUND.with(|g| *g.borrow_mut() = glass);
    let capabilities = Capabilities {
        app_effects: true,
        ..Capabilities::default()
    };
    let mut driver =
        TestDriver::<Panel>::new(capabilities, options(1000), ManualClock::new(Timestamp(0)));
    driver.run_until_idle(turns()).unwrap();
    let window = driver.runtime().windows()[0].1;
    driver
        .window_system_mut()
        .requests(window)
        .unwrap()
        .iter()
        .rev()
        .find_map(|r| match r {
            WindowRequest::Backdrop(b) => b.clone(),
            _ => None,
        })
        .expect("the window asks for its backdrop")
}

#[test]
fn a_window_background_asks_for_its_glass_variant_on_its_own() {
    let backdrop = backdrop_of(Some(
        Glass::sidebar()
            .variant(GlassVariant::AbuttedSidebar)
            .subdued(true)
            .tint(Color::new(0.0, 0.5, 1.0, 0.25))
            .container(3, 8.0)
            .radius(4.0),
    ));
    assert_eq!(
        backdrop.radius, 16.0,
        "the window's corners, not the glass's"
    );
    let glass = backdrop.glass.expect("the background is glass");
    assert_eq!((glass.container, glass.spacing), (0, 0.0), "on its own");
    assert_eq!(
        glass.style,
        GlassStyle::Sidebar,
        "kept for a window system without variants"
    );
    assert_eq!(
        glass.variant,
        Some((
            GlassVariant::AbuttedSidebar,
            GlassModifiers {
                subdued: true,
                ..GlassModifiers::DEFAULT
            }
        ))
    );
    assert_eq!(glass.tint, [0.0, 0.5, 1.0, 0.25]);
}

#[test]
fn a_window_background_without_glass_asks_for_its_material_alone() {
    let backdrop = backdrop_of(None);
    assert_eq!(backdrop.glass, None);
    assert!(!backdrop.material.is_empty());
}
