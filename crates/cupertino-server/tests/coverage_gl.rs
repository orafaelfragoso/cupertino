//! The compositor's rounded rectangles are the shared coverage (`cupertino_raster_math::coverage`): drawn on the GL
//! path, every pixel's alpha is the coverage of its centre's distance to the outline, for the shapes of the coverage
//! vectors with circular corners and for the continuous corner the shell draws, at scales 1, 1.5 and 2; and the
//! passes of one frame read the context's state once and draw what they draw one by one; and the chrome drawn on the
//! GPU from its list of draws is the list painted on the CPU, shown in frames of 8 and of 10 bits a channel alike.

use cupertino_assets::Assets;
use cupertino_decorations::{
    ButtonKind, DecorationConfig, DecorationState, Decorations, TitlebarStyle,
};
use cupertino_geometry::Size;
use cupertino_raster_math::coverage::{coverage, rounded_distance, stroke_coverage};
use cupertino_raster_math::dither::quantize;
use cupertino_raster_math::vectors;
use cupertino_server::backend::headless;
use cupertino_server::render::chrome::{self, ChromeImage, Resolved};
use cupertino_server::render::programs::{corner_uniforms, Programs};
use cupertino_server::render::screenshot::render_to_image;
use cupertino_server::render::shader_rect::{ElementCache, ElementKey};
use cupertino_server::render::ui::{rounded_rect, RoundedRectSpec};
use cupertino_server::render::uploads::Drawer;
use cupertino_server::render::CupertinoElement;
use cupertino_theme::Appearance;
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::element::texture::TextureRenderElement;
use smithay::backend::renderer::element::utils::RescaleRenderElement;
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::gles::{GlesRenderer, GlesTexture};
use smithay::backend::renderer::{Bind, Color32F, ExportMem, Offscreen};
use smithay::utils::{Rectangle, Transform};

/// Where the shape's top-left corner lies, in points.
const AT: f64 = 3.0;

/// Draws `spec`'s shape (radius `radius`, circular or continuous, with an inside border of `border` points) at `scale`
/// and checks every pixel's alpha against the definition: within one step (a driver may round a hair either way at
/// half a step) and the definition's own value for nearly every pixel.
fn check(half: [f64; 2], radius: f64, continuous: bool, border: f64, scale: f64) {
    let mut renderer = headless::create_renderer().expect("headless renderer");
    let programs = Programs::compile_with(&mut renderer, false).expect("programs");
    let mut cache = ElementCache::default();
    let rect = Rectangle::new((AT, AT).into(), (half[0] * 2.0, half[1] * 2.0).into());
    let mut spec = RoundedRectSpec::new(rect, radius, [1.0, 1.0, 1.0, 1.0]);
    spec.continuous = continuous;
    if border > 0.0 {
        spec.fill = [1.0, 1.0, 1.0, 0.0];
        spec.border = Some((border, [1.0, 1.0, 1.0, 1.0]));
    }
    let element = rounded_rect(
        &programs,
        &mut cache,
        scale,
        ElementKey::new("coverage", 0),
        &spec,
    );
    let size = (
        ((half[0] * 2.0 + 2.0 * AT) * scale).ceil() as i32,
        ((half[1] * 2.0 + 2.0 * AT) * scale).ceil() as i32,
    );
    let image = render_to_image(
        &mut renderer,
        &[element],
        size.into(),
        scale,
        Color32F::TRANSPARENT,
    )
    .expect("render");
    let (extent, exponent) = corner_uniforms(radius, continuous, 1.0);
    let extent = extent.min(half[0].min(half[1]) as f32);
    let (mut exact, mut total) = (0, 0);
    for (x, y, px) in image.enumerate_pixels() {
        let p = [
            ((f64::from(x) + 0.5) / scale - AT - half[0]) as f32,
            ((f64::from(y) + 0.5) / scale - AT - half[1]) as f32,
        ];
        let d = rounded_distance(p, [half[0] as f32, half[1] as f32], extent, exponent);
        let s = scale as f32;
        let cover = if border > 0.0 {
            stroke_coverage(d, border as f32, s)
        } else {
            coverage(d, s)
        };
        let want = quantize(cover);
        let got = px.0[3];
        assert!(
            got.abs_diff(want) <= 1,
            "{half:?} r {radius} continuous {continuous} border {border} at {scale}: ({x}, {y}) is {got}, the \
             definition {want}"
        );
        exact += usize::from(got == want);
        total += 1;
    }
    assert!(
        exact * 100 >= total * 95,
        "{exact} of {total} pixels are exactly the definition's"
    );
}

#[test]
fn rounded_rectangles_are_the_shared_coverage_at_every_scale() {
    // the shapes of the coverage vectors with circular corners, at the vectors' scales
    let mut shapes: Vec<([f64; 2], f64, f64)> = vectors::rows(vectors::COVERAGE)
        .iter()
        .filter(|r| r[5] <= 2.001)
        .map(|r| ([r[2], r[3]], r[4], r[6]))
        .collect();
    shapes.dedup();
    assert!(shapes.len() >= 6, "{shapes:?}");
    for (half, extent, scale) in shapes {
        check(half, extent, false, 0.0, scale);
    }
    // the continuous corner and an inside border
    for scale in [1.0, 1.5, 2.0] {
        check([30.0, 16.0], 10.0, true, 0.0, scale);
        check([30.0, 16.0], 8.0, false, 1.0, scale);
    }
}

#[test]
fn the_passes_of_a_frame_read_the_context_s_state_once_and_draw_what_they_draw_entered_one_by_one()
{
    let mut renderer = headless::create_renderer().expect("headless renderer");
    let programs = Programs::compile_with(&mut renderer, false).expect("programs");
    let mut cache = ElementCache::default();
    // four overlapping rounded rectangles with a border, each its own pass draw
    let elements: Vec<_> = (0..4u8)
        .map(|i| {
            let at = 2.0 + 5.0 * f64::from(i);
            let rect = Rectangle::new((at, at).into(), (24.0, 14.0).into());
            let mut spec = RoundedRectSpec::new(rect, 6.0, [0.2 * f32::from(i), 0.5, 0.8, 0.7]);
            spec.border = Some((1.0, [1.0, 1.0, 1.0, 1.0]));
            rounded_rect(
                &programs,
                &mut cache,
                1.5,
                ElementKey::new("frame", u64::from(i)),
                &spec,
            )
        })
        .collect();
    let draw = |renderer: &mut GlesRenderer| {
        render_to_image(
            renderer,
            &elements,
            (64, 56).into(),
            1.5,
            Color32F::new(0.1, 0.1, 0.1, 1.0),
        )
        .expect("render")
    };
    let reads = programs.passes.state_reads();
    let one_by_one = draw(&mut renderer);
    assert_eq!(
        programs.passes.state_reads() - reads,
        4,
        "outside a frame each pass reads the state"
    );
    let reads = programs.passes.state_reads();
    let in_frame = {
        let _frame = programs.passes.frame();
        draw(&mut renderer)
    };
    assert_eq!(
        programs.passes.state_reads() - reads,
        1,
        "inside one frame the first pass reads it"
    );
    assert_eq!(in_frame.as_raw(), one_by_one.as_raw());
    // and each pass reads it again once the frame is over
    let reads = programs.passes.state_reads();
    assert_eq!(draw(&mut renderer).as_raw(), one_by_one.as_raw());
    assert_eq!(programs.passes.state_reads() - reads, 4);
}

/// The element that shows the chrome's list drawn on the GPU (`chrome`) at one pixel a texel.
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
        1.0,
    ))
}

/// The chrome's list drawn on the GPU (`chrome`) and shown through its element at one pixel a texel: what the
/// output shows of it.
fn drawn(
    renderer: &mut GlesRenderer,
    programs: &Programs,
    image: &ChromeImage,
) -> image::RgbaImage {
    let element = chrome_element(renderer, programs, image);
    let (w, h) = image.size();
    render_to_image(
        renderer,
        &[element],
        (w, h).into(),
        1.0,
        Color32F::TRANSPARENT,
    )
    .expect("render")
}

/// The chrome's list drawn on the GPU and shown over an opaque `ground` in a frame of `format`, as a display's plane
/// of that format holds it, read back 8 bits a channel.
fn shown_in(
    renderer: &mut GlesRenderer,
    programs: &Programs,
    image: &ChromeImage,
    format: Fourcc,
    ground: [f32; 4],
) -> image::RgbaImage {
    let element = chrome_element(renderer, programs, image);
    let (w, h) = image.size();
    let mut frame: GlesTexture = renderer
        .create_buffer(format, (w, h).into())
        .unwrap_or_else(|e| panic!("a frame of {format}: {e}"));
    let mut tracker = OutputDamageTracker::new((w, h), 1.0, Transform::Normal);
    let mapping = {
        let mut target = renderer.bind(&mut frame).expect("bind the frame");
        tracker
            .render_output(
                renderer,
                &mut target,
                0,
                &[element],
                Color32F::new(ground[0], ground[1], ground[2], ground[3]),
            )
            .expect("render");
        renderer
            .copy_framebuffer(
                &target,
                Rectangle::from_size((w, h).into()),
                Fourcc::Abgr8888,
            )
            .expect("read the frame back")
    };
    let bytes = renderer.map_texture(&mapping).expect("map the frame");
    image::RgbaImage::from_raw(w as u32, h as u32, bytes.to_vec()).expect("the frame's size")
}

/// `painted` (premultiplied) laid over the opaque `ground` and stored in 8 bits: what a frame shows of it.
fn over_ground(painted: &tiny_skia::Pixmap, ground: [f32; 4]) -> tiny_skia::Pixmap {
    let mut out = painted.clone();
    for px in out.data_mut().as_chunks_mut::<4>().0 {
        let keep = 1.0 - f32::from(px[3]) / 255.0;
        for c in 0..3 {
            px[c] = quantize(f32::from(px[c]) / 255.0 + ground[c] * keep);
        }
        px[3] = 255;
    }
    out
}

/// The largest difference of any channel of any pixel between `got` and `want` (premultiplied RGBA both), and where.
fn worst(got: &image::RgbaImage, want: &tiny_skia::Pixmap) -> (u8, (u32, u32)) {
    let mut worst = (0, (0, 0));
    for (x, y, px) in got.enumerate_pixels() {
        let i = ((y * want.width() + x) * 4) as usize;
        for c in 0..4 {
            let d = px.0[c].abs_diff(want.data()[i + c]);
            if d > worst.0 {
                worst = (d, (x, y));
            }
        }
    }
    worst
}

#[test]
fn the_chrome_drawn_on_the_gpu_is_its_list_painted_on_the_cpu_within_two_steps_and_drawn_again_only_when_it_changes(
) {
    let mut renderer = headless::create_renderer().expect("headless renderer");
    let programs = Programs::compile_with(&mut renderer, false).expect("programs");
    let assets = Assets::builtin();
    let states = [
        DecorationState {
            title: "Documents".into(),
            ..DecorationState::default()
        },
        DecorationState {
            title: "Documents".into(),
            group_hovered: true,
            ..DecorationState::default()
        },
        DecorationState {
            title: "Documents".into(),
            group_hovered: true,
            pressed: Some(ButtonKind::Close),
            ..DecorationState::default()
        },
        DecorationState {
            title: "Documents".into(),
            active: false,
            ..DecorationState::default()
        },
    ];
    for appearance in Appearance::ALL {
        for style in [TitlebarStyle::Standard, TitlebarStyle::Unified] {
            let mut d = Decorations::new(DecorationConfig::builtin(appearance, style), &assets)
                .expect("decorations");
            for scale in [1.0, 1.5, 2.0] {
                let mut lists = vec![("rim", d.rim_draws(Size::new(420.0, 260.0), scale))];
                for (i, state) in states.iter().enumerate() {
                    lists.push(("title bar", d.titlebar_draws(420.0, scale, state, false)));
                    if style == TitlebarStyle::Standard && i < 2 {
                        lists.push((
                            "flush title bar",
                            d.titlebar_draws(420.0, scale, state, true),
                        ));
                    }
                }
                for (i, (what, list)) in lists.into_iter().enumerate() {
                    let want = list.paint();
                    let image = ChromeImage::new(list, Drawer::Decoration);
                    let got = drawn(&mut renderer, &programs, &image);
                    let (diff, at) = worst(&got, &want);
                    assert!(
                        diff <= 2,
                        "{appearance:?} {style:?} {what} {i} at {scale}: {diff} steps apart at {at:?}"
                    );
                }
            }
        }
    }
    // an unchanged list is shown from what was drawn; a changed one is drawn again, into the same texture
    let mut d = Decorations::new(
        DecorationConfig::builtin(Appearance::Light, TitlebarStyle::Standard),
        &assets,
    )
    .expect("decorations");
    let image = ChromeImage::new(
        d.titlebar_draws(420.0, 1.5, &states[0], false),
        Drawer::Decoration,
    );
    let first = drawn(&mut renderer, &programs, &image);
    let before = chrome::counts();
    assert_eq!(
        drawn(&mut renderer, &programs, &image).as_raw(),
        first.as_raw()
    );
    assert_eq!(chrome::counts(), before, "nothing drawn again");
    let hovered = d.titlebar_draws(420.0, 1.5, &states[1], false);
    let want = hovered.paint();
    image.set(hovered);
    let again = drawn(&mut renderer, &programs, &image);
    assert_eq!(chrome::counts().drawn, before.drawn + 1);
    assert_eq!(
        chrome::counts().painted,
        before.painted,
        "nothing painted on the CPU"
    );
    assert!(worst(&again, &want).0 <= 2);
    assert_ne!(
        again.as_raw(),
        first.as_raw(),
        "the hovered buttons show their glyphs"
    );
}

#[test]
fn window_buttons_drawn_on_the_gpu_show_in_8_and_10_bit_frames_as_painted_on_the_cpu_with_smooth_edges(
) {
    use cupertino_decorations::coverage::{Outline, Shade};
    use cupertino_decorations::{Draw, DrawList};
    let mut renderer = headless::create_renderer().expect("headless renderer");
    let programs = Programs::compile_with(&mut renderer, false).expect("programs");
    let assets = Assets::builtin();
    let ground = [0.93, 0.9, 0.9, 1.0];
    let states = [
        DecorationState::default(),
        DecorationState {
            group_hovered: true,
            ..DecorationState::default()
        },
    ];
    // the title bar with its three buttons, at rest and hovered, laid over an opaque ground in a frame of 8 and of 10
    // bits a channel (whose alpha has 2 bits): what each frame shows is the CPU painting over that ground
    for appearance in Appearance::ALL {
        let mut d = Decorations::new(
            DecorationConfig::builtin(appearance, TitlebarStyle::Standard),
            &assets,
        )
        .expect("decorations");
        for scale in [1.0, 1.5, 2.0] {
            for state in &states {
                let list = d.titlebar_draws(420.0, scale, state, false);
                let want = over_ground(&list.paint(), ground);
                let image = ChromeImage::new(list, Drawer::Decoration);
                for format in [Fourcc::Abgr8888, Fourcc::Abgr2101010] {
                    let got = shown_in(&mut renderer, &programs, &image, format, ground);
                    let (diff, at) = worst(&got, &want);
                    assert!(
                        diff <= 2,
                        "{appearance:?} hovered {} at {scale} in {format}: {diff} steps apart at {at:?}",
                        state.group_hovered
                    );
                }
            }
        }
    }
    // a white disc the size of a button, off the pixel grid, over black: each edge pixel shows the share of it the
    // disc covers, in many levels, in either frame
    for scale in [1.0f32, 1.5, 2.0] {
        let side = (32.0 * scale) as u32;
        let centre = [16.0 * scale + 0.3, 12.0 * scale + 0.1];
        let mut list = DrawList::new(side, side);
        list.draws.push(Draw::Shape {
            outline: Outline::circle(centre, 7.0 * scale),
            shade: Shade::Solid([1.0; 4]),
            stroke: None,
        });
        let want = over_ground(&list.paint(), [0.0, 0.0, 0.0, 1.0]);
        let image = ChromeImage::new(list, Drawer::Decoration);
        for format in [Fourcc::Abgr8888, Fourcc::Abgr2101010] {
            let got = shown_in(
                &mut renderer,
                &programs,
                &image,
                format,
                [0.0, 0.0, 0.0, 1.0],
            );
            let (diff, at) = worst(&got, &want);
            assert!(
                diff <= 1,
                "at {scale} in {format}: {diff} steps apart at {at:?}"
            );
            let edge: Vec<u8> = got
                .pixels()
                .map(|p| p.0[0])
                .filter(|&v| v > 0 && v < 255)
                .collect();
            let levels: std::collections::BTreeSet<u8> = edge.iter().copied().collect();
            assert!(
                levels.len() * 3 >= edge.len(),
                "at {scale} in {format}: {} edge pixels in only {} levels: {levels:?}",
                edge.len(),
                levels.len()
            );
        }
    }
}

#[test]
fn a_hud_s_rounded_fills_and_glyph_runs_drawn_on_the_gpu_are_their_list_painted_on_the_cpu() {
    use cupertino_decorations::shape::rounded_path;
    use cupertino_decorations::text::TextRenderer;
    use cupertino_decorations::{Draw, DrawList};
    use cupertino_geometry::{CornerStyle, Radii, Rect};
    let mut renderer = headless::create_renderer().expect("headless renderer");
    let programs = Programs::compile_with(&mut renderer, false).expect("programs");
    let assets = Assets::builtin();
    let mut text = TextRenderer::new(&assets, cupertino_assets::FontRole::Ui).expect("text");
    // a panel of two rows, the second highlighted, each a source badge and a label, as the input-source HUD lays them
    for (dark, scale) in [(false, 1.0), (true, 1.5), (false, 2.0), (true, 2.0)] {
        text.set_scale(scale as f32);
        let (fg, highlight) = if dark {
            ([0.92, 0.92, 0.94, 1.0], [0.25, 0.45, 0.95, 0.9])
        } else {
            ([0.1, 0.1, 0.12, 1.0], [0.2, 0.42, 0.92, 0.85])
        };
        let mut list = DrawList::new((240.0 * scale) as u32, (80.0 * scale) as u32);
        let fill = |r: Rect, radius: f64, color| Draw::Path {
            path: rounded_path(r, Radii::uniform(radius), CornerStyle::continuous(), scale)
                .expect("a path"),
            stroke: None,
            color,
        };
        list.draws.push(fill(
            Rect::new(0.0, 0.0, 240.0, 80.0),
            18.0,
            [0.97, 0.97, 0.98, 0.94],
        ));
        for row in 0..2 {
            let y = 8.0 + 32.0 * f64::from(row);
            let color = if row == 1 { [1.0; 4] } else { fg };
            if row == 1 {
                list.draws
                    .push(fill(Rect::new(8.0, y, 224.0, 32.0), 8.0, highlight));
            }
            list.draws.push(fill(
                Rect::new(18.0, y + 4.0, 24.0, 24.0),
                6.0,
                [color[0], color[1], color[2], 0.14],
            ));
            let baseline = ((y + 20.0) * scale) as f32;
            for (word, x, px) in [("AB", 21.5, 10.0), ("Keyboard layout", 52.0, 13.0)] {
                let px = (px * scale) as f32;
                list.draws.push(Draw::Glyphs {
                    glyphs: text.place(word, (x * scale) as f32, baseline, px, 400, 0.0, color),
                    color,
                });
            }
        }
        let want = list.paint();
        let image = ChromeImage::new(list, Drawer::Hud);
        let got = drawn(&mut renderer, &programs, &image);
        let (diff, at) = worst(&got, &want);
        assert!(
            diff <= 2,
            "dark {dark} at {scale}: {diff} steps apart at {at:?}"
        );
    }
    assert_eq!(
        chrome::counts().hud_painted,
        0,
        "nothing painted on the CPU"
    );
    assert!(chrome::counts().hud_drawn >= 4);
}
