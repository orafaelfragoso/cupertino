//! The stages a recipe turns on with a value of its own (the key and fill light, the control outline, the edge bleed,
//! the bend outside the edge and the tint) and the look of each shape of a container: off at their defaults, on they
//! follow the formulas of the stage hand-computed at a few points, and the glass program draws them as the CPU model
//! does.

use super::glass_stages::{agrees, scenes};
use cupertino_glass::glass::{
    BleedSource, Container, GlassMode, GlassShape, Rgb, BLEED_LEVEL, SHARP_LEVEL,
};
use cupertino_glass::gpu::HeadlessGpu;
use cupertino_theme::glass::{
    rim_brightness, GlassBorder, GlassParams, GlassSpecular, GlassStyle, GlowDiffuse,
};
use cupertino_theme::{GlassModifiers, GlassVariant};

const FLAT: Rgb = [0.3, 0.5, 0.7];

fn flat(_level: usize, _at: [f32; 2]) -> Rgb {
    FLAT
}

/// A square of straight edges whose top edge lies along y = 0, in light regular glass with nothing at its edge but what
/// a test turns on: no rim, no edge line, no fill over the face, no key and fill light, no edge bleed and no ring.
fn square() -> Container {
    let mut c = Container::new(
        vec![GlassShape::new([0.0, 0.0, 100.0, 100.0], 0.0)],
        0.0,
        GlassMode::default(),
    );
    c.params.highlight.amount = 0.0;
    c.params.shadow.edge_line = 0.0;
    c.params.face.fill = [1.0, 1.0, 1.0, 0.0];
    c.params.specular = GlassSpecular::NONE;
    c.params.bleed.opacity = 0.0;
    c.params.shadow.ring_opacity = 0.0;
    c
}

/// The key and fill light of the light regular glass of the recorded recipe: 0.4 strong, arcs 96° either side of a
/// key from the top, 0.75 points deep, the fill as strong, a diffuse glow 8 times as deep over arcs 0.65 as wide at
/// 0.15 of the strength; here a point deep to make the band's falloff easy to follow.
const SPECULAR: GlassSpecular = GlassSpecular {
    amount: 0.4,
    spread: 96.0 * std::f32::consts::PI / 180.0,
    height: 1.0,
    angle: std::f32::consts::FRAC_PI_2,
    fill: 1.0,
    diffuse: GlowDiffuse {
        height_scale: 8.0,
        spread_scale: 0.65,
        amount_scale: 0.15,
    },
};

fn close(a: Rgb, b: Rgb, tolerance: f32) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() <= tolerance)
}

#[test]
fn a_stage_with_its_values_but_not_its_strength_draws_nothing() {
    let off = square();
    let mut set = square();
    set.params.specular = GlassSpecular {
        amount: 0.0,
        ..SPECULAR
    };
    set.params.border = GlassBorder {
        opacity: 0.0,
        width: 2.0,
    };
    set.params.bleed.opacity = 0.0;
    set.params.bleed.height = 30.0;
    set.params.refraction.outer_amount = -6.0;
    set.params.refraction.outer_height = 8.0;
    set.params.tint = [1.0, 0.0, 0.0, 0.0];
    for y in [-3.0, -0.8, 0.25, 0.75, 4.0, 50.0] {
        let p = [50.0, y];
        assert_eq!(off.shade(p, &flat), set.shade(p, &flat), "at {p:?}");
    }
    assert_eq!(set.bleed_source(), None);
    assert!(!set.reads_sharp());
    assert_eq!(set.sample_area(), off.sample_area());
}

#[test]
fn the_key_and_fill_light_add_white_along_the_lit_arcs() {
    let off = square();
    let mut on = square();
    on.params.specular = SPECULAR;
    // 0.75 inside the top edge, facing the key: the band 1 − smoothstep(0, 1, 0.75) = 0.15625, the diffuse glow
    // 0.15 · (1 − 0.75 / 8)² = 0.123 193 36, at 0.4: 0.112 377 34
    let top = [50.0, 0.75];
    let (base, _) = off.shade(top, &flat);
    let (lit, _) = on.shade(top, &flat);
    let add = 0.4 * (0.15625 + 0.15 * 0.906_25 * 0.906_25);
    assert!(
        close(lit, base.map(|v| (v + add).min(1.0)), 1e-5),
        "{lit:?} {base:?}"
    );
    // on the left side, a quarter turn from either light: each arc 1 − smoothstep(0, 96°, 90°) = 0.011 230 47 of it
    let side = [0.75, 50.0];
    let (base, _) = off.shade(side, &flat);
    let (lit, _) = on.shade(side, &flat);
    let t: f32 = 90.0 / 96.0;
    let arc = 1.0 - t * t * (3.0 - 2.0 * t);
    let wide: f32 = 90.0 / (96.0 * 0.65);
    let glow_arc = if wide >= 1.0 {
        0.0
    } else {
        1.0 - wide * wide * (3.0 - 2.0 * wide)
    };
    let add = 0.4 * (2.0 * arc * 0.15625 + 0.15 * 2.0 * glow_arc * 0.906_25 * 0.906_25);
    assert!(
        close(lit, base.map(|v| (v + add).min(1.0)), 1e-5),
        "{lit:?} {base:?}"
    );
    // deep inside the face, past both bands, nothing
    assert_eq!(
        on.shade([50.0, 30.0], &flat),
        off.shade([50.0, 30.0], &flat)
    );
}

#[test]
fn the_control_outline_is_centred_on_the_edge_and_hidden_under_the_light() {
    let off = square();
    let mut lined = square();
    lined.params.border = GlassBorder {
        opacity: 0.125,
        width: 1.0,
    };
    // half a point inside, on the first pixel the glass covers whole: 0.125 · clamp(0.5 + 0.5 − 0.5) = 0.0625 of black
    let inside = [50.0, 0.5];
    let (base, _) = off.shade(inside, &flat);
    let (dark, a) = lined.shade(inside, &flat);
    assert_eq!(a, 1.0);
    assert!(close(dark, base.map(|v| v * (1.0 - 0.0625)), 1e-6));
    // 0.8 outside: its outer half, 0.125 · 0.2 = 0.025 of black
    let (c, a) = lined.shade([50.0, -0.8], &flat);
    assert!((a - 0.025).abs() < 1e-6, "{a}");
    assert!(close(c, [0.0; 3], 1e-6));
    // under the key light at the top it keeps a fifth: 0.018 75
    let mut lit = lined.clone();
    lit.params.specular = SPECULAR;
    let (_, a) = lit.shade([50.0, -0.8], &flat);
    assert!((a - 0.025 * 0.2).abs() < 1e-6, "{a}");
}

#[test]
fn the_ring_shadow_shades_below_the_top_edge_and_down_the_sides_but_not_along_the_bottom() {
    let off = square();
    let mut ringed = square();
    // the regular ring, unblurred so its stroke is easy to follow: 4 points wide, 8 points down, at 0.06
    let s = &mut ringed.params.shadow;
    (s.ring_stroke, s.ring_offset, s.ring_blur, s.ring_opacity) = (4.0, 8.0, 0.0, 0.06);
    let darkened = |p: [f32; 2]| {
        let (base, _) = off.shade(p, &flat);
        let (c, _) = ringed.shade(p, &flat);
        (base, c)
    };
    // 10 points below the top edge, 2 inside the moved stroke: the face at 0.94
    let (base, c) = darkened([50.0, 10.0]);
    assert!(close(c, base.map(|v| v * 0.94), 1e-6), "{c:?} {base:?}");
    // 2 points in from the left side, half way down: the stroke runs down the side
    let (base, c) = darkened([2.0, 50.0]);
    assert!(close(c, base.map(|v| v * 0.94), 1e-6), "{c:?} {base:?}");
    // just under the top edge, above the moved stroke, and just above the bottom edge, whose stroke lies below the
    // shape: nothing
    for p in [[50.0, 4.0], [50.0, 97.0], [50.0, 93.0]] {
        let (base, c) = darkened(p);
        assert_eq!(c, base, "at {p:?}");
    }
}

#[test]
fn the_rim_shifts_each_channel_by_its_own_brightness() {
    let off = square();
    let mut rimmed = square();
    let h = &mut rimmed.params.highlight;
    (h.amount, h.color_bias, h.height) = (0.5, -0.25, 0.5);
    // a quarter point inside the top edge, within the rim's 0.85-point band: a straight edge keeps 0.15 of the rim, so
    // each channel moves by 0.5 · −0.25 · 0.15 times the response to its own value
    let p = [50.0, 0.25];
    let (base, _) = off.shade(p, &flat);
    let (c, _) = rimmed.shade(p, &flat);
    let k = 0.5 * -0.25 * 0.15;
    let expected = base.map(|v| (v + k * rim_brightness(v)).clamp(0.0, 1.0));
    assert!(close(c, expected, 1e-6), "{c:?} {expected:?}");
    // a coloured face: the channels move by different amounts, so the rim keeps the face's hue rather than one shift
    let shift: Vec<f32> = c.iter().zip(base).map(|(a, b)| a - b).collect();
    assert!((shift[0] - shift[2]).abs() > 1e-4, "{shift:?}");
}

#[test]
fn the_edge_bleed_blends_what_lies_outside_through_its_own_levels() {
    let off = square();
    let mut bleeding = square();
    let b = &mut bleeding.params.bleed;
    (b.height, b.amount, b.blur, b.opacity) = (10.0, 5.0, 3.0, 0.2);
    (b.black, b.white, b.saturation, b.darken) = (0.9, 1.0, 1.2, true);
    bleeding.values[0].bleed_height = 10.0;
    bleeding.values[0].bleed_amount = 5.0;
    // 0.75 inside the top edge, t = 0.075: read 5 · 0.925 = 4.625 points outside it, at y = −3.875
    let colour = [0.2, 0.4, 0.6];
    let sample = |level: usize, q: [f32; 2]| {
        if level == BLEED_LEVEL {
            assert!(
                (q[1] + 3.875).abs() < 1e-4 && (q[0] - 50.0).abs() < 1e-4,
                "read at {q:?}"
            );
            colour
        } else {
            FLAT
        }
    };
    let p = [50.0, 0.75];
    let (base, _) = off.shade(p, &flat);
    let (bled, _) = bleeding.shade(p, &sample);
    // saturated 1.2 about its luma 0.371 92 and lifted by 0.9 · (1 − 0.371 92): 0.730 888, 0.970 888, 1
    let tone = [0.730_888, 0.970_888, 1.0];
    let w = 0.2 * 0.925 * 0.925;
    let expected: Rgb = std::array::from_fn(|i| base[i] + (base[i].min(tone[i]) - base[i]) * w);
    assert!(close(bled, expected, 1e-5), "{bled:?} {expected:?}");
    // in normal blending the tone itself
    bleeding.params.bleed.darken = false;
    let (bled, _) = bleeding.shade(p, &sample);
    let expected: Rgb = std::array::from_fn(|i| base[i] + (tone[i] - base[i]) * w);
    assert!(close(bled, expected, 1e-5), "{bled:?} {expected:?}");
    // past its band, nothing
    assert_eq!(
        bleeding.shade([50.0, 20.0], &flat),
        off.shade([50.0, 20.0], &flat)
    );
    assert_eq!(bleeding.bleed_source(), Some(BleedSource::Own(3.0)));
    let [x, ..] = bleeding.sample_area().unwrap();
    assert!(
        x <= -(5.0 + 9.0),
        "it reads its reach and three deviations past the shape: {x}"
    );
}

#[test]
fn the_edge_bleed_reads_a_copy_of_its_deviation_the_container_blurs_already() {
    let mut c = square();
    let widest = c.levels()[0].0;
    c.params.bleed.opacity = 0.2;
    c.params.bleed.blur = widest;
    assert_eq!(c.bleed_source(), Some(BleedSource::Level(0)));
}

#[test]
fn the_edge_bleed_reads_a_copy_within_a_tenth_of_its_deviation_and_reaches_as_far_as_that_copy() {
    let mut c = square();
    let fill = c.fill_sigma().unwrap();
    c.params.bleed.opacity = 0.2;
    c.params.bleed.amount = 5.0;
    c.params.bleed.height = 10.0;
    c.values[0].bleed_amount = 5.0;
    c.values[0].bleed_height = 10.0;
    // within a tenth of its own deviation of the blur fill's: the fill's copy, read as far as its deviation
    c.params.bleed.blur = fill * 0.95;
    assert_eq!(c.bleed_source(), Some(BleedSource::Fill));
    assert_eq!(c.bleed_sigma(), Some(fill));
    let [x, ..] = c.sample_area().unwrap();
    assert!((x + 5.0 + 3.0 * fill).abs() < 1e-4, "{x}");
    // further off: a copy of its own
    c.params.bleed.blur = fill * 0.85;
    assert_eq!(c.bleed_source(), Some(BleedSource::Own(fill * 0.85)));
}

/// The regular toolbar glass of a capsule `side` points high, twice as wide.
fn toolbar(side: f32) -> Container {
    let mode = GlassMode::new(false, GlassVariant::ToolbarItem, GlassModifiers::DEFAULT);
    Container::new(
        vec![GlassShape::new([0.0, 0.0, 2.0 * side, side], side / 2.0)],
        0.0,
        mode,
    )
}

#[test]
fn a_controls_edge_bleed_keeps_its_own_blur_and_a_panels_reads_its_blur_fill_as_far_as_its_band() {
    // a 36-point capsule: 0.35 of its side for the reach and the blur, read that and three deviations around it
    let capsule = toolbar(36.0);
    let p = &capsule.params.bleed;
    assert!(
        (p.blur - 12.6).abs() < 1e-4 && (p.amount - 12.6).abs() < 1e-4,
        "{p:?}"
    );
    assert_eq!(capsule.bleed_source(), Some(BleedSource::Own(p.blur)));
    let [x, ..] = capsule.sample_area().unwrap();
    assert!((x + 4.0 * 12.6).abs() < 1e-3, "{x}");
    // a 300-point panel: its bleed reads the blur fill's copy as far out as its band is deep (105 points), and three
    // deviations of that copy around it
    let panel = toolbar(300.0);
    assert_eq!(panel.bleed_source(), Some(BleedSource::Fill));
    let fill = panel.fill_sigma().unwrap();
    assert!((panel.params.bleed.amount - 105.0).abs() < 1e-3);
    let [x, ..] = panel.sample_area().unwrap();
    assert!((x + 105.0 + 3.0 * fill).abs() < 1e-3, "{x}");
}

#[test]
fn the_backdrop_outside_the_edge_bends_and_fades_across_its_band() {
    let mut bent = square();
    bent.params.refraction.outer_opacity = 0.6;
    bent.values[0].outer_amount = -4.0;
    bent.values[0].outer_height = 8.0;
    assert!(bent.reads_sharp());
    // 2 points above the top edge, t = 0.25: g = 0.9 · e^(−1.05) · (1 − 0.25⁴) = 0.313 712, read −4 · g along the
    // outward normal, at y = −2 + 1.254 85; laid at 0.6 · (1 − smoothstep(0, 1, 0.25)) = 0.506 25
    let g = 0.9 * (-1.05f32).exp() * (1.0 - 0.25f32.powi(4));
    let read_y = -2.0 + 4.0 * g;
    let colour = [0.9, 0.1, 0.4];
    let sample = |level: usize, q: [f32; 2]| {
        if level == SHARP_LEVEL {
            assert!(
                (q[1] - read_y).abs() < 1e-4 && (q[0] - 50.0).abs() < 1e-4,
                "read at {q:?}"
            );
            colour
        } else {
            FLAT
        }
    };
    let (c, a) = bent.shade([50.0, -2.0], &sample);
    assert!((a - 0.506_25).abs() < 1e-5, "{a}");
    assert!(close(c, colour, 1e-5), "{c:?}");
    // past its band nothing is drawn
    assert_eq!(bent.shade([50.0, -9.0], &flat).1, 0.0);
}

#[test]
fn the_tint_lies_over_the_face_at_its_alpha_times_the_face_opacity() {
    let off = square();
    let mut tinted = square();
    tinted.params = tinted.params.with_tint([1.0, 0.0, 0.0, 0.5], 0.5);
    assert_eq!(tinted.params.tint, [1.0, 0.0, 0.0, 0.25]);
    let p = [50.0, 50.0];
    let (base, _) = off.shade(p, &flat);
    let (c, _) = tinted.shade(p, &flat);
    let expected: Rgb = std::array::from_fn(|i| base[i] + ([1.0, 0.0, 0.0][i] - base[i]) * 0.25);
    assert!(close(c, expected, 1e-6), "{c:?} {expected:?}");
}

/// A toolbar group and a segment inside it, clear of its edges, in one container joined hard, without the ring shadow
/// (which darkens the face after the tint).
fn group() -> Container {
    let mut c = Container::new(
        vec![
            GlassShape::new([10.0, 10.0, 80.0, 40.0], 12.0),
            GlassShape::new([60.0, 16.0, 24.0, 28.0], 10.0),
        ],
        0.0,
        GlassMode::default(),
    );
    c.for_each_recipe(|r| r.shadow.ring_opacity = 0.0);
    c
}

#[test]
fn each_shape_of_a_container_keeps_its_own_glass() {
    let same = group();
    let mut red = GlassParams::new(GlassMode::default(), 30.0).with_tint([1.0, 0.0, 0.0, 0.5], 1.0);
    red.shadow.ring_opacity = 0.0;
    let mixed = group().with_shape_recipe(1, red);
    // the group's own glass where only it lies, the segment's tint over the same face where it lies, though the
    // group's outline is the nearer one there
    let group_point = [30.0, 30.0];
    assert_eq!(
        mixed.shade(group_point, &flat),
        same.shade(group_point, &flat)
    );
    let segment_point = [72.0, 30.0];
    assert!(mixed.shapes[0].sdf(segment_point) < mixed.shapes[1].sdf(segment_point));
    let (base, _) = same.shade(segment_point, &flat);
    let (c, _) = mixed.shade(segment_point, &flat);
    let expected: Rgb = std::array::from_fn(|i| base[i] + ([1.0, 0.0, 0.0][i] - base[i]) * 0.5);
    assert!(close(c, expected, 1e-6), "{c:?} {expected:?}");
    // another style on the segment changes its face and leaves the group's
    let clear = GlassMode::of_style(false, GlassStyle::Clear);
    let segment = group().with_shape_mode(1, clear);
    assert!(segment.recipes[1].is_some() && segment.recipes[0].is_none());
    assert_ne!(
        segment.shade(segment_point, &flat),
        same.shade(segment_point, &flat)
    );
    assert_eq!(
        segment.shade(group_point, &flat),
        same.shade(group_point, &flat)
    );
}

/// Scenes with every stage on, over the textures of the device checks.
fn lit_scenes() -> Vec<(&'static str, Container)> {
    let mut out = Vec::new();
    for (name, base) in scenes().into_iter().take(3) {
        let mut c = base.clone();
        c.for_each_recipe(|r| {
            r.specular = SPECULAR;
            r.border = GlassBorder {
                opacity: 0.3,
                width: 1.5,
            };
            r.tint = [0.2, 0.6, 1.0, 0.3];
            r.bleed.opacity = 0.5;
            r.bleed.blur = 4.0;
            r.refraction.outer_opacity = 0.7;
        });
        for v in &mut c.values {
            (v.bleed_amount, v.bleed_height) = (4.0, 9.0);
            (v.outer_amount, v.outer_height) = (-5.0, 7.0);
        }
        out.push((name, c));
    }
    let clear = GlassMode::of_style(true, GlassStyle::Clear);
    out.push(("segment of other glass", group().with_shape_mode(1, clear)));
    let tinted = GlassParams::new(GlassMode::default(), 30.0).with_tint([0.9, 0.2, 0.1, 0.6], 1.0);
    let mut joined = group().with_shape_recipe(1, tinted);
    joined.spacing = 16.0;
    out.push(("tinted segment joined", joined));
    out
}

#[test]
fn the_glass_program_draws_every_stage_as_the_cpu_model() {
    let Some(gpu) = HeadlessGpu::new() else {
        eprintln!("no headless graphics device: the glass program is not exercised");
        return;
    };
    for (name, c) in lit_scenes() {
        if let Err(e) = agrees(&gpu, &c) {
            panic!("{name} with every stage on: {e}");
        }
    }
}
