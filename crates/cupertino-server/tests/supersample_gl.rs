//! The final pass of an output drawn at the next whole scale above its fractional one: what it shows of a frame drawn
//! at 2 is that frame shrunk with the area filter (`cupertino_raster_math::resample`); a stroked ring as thin as a
//! window button's keeps its weight all around under it; only what changed is drawn again, and a frame with no change
//! draws nothing; the offscreen frame holds 10 bits a channel for a 10-bit display, and the pass dithers at the
//! display's depth.

use cupertino_decorations::coverage::{Outline, Shade};
use cupertino_decorations::{Draw, DrawList};
use cupertino_raster_math::resample::shrink;
use cupertino_server::backend::headless;
use cupertino_server::render::chrome::{ChromeImage, Resolved};
use cupertino_server::render::programs::Programs;
use cupertino_server::render::screenshot::render_to_image;
use cupertino_server::render::shader_rect::{ElementCache, ElementKey};
use cupertino_server::render::supersample::{panel_damage, ResampledElement, Supersample};
use cupertino_server::render::ui::{rounded_rect, RoundedRectSpec};
use cupertino_server::render::uploads::Drawer;
use cupertino_server::render::CupertinoElement;
use image::RgbaImage;
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::element::texture::TextureRenderElement;
use smithay::backend::renderer::element::utils::RescaleRenderElement;
use smithay::backend::renderer::element::{Element, Kind};
use smithay::backend::renderer::gles::{GlesRenderer, GlesTexture};
use smithay::backend::renderer::{Bind, Color32F, ExportMem, Offscreen};
use smithay::utils::{Rectangle, Scale, Transform};

/// A frame drawn at 2 for a display at 1.5.
const RENDER: f64 = 2.0;
const SCALE: f64 = 1.5;

/// `element` drawn into a target of `format` and `size` pixels at the display's scale over `clear`, read back 8 bits a
/// channel; with the tracker and target kept in `kept` for a later frame (age 1), and the damage it drew.
fn show(
    renderer: &mut GlesRenderer,
    element: ResampledElement,
    format: Fourcc,
    size: (i32, i32),
    kept: &mut Option<(GlesTexture, OutputDamageTracker)>,
) -> (
    RgbaImage,
    Option<Vec<Rectangle<i32, smithay::utils::Physical>>>,
) {
    let age = usize::from(kept.is_some());
    let (texture, tracker) = kept.get_or_insert_with(|| {
        (
            renderer
                .create_buffer(format, size.into())
                .expect("a target"),
            OutputDamageTracker::new(size, SCALE, Transform::Normal),
        )
    });
    let elements = [CupertinoElement::Resampled(element)];
    let mut target = renderer.bind(texture).expect("bind the target");
    let damage = tracker
        .render_output(renderer, &mut target, age, &elements, Color32F::BLACK)
        .expect("render")
        .damage
        .cloned();
    let mapping = renderer
        .copy_framebuffer(&target, Rectangle::from_size(size.into()), Fourcc::Abgr8888)
        .expect("read back");
    drop(target);
    let bytes = renderer.map_texture(&mapping).expect("map");
    (
        RgbaImage::from_raw(size.0 as u32, size.1 as u32, bytes.to_vec()).expect("size"),
        damage,
    )
}

/// `elements` (at 2) drawn into a fresh offscreen frame of `frame` pixels and shown shrunk at `panel` pixels.
fn shrunk(
    renderer: &mut GlesRenderer,
    programs: &Programs,
    elements: &[CupertinoElement],
    frame: (i32, i32),
    panel: (i32, i32),
) -> RgbaImage {
    let mut s = Supersample::default();
    s.dither = false;
    let element = s
        .draw(
            renderer,
            &programs.resample,
            elements,
            frame,
            RENDER,
            panel,
            Color32F::BLACK,
        )
        .expect("draw the frame");
    show(renderer, element, Fourcc::Abgr8888, panel, &mut None).0
}

/// What the area filter makes of `image`, shrunk to `out` pixels, stored in 8 bits.
fn filtered(image: &RgbaImage, out: (i32, i32)) -> RgbaImage {
    let texels: Vec<[f32; 4]> = image
        .pixels()
        .map(|p| p.0.map(|c| f32::from(c) / 255.0))
        .collect();
    let px = shrink(
        &texels,
        (image.width(), image.height()),
        (out.0 as u32, out.1 as u32),
    );
    let bytes: Vec<u8> = px
        .iter()
        .flat_map(|p| p.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8))
        .collect();
    RgbaImage::from_raw(out.0 as u32, out.1 as u32, bytes).expect("size")
}

/// The largest difference of a colour channel between `a` and `b`, and where.
fn worst(a: &RgbaImage, b: &RgbaImage) -> (u8, (u32, u32)) {
    let mut worst = (0, (0, 0));
    for (x, y, p) in a.enumerate_pixels() {
        let q = b.get_pixel(x, y);
        for c in 0..3 {
            let d = p.0[c].abs_diff(q.0[c]);
            if d > worst.0 {
                worst = (d, (x, y));
            }
        }
    }
    worst
}

/// Rounded rectangles with borders, off the pixel grid, over each other, at 2.
fn shapes(programs: &Programs, cache: &mut ElementCache, shift: f64) -> Vec<CupertinoElement> {
    (0..5u8)
        .map(|i| {
            let at = 3.3 + 9.7 * f64::from(i) + shift;
            let rect = Rectangle::new((at, at * 0.6).into(), (30.0, 18.5).into());
            let mut spec = RoundedRectSpec::new(
                rect,
                6.0,
                [0.15 * f32::from(i), 0.55, 0.9 - 0.1 * f32::from(i), 0.8],
            );
            spec.border = Some((0.5, [0.05, 0.05, 0.05, 1.0]));
            CupertinoElement::Shader(rounded_rect(
                programs,
                cache,
                RENDER,
                ElementKey::new("shape", u64::from(i)),
                &spec,
            ))
        })
        .collect()
}

#[test]
fn the_final_pass_shows_the_frame_drawn_at_two_shrunk_with_the_area_filter() {
    let mut renderer = headless::create_renderer().expect("headless renderer");
    let programs = Programs::compile_with(&mut renderer, false).expect("programs");
    let mut cache = ElementCache::default();
    let elements = shapes(&programs, &mut cache, 0.0);
    // 80 x 56 points: 160 x 112 at 2, 120 x 84 at 1.5
    let (frame, panel) = ((160, 112), (120, 84));
    let at_two = render_to_image(
        &mut renderer,
        &elements,
        frame.into(),
        RENDER,
        Color32F::BLACK,
    )
    .expect("the frame at 2");
    let got = shrunk(&mut renderer, &programs, &elements, frame, panel);
    let want = filtered(&at_two, panel);
    let (diff, at) = worst(&got, &want);
    assert!(diff <= 2, "{diff} steps from the filter's at {at:?}");
}

/// The chrome's list drawn on the GPU and shown at one pixel a texel at 2.
fn chrome_element(
    renderer: &mut GlesRenderer,
    programs: &Programs,
    image: &ChromeImage,
) -> CupertinoElement {
    let Resolved::Texture(buffer) = image.resolve(renderer, &programs.passes) else {
        panic!("the chrome is painted on the CPU: the device could not draw it");
    };
    let element = TextureRenderElement::from_texture_render_buffer(
        (0.0, 0.0),
        &buffer,
        None,
        None,
        None,
        Kind::Unspecified,
    );
    CupertinoElement::Chrome(RescaleRenderElement::from_element(
        element,
        (0, 0).into(),
        1.0 / RENDER,
    ))
}

/// A dark ring of half a point round a disc of a button's size, centred off the pixel grid, over the disc's own colour,
/// at `scale`.
fn ring(scale: f32) -> DrawList {
    let side = (32.0 * scale) as u32;
    let mut list = DrawList::new(side, side);
    list.draws.push(Draw::Shape {
        outline: Outline::circle([16.0 * scale, 16.0 * scale], 15.0 * scale),
        shade: Shade::Solid([1.0, 0.37, 0.34, 1.0]),
        stroke: None,
    });
    let centre = [16.0 * scale + 0.37, 16.0 * scale + 0.21];
    list.draws.push(Draw::Shape {
        outline: Outline::circle(centre, 6.0 * scale),
        shade: Shade::Solid([1.0, 0.37, 0.34, 1.0]),
        stroke: None,
    });
    list.draws.push(Draw::Shape {
        outline: Outline::circle(centre, 6.0 * scale),
        shade: Shade::Solid([0.35, 0.1, 0.1, 1.0]),
        stroke: Some(0.5 * scale),
    });
    list
}

/// How dark `image` is along each degree of a ray from `centre` (pixels) between `from` and `to` pixels out: the
/// ring's weight at that angle, read by linear interpolation in steps of a sixteenth of a pixel.
fn weights_round(image: &RgbaImage, centre: (f32, f32), from: f32, to: f32) -> Vec<f32> {
    let value = |x: f32, y: f32| {
        let (x0, y0) = ((x - 0.5).floor(), (y - 0.5).floor());
        let (fx, fy) = (x - 0.5 - x0, y - 0.5 - y0);
        let at = |x: f32, y: f32| f32::from(image.get_pixel(x as u32, y as u32).0[1]) / 255.0;
        let top = at(x0, y0) * (1.0 - fx) + at(x0 + 1.0, y0) * fx;
        let bottom = at(x0, y0 + 1.0) * (1.0 - fx) + at(x0 + 1.0, y0 + 1.0) * fx;
        top * (1.0 - fy) + bottom * fy
    };
    (0..360)
        .map(|deg| {
            let a = (deg as f32).to_radians();
            let steps = ((to - from) * 16.0) as i32;
            (0..steps)
                .map(|i| {
                    let r = from + (i as f32 + 0.5) / 16.0;
                    // the disc's own green, less what is there: the ring's darkness
                    0.37 - value(centre.0 + r * a.cos(), centre.1 + r * a.sin())
                })
                .map(|d| d.max(0.0) / 16.0)
                .sum()
        })
        .collect()
}

#[test]
fn a_ring_as_thin_as_a_window_button_s_keeps_its_weight_all_round_and_its_edges_in_many_levels() {
    let mut renderer = headless::create_renderer().expect("headless renderer");
    let programs = Programs::compile_with(&mut renderer, false).expect("programs");
    let list = ring(RENDER as f32);
    let side = list.width as i32;
    let panel = (side * 3 / 4, side * 3 / 4);
    let at_two = ChromeImage::new(list, Drawer::Decoration);
    let element = chrome_element(&mut renderer, &programs, &at_two);
    let got = shrunk(&mut renderer, &programs, &[element], (side, side), panel);
    // the button at 1.5: centre at 16 points and the offset the list put it at, the ring 6 points out
    let centre = (16.0 * 1.5 + 0.37 * 0.75, 16.0 * 1.5 + 0.21 * 0.75);
    let round = weights_round(&got, centre, 6.0 * 1.5 - 2.0, 6.0 * 1.5 + 2.0);
    let mean = |r: &[f32]| r.iter().sum::<f32>() / r.len() as f32;
    // how far the weight strays round the circle, against its mean (what reading a thin line along rays gives even
    // for a ring drawn one pixel a texel)
    let stray = |r: &[f32]| {
        let m = mean(r);
        (r.iter().map(|w| (w - m) * (w - m)).sum::<f32>() / r.len() as f32).sqrt() / m
    };
    // the same ring drawn at 2 and read at 2
    let at_two_element = chrome_element(&mut renderer, &programs, &at_two);
    let two = render_to_image(
        &mut renderer,
        &[at_two_element],
        (side, side).into(),
        RENDER,
        Color32F::BLACK,
    )
    .expect("the ring at 2");
    let reference = weights_round(&two, (32.37, 32.21), 12.0 - 3.0, 12.0 + 3.0);
    assert!(mean(&round) > 0.15, "a ring is there: {}", mean(&round));
    // the ring keeps its weight: three quarters of what it has at 2, as it is three quarters as wide
    assert!(
        (mean(&round) - 0.75 * mean(&reference)).abs() < 0.01 * mean(&round),
        "{} at 1.5, {} at 2",
        mean(&round),
        mean(&reference)
    );
    // and is as even round the circle as at 2
    assert!(
        stray(&round) < 0.07 && stray(&round) < stray(&reference) * 1.25,
        "the ring strays by {} round the circle, at 2 by {}",
        stray(&round),
        stray(&reference)
    );
    // the ring's pixels: between the disc's face and the ring's colour in many levels, not a few steps
    let edge: Vec<u8> = got
        .pixels()
        .map(|p| p.0[1])
        .filter(|&g| g > 26 && g < 94)
        .collect();
    let levels: std::collections::BTreeSet<u8> = edge.iter().copied().collect();
    assert!(
        levels.len() * 3 >= edge.len().min(60) && levels.len() >= 20,
        "{} ring pixels in {} levels",
        edge.len(),
        levels.len()
    );
}

#[test]
fn only_what_changed_is_drawn_again_widened_by_the_filter_and_an_unchanged_frame_draws_nothing() {
    let mut renderer = headless::create_renderer().expect("headless renderer");
    let programs = Programs::compile_with(&mut renderer, false).expect("programs");
    let mut cache = ElementCache::default();
    let (frame, panel) = ((400, 240), (300, 180));
    let mut s = Supersample::default();
    s.dither = false;
    let mut kept = None;
    let draw = |s: &mut Supersample,
                renderer: &mut GlesRenderer,
                kept: &mut Option<_>,
                elements: &[CupertinoElement]| {
        let element = s
            .draw(
                renderer,
                &programs.resample,
                elements,
                frame,
                RENDER,
                panel,
                Color32F::BLACK,
            )
            .expect("draw");
        show(renderer, element, Fourcc::Abgr8888, panel, kept)
    };
    let small = |cache: &mut ElementCache, x: f64| {
        let spec = RoundedRectSpec::new(
            Rectangle::new((x, 20.0).into(), (10.0, 6.0).into()),
            2.0,
            [1.0, 0.5, 0.2, 1.0],
        );
        CupertinoElement::Shader(rounded_rect(
            &programs,
            cache,
            RENDER,
            ElementKey::new("small", 0),
            &spec,
        ))
    };
    let mut scene = shapes(&programs, &mut cache, 0.0);
    scene.insert(0, small(&mut cache, 120.0));
    let (first, damage) = draw(&mut s, &mut renderer, &mut kept, &scene);
    assert_eq!(
        damage,
        Some(vec![Rectangle::from_size(panel.into())]),
        "the first frame is drawn whole"
    );
    // nothing changed: nothing is drawn, the frame stays
    let mut scene = shapes(&programs, &mut cache, 0.0);
    scene.insert(0, small(&mut cache, 120.0));
    let (same, damage) = draw(&mut s, &mut renderer, &mut kept, &scene);
    assert_eq!(damage, None, "an unchanged frame draws nothing");
    assert_eq!(same.as_raw(), first.as_raw());
    // the small rectangle moves: what it covered before and after, at 2, reaches the display widened by a pixel
    let mut scene = shapes(&programs, &mut cache, 0.0);
    scene.insert(0, small(&mut cache, 140.0));
    let before = scene[0].geometry(Scale::from(RENDER));
    let (moved, damage) = draw(&mut s, &mut renderer, &mut kept, &scene);
    let damage = damage.expect("the move is drawn");
    let area: i32 = damage.iter().map(|r| r.size.w * r.size.h).sum();
    assert!(
        area < panel.0 * panel.1 / 8,
        "only the moved rectangle's area is drawn: {damage:?}"
    );
    let reach = panel_damage(&[before], frame, panel);
    for r in &reach {
        assert!(
            damage.iter().any(|d| d.contains_rect(*r)),
            "{r:?} of the new place is drawn: {damage:?}"
        );
    }
    // and what was drawn is what drawing the whole frame anew shows
    let whole = shrunk(&mut renderer, &programs, &scene, frame, panel);
    assert_eq!(worst(&moved, &whole).0, 0);
}

#[test]
fn a_ten_bit_display_gets_ten_bits_from_the_frame_and_the_pass_dithers_at_the_display_s_step() {
    let mut renderer = headless::create_renderer().expect("headless renderer");
    let programs = Programs::compile_with(&mut renderer, false).expect("programs");
    let mut cache = ElementCache::default();
    let (frame, panel) = ((64, 64), (48, 48));
    // a flat grey half way between two 8-bit steps
    let grey = 100.5 / 255.0;
    let flat = |cache: &mut ElementCache| {
        vec![CupertinoElement::Shader(rounded_rect(
            &programs,
            cache,
            RENDER,
            ElementKey::new("flat", 0),
            &RoundedRectSpec::new(
                Rectangle::from_size((32.0, 32.0).into()),
                0.0,
                [grey, grey, grey, 1.0],
            ),
        ))]
    };
    // into a 10-bit display, undithered: the 10-bit value of the grey, which 8 bits cannot hold
    let mut s = Supersample::default();
    s.dither = false;
    s.depth = 10;
    let element = s
        .draw(
            &mut renderer,
            &programs.resample,
            &flat(&mut cache),
            frame,
            RENDER,
            panel,
            Color32F::BLACK,
        )
        .expect("draw");
    let mut target: GlesTexture = match renderer.create_buffer(Fourcc::Abgr2101010, panel.into()) {
        Ok(t) => t,
        Err(e) => panic!("a 10-bit target: {e}"),
    };
    let mut tracker = OutputDamageTracker::new(panel, SCALE, Transform::Normal);
    let words: Vec<u32> = {
        let mut fb = renderer.bind(&mut target).expect("bind");
        tracker
            .render_output(
                &mut renderer,
                &mut fb,
                0,
                &[CupertinoElement::Resampled(element)],
                Color32F::BLACK,
            )
            .expect("render");
        let mapping = renderer
            .copy_framebuffer(&fb, Rectangle::from_size(panel.into()), Fourcc::Abgr2101010)
            .expect("read back 10 bits");
        drop(fb);
        let bytes = renderer.map_texture(&mapping).expect("map");
        bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| u32::from_le_bytes(*b))
            .collect()
    };
    let want = (grey * 1023.0).round() as i32;
    for w in &words {
        let red = (w & 0x3ff) as i32;
        assert!((red - want).abs() <= 1, "{red}, not {want}");
    }
    // into an 8-bit display, dithered: the pixels spread over the two steps about the grey, half and half
    let mut s = Supersample::default();
    s.dither = true;
    s.depth = 8;
    let element = s
        .draw(
            &mut renderer,
            &programs.resample,
            &flat(&mut cache),
            frame,
            RENDER,
            panel,
            Color32F::BLACK,
        )
        .expect("draw");
    let (dithered, _) = show(&mut renderer, element, Fourcc::Abgr8888, panel, &mut None);
    let low = dithered.pixels().filter(|p| p.0[0] == 100).count();
    let high = dithered.pixels().filter(|p| p.0[0] == 101).count();
    assert_eq!(low + high, (panel.0 * panel.1) as usize);
    let share = high as f64 / (low + high) as f64;
    assert!((0.35..0.65).contains(&share), "{low} at 100, {high} at 101");
    // undithered, every pixel rounds the same way
    let mut s = Supersample::default();
    s.dither = false;
    let element = s
        .draw(
            &mut renderer,
            &programs.resample,
            &flat(&mut cache),
            frame,
            RENDER,
            panel,
            Color32F::BLACK,
        )
        .expect("draw");
    let (plain, _) = show(&mut renderer, element, Fourcc::Abgr8888, panel, &mut None);
    let first = plain.get_pixel(0, 0).0[0];
    assert!(plain.pixels().all(|p| p.0[0] == first));
}
