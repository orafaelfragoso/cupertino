//! A held switch's thumb or slider's knob becomes a lens: where the window system shows surface layers it is the
//! compositor's glass in a layer of its own over the knob, magnifying the track, named by the control lens variant for
//! a switch and the slider lens variant for a slider, and goes when the knob is let go;
//! without surface layers, and with Reduce Transparency, the control paints it and shows no layer.

use crate::support;

use cupertino_geometry::{Insets, Rect};
use cupertino_ui_controls::{Slider, SliderStyle, Stack, Toggle};
use cupertino_ui_core::layout::{Align, Axis};
use cupertino_ui_core::platform::{
    Capabilities, GlassStyle, GlassVariant, Mods, MouseButton, MAX_LAYER_LENS,
};
use cupertino_ui_core::{Element, Role};
use cupertino_ui_test::PresentedSurfaceLayer;
use support::{Scene, View};

#[derive(Clone, Debug, PartialEq)]
enum Msg {
    Volume(f64),
    Wifi(bool),
}

struct Knobs;

impl Scene for Knobs {
    type State = (f64, bool);
    type Msg = Msg;

    fn state() -> (f64, bool) {
        (50.0, false)
    }

    fn update(state: &mut (f64, bool), message: Msg) {
        match message {
            Msg::Volume(v) => state.0 = v,
            Msg::Wifi(on) => state.1 = on,
        }
    }

    fn view(state: &(f64, bool)) -> Element<Msg> {
        Element::new(Stack {
            axis: Axis::Vertical,
            padding: Insets::uniform(40.0),
            spacing: 40.0,
            align: Align::Start,
            children: vec![
                Element::new(Slider {
                    label: "Volume".into(),
                    value: state.0,
                    min: 0.0,
                    max: 100.0,
                    step: None,
                    style: SliderStyle::Regular,
                    ticks: 0,
                    enabled: true,
                    on_change: Some(Box::new(Msg::Volume)),
                    on_end: None,
                }),
                Element::new(Toggle {
                    label: "Wi-Fi".into(),
                    on: state.1,
                    enabled: true,
                    on_toggle: Some(Box::new(Msg::Wifi)),
                }),
            ],
        })
    }
}

/// A window system that shows surface layers.
fn layered() -> Capabilities {
    Capabilities {
        surface_layers: true,
        ..Capabilities::default()
    }
}

/// The surface layers the window showed last.
fn shown(view: &mut View<Knobs>) -> Vec<PresentedSurfaceLayer> {
    let window = view.window;
    view.driver
        .window_system_mut()
        .surface_layers(window)
        .unwrap()
        .last()
        .cloned()
        .unwrap_or_default()
}

/// Holds the control `role` named `name` down at its centre until its lens has swelled, then returns the layers shown.
fn held(view: &mut View<Knobs>, role: Role, name: &str) -> Vec<PresentedSurfaceLayer> {
    let at = view.center_of(role, name);
    view.press(at, MouseButton::Left, Mods::NONE);
    view.wait(0.5);
    shown(view)
}

/// The glass lens a held control shows over `bounds`, its node's: one layer of the compositor's glass, magnifying, in a
/// shape that holds the centre of the control.
fn assert_lens(layers: &[PresentedSurfaceLayer], bounds: Rect, name: &str, variant: GlassVariant) {
    let [layer] = layers else {
        panic!("{name}: one lens layer, not {layers:?}")
    };
    assert_eq!(layer.look.glass, Some(GlassStyle::Track), "{name}");
    assert_eq!(layer.look.variant.map(|(v, _)| v), Some(variant), "{name}");
    assert!(
        layer.look.lens > 1.0 && layer.look.lens <= MAX_LAYER_LENS,
        "{name}: the lens magnifies the track, {}",
        layer.look.lens
    );
    assert!(layer.look.radius > 0.0, "{name}: the lens is rounded");
    assert!(
        layer.rect.contains(bounds.center()),
        "{name}: the lens {:?} lies over the knob of {bounds:?}",
        layer.rect
    );
    assert!(
        layer.rect.width() < bounds.width() * 2.0,
        "{name}: the layer covers the lens, not the window: {:?}",
        layer.rect
    );
}

#[test]
fn a_held_knob_is_the_compositors_glass_lens_in_a_layer_that_goes_when_let_go() {
    let mut view = View::<Knobs>::open_with(layered(), false);
    assert!(shown(&mut view).is_empty(), "no lens at rest");
    for (role, name, variant) in [
        (Role::Slider, "Volume", GlassVariant::Slider),
        (Role::Switch, "Wi-Fi", GlassVariant::Control),
    ] {
        let bounds = view.bounds_of(role, name);
        let layers = held(&mut view, role, name);
        assert_lens(&layers, bounds, name, variant);
        view.release(bounds.center(), MouseButton::Left);
        view.wait(0.5);
        assert!(
            shown(&mut view).is_empty(),
            "{name}: the lens goes once the knob is let go"
        );
    }
    assert!(view.state().1, "the click still toggles the switch");
}

#[test]
fn a_held_knob_paints_its_lens_without_surface_layers_and_with_reduce_transparency() {
    for (capabilities, reduce_transparency) in [(Capabilities::default(), false), (layered(), true)]
    {
        let mut view = View::<Knobs>::open_with(capabilities, reduce_transparency);
        for (role, name) in [(Role::Slider, "Volume"), (Role::Switch, "Wi-Fi")] {
            let layers = held(&mut view, role, name);
            assert!(
                layers.is_empty(),
                "{name} with {capabilities:?}, Reduce Transparency {reduce_transparency}: {layers:?}"
            );
            let at = view.center_of(role, name);
            view.release(at, MouseButton::Left);
            view.wait(0.5);
        }
    }
}
