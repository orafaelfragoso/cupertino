//! Glass a node asks for ([`PaintCx::glass`]) as the window asks its window system for it: a legacy style alone goes
//! as the style; a system variant goes with its size class, whether it responds to a press, steps back or draws no shadow
//! of its own, and its tint, the style kept
//! for a window system without variants; interactive glass of a style alone is named by the style's own variant so the
//! compositor can grow it.

use crate::support::{options, turns};

use cupertino_geometry::Size;
use cupertino_ui_core::layout::{Constraints, ContainerKind, LayoutSpec, Measured};
use cupertino_ui_core::platform::{
    Capabilities, Color, DecorationMode, GlassModifiers, GlassRegion, GlassSize, GlassStyle,
    GlassVariant, Timestamp, WindowRequest, WindowSpec, WindowStates,
};
use cupertino_ui_core::{
    Application, Change, Command, CommandRegistry, Element, Event, EventCx, Glass, MeasureCx,
    PaintCx, Propagation, SemanticsCx, Widget, Window, WindowBackground, WindowKey,
};
use cupertino_ui_test::{ManualClock, TestDriver};
use std::cell::RefCell;

thread_local! {
    /// The glass the next window's node asks for.
    static GLASS: RefCell<Option<Glass>> = const { RefCell::new(None) };
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
        assert!(cx.glass(self.0), "the window shows the glass");
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// The glass region the window asks for when its node asks for `glass`, on a window system with the desktop's effects.
fn region_of(glass: Glass) -> GlassRegion {
    GLASS.with(|g| *g.borrow_mut() = Some(glass));
    let capabilities = Capabilities {
        app_effects: true,
        ..Capabilities::default()
    };
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
