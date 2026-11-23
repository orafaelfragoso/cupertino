//! The identities a glass container's textures and composite are kept under: a shape grown inside an unchanged sampled
//! area keeps its blurred textures and draws only its composite again, and every part of its recipe the composite reads
//! tells one look from another.

use cupertino_glass::glass::{Container, GlassShape};
use cupertino_server_render::render::material::glass::GlassContainer;
use cupertino_theme::{GlassMode, GlassModifiers, GlassVariant};
use smithay::utils::{Logical, Rectangle};

const GENERATION: u64 = 7;
const SCALE: f32 = 1.5;

fn mode(variant: GlassVariant) -> GlassMode {
    GlassMode::new(false, variant, GlassModifiers::DEFAULT)
}

/// A 36-point capsule of regular glass at rest, or grown about its centre by `grow`, its recipe the one at rest (as the
/// server draws a pressed glass button).
fn button(grow: f32) -> Container {
    let rest = [100.0, 100.0, 96.0, 36.0];
    let mut c = Container::new(
        vec![GlassShape::capsule(rest)],
        0.0,
        mode(GlassVariant::Regular),
    );
    let [x, y, w, h] = rest;
    let (gw, gh) = (w * grow, h * grow);
    c.shapes[0] = GlassShape::capsule([x - (gw - w) / 2.0, y - (gh - h) / 2.0, gw, gh]);
    c
}

/// The button's subsurface, widened to hold its glass at its largest: the glass reads no more than it.
fn bound() -> Rectangle<f64, Logical> {
    Rectangle::new((96.0, 96.0).into(), (104.0, 44.0).into())
}

#[test]
fn glass_keys_a_grown_shape_inside_its_bound_keeps_its_blurred_textures_and_draws_again() {
    let rest = GlassContainer::new(button(1.0)).bounded(bound());
    for grow in [1.014, 1.043] {
        let grown = GlassContainer::new(button(grow)).bounded(bound());
        assert_eq!(grown.sample_area(), rest.sample_area());
        assert_eq!(
            grown.state(GENERATION, SCALE),
            rest.state(GENERATION, SCALE),
            "the blurred textures are kept at {grow}"
        );
        assert_ne!(
            grown.key, rest.key,
            "the composite is drawn again at {grow}"
        );
    }
    // a new backdrop or another scale makes them again
    assert_ne!(
        rest.state(GENERATION + 1, SCALE),
        rest.state(GENERATION, SCALE)
    );
    assert_ne!(rest.state(GENERATION, 2.0), rest.state(GENERATION, SCALE));
}

#[test]
fn glass_keys_a_change_of_blur_makes_the_textures_again() {
    let regular = GlassContainer::new(button(1.0));
    let mut c = button(1.0);
    c.params.blur.fill_radius *= 2.0;
    assert_ne!(
        GlassContainer::new(c).state(GENERATION, SCALE),
        regular.state(GENERATION, SCALE)
    );
    let clear = GlassContainer::new(Container::new(
        vec![GlassShape::capsule([100.0, 100.0, 96.0, 36.0])],
        0.0,
        mode(GlassVariant::Clear),
    ));
    assert_ne!(
        clear.state(GENERATION, SCALE),
        regular.state(GENERATION, SCALE)
    );
}

/// A change of a container's recipe.
type Change = fn(&mut Container);

/// A glass window's container, 300 points square.
fn window() -> Container {
    Container::new(
        vec![GlassShape::new([40.0, 40.0, 300.0, 300.0], 16.0)],
        0.0,
        mode(GlassVariant::GlassWindow),
    )
}

#[test]
fn glass_keys_a_sheets_shadow_tells_its_glass_from_an_alerts() {
    let mut alert = window();
    alert.params.shadow = alert.params.shadow.alert(true, false);
    let mut sheet = window();
    sheet.params.shadow = sheet.params.shadow.sheet(true, false);
    let (alert, sheet) = (GlassContainer::new(alert), GlassContainer::new(sheet));
    assert_ne!(alert.key, sheet.key);
    // the shadow is the composite's: the textures are the same
    assert_eq!(
        alert.state(GENERATION, SCALE),
        sheet.state(GENERATION, SCALE)
    );
}

#[test]
fn glass_keys_every_part_of_the_recipe_the_composite_reads_changes_the_key() {
    let plain = GlassContainer::new(window()).key;
    let changes: [(&str, Change); 7] = [
        ("shadow offset", |c| c.params.shadow.offset[1] += 2.0),
        ("shadow colour", |c| c.params.shadow.fill[0] = 0.5),
        ("tint", |c| {
            c.params = c.params.with_tint([1.0, 0.0, 0.0, 0.5], 0.5)
        }),
        ("blur ramp", |c| c.blur_ramp = [1.0, 0.5]),
        ("lens", |c| c.lens = 1.2),
        ("veil", |c| c.params.veil = [0.2, 0.4]),
        ("edge line", |c| c.params.shadow.edge_line += 0.1),
    ];
    for (name, change) in changes {
        let mut c = window();
        change(&mut c);
        assert_ne!(GlassContainer::new(c).key, plain, "{name}");
    }
}
