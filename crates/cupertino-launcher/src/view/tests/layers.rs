//! The panel drawn as compositor layers ([`crate::view::layers`]): at rest its layers carry the pixels it draws in
//! place and the glass it stands on, and its close runs on the compositor's clock without a frame of the panel's.

use super::*;
use crate::motion::CLOSE_DURATION;
use crate::view::layers::{
    button_face_layer, button_glass_layer, CONTENT_LAYER, EDGE_LAYER, GLASS_LAYER,
};
use cupertino_kit::platform::{LayerProperty, SurfaceLayerLook};
use cupertino_ui_test::PresentedSurfaceLayer;

/// The launcher drawing its frames at `scale` in the `dark` or light appearance, on a window system that shows
/// surface layers when `layers`.
fn drawn(scale: f64, dark: bool, layers: bool) -> Metered {
    LAYERS.set(layers);
    DARK.set(dark);
    let mut m = Metered::new(scale, true);
    LAYERS.set(false);
    DARK.set(false);
    m.driver.window_system_mut().set_keep_pixels(true);
    m
}

impl Metered {
    /// The panel's last window frame, `(width, height, pixels)`.
    fn window_pixels(&self) -> (u32, u32, Vec<u8>) {
        let panel = self.panel().expect("the panel is shown");
        let frames = self.driver.runtime().window_system().frames(panel).unwrap();
        let last = frames
            .iter()
            .rev()
            .find(|f| !f.pixels.is_empty())
            .expect("the panel drew a frame");
        (last.width, last.height, last.pixels.clone())
    }

    /// The layers the panel shows now, each with the last frame it drew.
    fn shown_layers(&self) -> Vec<PresentedSurfaceLayer> {
        let panel = self.panel().expect("the panel is shown");
        let lists = self
            .driver
            .runtime()
            .window_system()
            .surface_layers(panel)
            .unwrap();
        let Some(now) = lists.last() else {
            return Vec::new();
        };
        now.iter()
            .map(|l| {
                let mut l = l.clone();
                if l.frame.is_none() {
                    l.frame = lists
                        .iter()
                        .rev()
                        .flat_map(|list| list.iter())
                        .find(|o| o.key == l.key && o.frame.is_some())
                        .and_then(|o| o.frame.clone());
                }
                l
            })
            .collect()
    }

    /// The panel's window frame with its layers' frames laid over it where they stand, as a compositor shows them
    /// without its glass.
    fn composed(&self) -> (u32, u32, Vec<u8>) {
        let (w, h, mut out) = self.window_pixels();
        for layer in self.shown_layers() {
            let frame = layer.frame.expect("every shown layer drew a frame");
            let (x0, y0) = (
                (layer.rect.x() * self.scale).round() as i64,
                (layer.rect.y() * self.scale).round() as i64,
            );
            for y in 0..i64::from(frame.pixel_height) {
                for x in 0..i64::from(frame.width) {
                    let (tx, ty) = (x0 + x, y0 + y);
                    if tx < 0 || ty < 0 || tx >= i64::from(w) || ty >= i64::from(h) {
                        continue;
                    }
                    let s =
                        &frame.pixels[(y as usize) * frame.stride as usize + x as usize * 4..][..4];
                    let d = &mut out[(ty as usize * w as usize + tx as usize) * 4..][..4];
                    let a = u32::from(s[3]);
                    for c in 0..4 {
                        d[c] = (u32::from(s[c]) + u32::from(d[c]) * (255 - a) / 255).min(255) as u8;
                    }
                }
            }
        }
        (w, h, out)
    }

    /// Draws the panel whole once more at its scale, by way of another scale, so what it shows is drawn afresh and
    /// not kept from the frames of a transition.
    fn repaint_whole(&mut self) {
        let panel = self.panel().expect("the panel is shown");
        for scale in [self.scale + 0.25, self.scale] {
            self.driver
                .window_system_mut()
                .enqueue_window(panel, WindowEvent::Scale(scale))
                .unwrap();
            self.step(|_, _| {});
        }
        self.until_quiet(4, 200);
    }

    /// The panel's frame and layer presents so far: `(window frames, layer frames)`.
    fn presents(&self) -> (usize, usize) {
        let panel = self.panel().expect("the panel is shown");
        let system = self.driver.runtime().window_system();
        let windows = system.frames(panel).unwrap().len();
        let layers = system
            .surface_layers(panel)
            .unwrap()
            .iter()
            .flatten()
            .filter(|l| l.frame.is_some())
            .count();
        (windows, layers)
    }

    /// The layer changes the panel asked of the compositor so far.
    fn layer_requests(&self) -> Vec<(u64, Vec<cupertino_kit::platform::LayerChange>)> {
        let panel = self.panel().expect("the panel is shown");
        self.driver
            .runtime()
            .window_system()
            .requests(panel)
            .unwrap()
            .iter()
            .filter_map(|r| match r {
                WindowRequest::Layer { key, changes } => Some((*key, changes.clone())),
                _ => None,
            })
            .collect()
    }
}

/// How far two frames `width` pixels wide differ: the largest difference of a channel, and the pixels that differ by
/// more than 1.
fn compare(a: &[u8], b: &[u8], width: u32) -> (u8, Vec<(u32, u32)>) {
    let mut most = 0;
    let mut stray = Vec::new();
    for (i, (p, q)) in a.chunks(4).zip(b.chunks(4)).enumerate() {
        let (x, y) = (i as u32 % width, i as u32 / width);
        let d = p
            .iter()
            .zip(q)
            .map(|(u, v)| u.abs_diff(*v))
            .max()
            .unwrap_or(0);
        most = most.max(d);
        if d > 1 {
            stray.push((x, y));
        }
    }
    (most, stray)
}

/// What the panel shows at rest at each step of [`walk`]: its name, the window's frame (with the layers laid over it)
/// and the layers' keys and glass.
struct Rest {
    what: &'static str,
    size: (u32, u32),
    pixels: Vec<u8>,
    keys: Vec<u64>,
    glass: Vec<(Rect, SurfaceLayerLook)>,
    /// The glass the window stands on itself.
    regions: Vec<cupertino_kit::platform::MaterialRegion>,
}

/// The panel at rest in the search field, its results and the browser, at `scale` in the `dark` or light appearance,
/// drawn as layers when `layers`.
fn walk(scale: f64, dark: bool, layers: bool) -> Vec<Rest> {
    let mut m = drawn(scale, dark, layers);
    let mut out = Vec::new();
    let take = |m: &mut Metered, what| {
        m.repaint_whole();
        let (w, h, pixels) = m.composed();
        let shown = m.shown_layers();
        let panel = m.panel().unwrap();
        let regions = m
            .driver
            .runtime()
            .window_system()
            .requests(panel)
            .unwrap()
            .iter()
            .rev()
            .find_map(|r| match r {
                WindowRequest::Materials(regions) => Some(regions.clone()),
                _ => None,
            })
            .unwrap_or_default();
        Rest {
            what,
            size: (w, h),
            pixels,
            keys: shown.iter().map(|l| l.key).collect(),
            glass: shown
                .iter()
                .filter(|l| l.look.glass.is_some())
                .map(|l| (l.rect, l.look))
                .collect(),
            regions,
        }
    };
    m.settle_on(Message::Toggle);
    out.push(take(&mut m, "the search field"));
    for c in "te".chars() {
        m.step(|s, w| key(s, w, c as u32, Some(c), Mods::NONE));
    }
    m.until_quiet(4, 200);
    out.push(take(&mut m, "the results"));
    m.step(|s, w| key(s, w, ESCAPE, None, Mods::NONE));
    m.until_quiet(4, 200);
    m.step(|s, w| key(s, w, '1' as u32, Some('1'), Mods::PRIMARY));
    m.until_quiet(4, 200);
    assert_eq!(m.model().nav.view, View::Browse);
    out.push(take(&mut m, "the browser"));
    out
}

/// The panel at rest in each view, drawn in place and drawn as layers: the layers laid over the window's frame are
/// the window's frame drawn in place, pixel for pixel, at whole and fractional scales and in both appearances; and
/// the glass the layers carry is the glass the panel stands on in place, the window standing on none of its own.
#[test]
fn a_resting_panel_s_layers_carry_the_pixels_and_the_glass_it_draws_in_place() {
    for scale in [1.0, 1.5, 2.0] {
        for dark in [false, true] {
            let flat = walk(scale, dark, false);
            let layered = walk(scale, dark, true);
            for (a, b) in flat.iter().zip(&layered) {
                let what = a.what;
                assert!(a.keys.is_empty(), "{what}: drawn in place");
                assert!(
                    b.keys.contains(&GLASS_LAYER) && b.keys.contains(&CONTENT_LAYER),
                    "{what} at {scale}x: {:?}",
                    b.keys
                );
                assert_eq!(a.size, b.size, "{what} at {scale}x");
                let (most, stray) = compare(&a.pixels, &b.pixels, a.size.0);
                assert!(
                    most <= 1 && stray.is_empty(),
                    "{what} at {scale}x, dark {dark}: the layers differ from the panel by {most} at {stray:?}"
                );
                assert!(
                    b.regions.is_empty(),
                    "{what}: the window stands on no glass of its own"
                );
                assert_eq!(
                    a.regions.len(),
                    b.glass.len(),
                    "{what}: {:?} {:?}",
                    a.regions,
                    b.glass
                );
                for (region, (rect, look)) in a.regions.iter().zip(&b.glass) {
                    assert_eq!(region.rect, *rect, "{what}");
                    assert_eq!(region.glass.map(|g| g.style), look.glass, "{what}");
                    assert_eq!(region.radius, look.radius, "{what}");
                    assert_eq!(
                        region.glass.map(|g| (g.container, g.spacing)),
                        Some(look.group),
                        "{what}"
                    );
                }
            }
            assert_eq!(
                layered[0].keys,
                vec![
                    GLASS_LAYER,
                    CONTENT_LAYER,
                    EDGE_LAYER,
                    button_glass_layer(0),
                    button_face_layer(0)
                ],
                "the field's glass, content and edge, then the button's glass and face"
            );
        }
    }
}

/// Closing the resting panel hands the close to the compositor: its layers grow, blur and fade on the close's curve,
/// and while it runs the panel draws no frame, of its window or of a layer, and asks for none; once it is over the
/// window goes.
#[test]
fn a_resting_panel_closes_on_the_compositor_s_clock_without_a_frame() {
    let mut m = drawn(1.5, true, true);
    m.settle_on(Message::Toggle);
    let before = m.presents();
    let requests = m.layer_requests().len();
    assert!(m.model().layered(), "the resting panel is drawn as layers");
    // Escape on the field closes it, as the person does
    m.step(|s, w| key(s, w, ESCAPE, None, Mods::NONE));
    assert!(!m.model().nav.is_visible(), "the panel closes");
    assert!(m.model().close_handed(), "the close is the compositor's");
    let asked = m.layer_requests();
    let close = &asked[requests..];
    let keys: Vec<u64> = close.iter().map(|(k, _)| *k).collect();
    assert_eq!(
        keys,
        vec![
            GLASS_LAYER,
            EDGE_LAYER,
            CONTENT_LAYER,
            button_glass_layer(0),
            button_face_layer(0)
        ]
    );
    for (key, changes) in close {
        let fade = changes
            .iter()
            .find(|c| c.property == LayerProperty::Opacity)
            .unwrap_or_else(|| panic!("layer {key} fades"));
        assert_eq!(fade.to, 0.0);
        assert!(matches!(
            fade.curve,
            Some(cupertino_kit::platform::LayerCurve::Timed { duration, .. }) if duration == CLOSE_DURATION
        ));
    }
    // a tenth of a second of frames: none drawn, none asked for
    for _ in 0..8 {
        m.step(|_, _| {});
        assert!(!m.model().animating(), "the close asks for no frame");
    }
    assert_eq!(m.presents(), before, "no frame of the window or its layers");
    send(Message::CloseOver);
    m.step(|_, _| {});
    assert!(
        m.panel().is_none(),
        "the panel's window went with the close"
    );
}

/// Under Reduce Motion nothing is handed over: the panel is drawn in place and fades out itself.
#[test]
fn under_reduce_motion_the_panel_draws_its_own_close() {
    let mut m = drawn(1.5, true, true);
    m.settle_on(Message::ReduceMotion(true));
    m.settle_on(Message::Toggle);
    assert!(m.shown_layers().is_empty(), "drawn in place");
    send(Message::Toggle);
    m.step(|_, _| {});
    assert!(!m.model().close_handed());
    assert!(m.layer_requests().is_empty());
}

/// A close that falls in the caret's blink draws nothing: while the compositor runs it the field holds its caret, so
/// the caret's fade, which starts inside the close, draws no frame of the content's layer.
#[test]
fn a_close_in_the_caret_s_fade_draws_no_frame() {
    // how many frames after the panel rests its caret starts to fade, which it draws
    let fade = {
        let mut m = drawn(1.5, true, true);
        m.settle_on(Message::Toggle);
        let rest = m.presents();
        (1..120)
            .find(|_| {
                m.step(|_, _| {});
                m.presents() != rest
            })
            .expect("the caret fades")
    };
    let mut m = drawn(1.5, true, true);
    m.settle_on(Message::Toggle);
    for _ in 2..fade {
        m.step(|_, _| {});
    }
    let shown = m.presents();
    // two frames before the fade, closed by the launcher's action, not a key, so the blink goes on
    send(Message::Toggle);
    m.step(|_, _| {});
    assert!(m.model().close_handed(), "the close is the compositor's");
    for _ in 0..6 {
        m.step(|_, _| {});
    }
    assert_eq!(m.presents(), shown, "no frame of the window or its layers");
}

/// The panel at rest in the search field, its results and the browser, drawn as layers at `scale` in the `dark` or
/// light appearance: each view's name, window size and the layers it shows with their frames.
fn resting_layers(
    scale: f64,
    dark: bool,
) -> Vec<(&'static str, (u32, u32), Vec<PresentedSurfaceLayer>)> {
    let mut m = drawn(scale, dark, true);
    let mut out = Vec::new();
    let take = |m: &mut Metered, what| {
        m.repaint_whole();
        let (w, h, _) = m.window_pixels();
        (what, (w, h), m.shown_layers())
    };
    m.settle_on(Message::Toggle);
    out.push(take(&mut m, "the search field"));
    for c in "te".chars() {
        m.step(|s, w| key(s, w, c as u32, Some(c), Mods::NONE));
    }
    m.until_quiet(4, 200);
    out.push(take(&mut m, "the results"));
    m.step(|s, w| key(s, w, ESCAPE, None, Mods::NONE));
    m.until_quiet(4, 200);
    m.step(|s, w| key(s, w, '1' as u32, Some('1'), Mods::PRIMARY));
    m.until_quiet(4, 200);
    out.push(take(&mut m, "the browser"));
    out
}

/// The edge a compositor draws along the glass of `layers` with `edge` in each glass group's first layer's look, in a
/// window `size` pixels large at `scale`: for each pixel, its alpha and how far its centre lies outside the glass, in
/// pixels, as the glass's own model of the edge works them out in device pixels.
fn compositor_edge(
    size: (u32, u32),
    scale: f64,
    layers: &[PresentedSurfaceLayer],
    edge: cupertino_kit::platform::LayerEdge,
) -> Vec<(f32, f32)> {
    use cupertino_glass::glass::{Container, GlassEdge, GlassMode, GlassShape};
    let s = scale as f32;
    let glass: Vec<&PresentedSurfaceLayer> =
        layers.iter().filter(|l| l.look.glass.is_some()).collect();
    let mut groups: Vec<(u32, f64, Vec<GlassShape>)> = Vec::new();
    for l in &glass {
        let r = l.rect;
        let shape = GlassShape::new(
            [
                r.x() as f32 * s,
                r.y() as f32 * s,
                r.width() as f32 * s,
                r.height() as f32 * s,
            ],
            l.look.radius as f32 * s,
        );
        let (group, spacing) = l.look.group;
        match groups.iter_mut().find(|g| group != 0 && g.0 == group) {
            Some(g) => g.2.push(shape),
            None => groups.push((group, spacing, vec![shape])),
        }
    }
    let containers: Vec<Container> = groups
        .into_iter()
        .map(|(_, spacing, shapes)| {
            Container::new(shapes, spacing as f32 * s, GlassMode::default()).with_edge(GlassEdge {
                width: edge.width as f32 * s,
                color: edge.color,
            })
        })
        .collect();
    let (w, h) = (size.0 as usize, size.1 as usize);
    let mut out = vec![(0.0, f32::INFINITY); w * h];
    for (i, px) in out.iter_mut().enumerate() {
        let p = [(i % w) as f32 + 0.5, (i / w) as f32 + 0.5];
        for c in &containers {
            let d = c.field(p).0;
            let a = c.edge.color[3] * c.edge_share_at(d);
            *px = (1.0 - (1.0 - px.0) * (1.0 - a), px.1.min(d));
        }
    }
    out
}

/// The alpha the panel's own edges draw at each pixel of a window `size` pixels large at `scale`: its edge layer's and
/// its mode buttons' faces' frames laid where they stand.
fn drawn_edges(size: (u32, u32), scale: f64, layers: &[PresentedSurfaceLayer]) -> Vec<f32> {
    let (w, h) = (size.0 as i64, size.1 as i64);
    let mut out = vec![0.0f32; (w * h) as usize];
    for l in layers
        .iter()
        .filter(|l| l.key == EDGE_LAYER || (l.key >= 16 && l.key % 2 == 1))
    {
        let frame = l.frame.as_ref().expect("an edge layer drew a frame");
        let (x0, y0) = (
            (l.rect.x() * scale).round() as i64,
            (l.rect.y() * scale).round() as i64,
        );
        for y in 0..i64::from(frame.pixel_height) {
            for x in 0..i64::from(frame.width) {
                let (tx, ty) = (x0 + x, y0 + y);
                if tx < 0 || ty < 0 || tx >= w || ty >= h {
                    continue;
                }
                let a = f32::from(
                    frame.pixels[y as usize * frame.stride as usize + x as usize * 4 + 3],
                ) / 255.0;
                let d = &mut out[(ty * w + tx) as usize];
                *d = 1.0 - (1.0 - *d) * (1.0 - a);
            }
        }
    }
    out
}

/// The edge the compositor draws along the panel's glass is the edge the panel draws itself: at rest, at 1, 1.5 and 2
/// in both appearances, outside the glass (inside it the faces draw their symbols), the glass's own model of the edge
/// of [`crate::view::layers::glass_edge`] in each group's look, the search field and its mode button joined in one,
/// is the panel's edge layers within 4 of 255 along every capsule and every straight side. A scope's continuous
/// corners are not the circular arcs the panel strokes its edge along, which stand up to 0.57 points off them where
/// each corner starts to curve: there the compositor's edge follows the glass and the two part by up to the line's own
/// opacity, at the corners alone (`--no-capture` prints both).
#[test]
fn the_edge_the_compositor_draws_along_the_glass_is_the_edge_the_panel_draws() {
    for scale in [1.0, 1.5, 2.0] {
        for dark in [false, true] {
            for (what, size, layers) in resting_layers(scale, dark) {
                let model =
                    compositor_edge(size, scale, &layers, crate::view::layers::glass_edge(dark));
                let own = drawn_edges(size, scale, &layers);
                let enclosure = layers.iter().find(|l| l.key == GLASS_LAYER).unwrap();
                let (r, radius) = (enclosure.rect, enclosure.look.radius);
                // a corner's curve reaches this far along the sides of a continuous corner
                let reach = radius * 1.53 + 2.0;
                let corner = |x: f64, y: f64| {
                    let (dx, dy) = (
                        (x - r.x()).min(r.max_x() - x),
                        (y - r.y()).min(r.max_y() - y),
                    );
                    dx < reach && dy < reach
                };
                let capsule = radius * 2.0 >= r.height() - 1e-9;
                let (mut most, mut most_corner) = (0.0f32, 0.0f32);
                for (i, ((a, d), b)) in model.iter().zip(&own).enumerate() {
                    if *d < -1.0 {
                        continue;
                    }
                    let diff = (a - b).abs() * 255.0;
                    let (x, y) = (
                        (i % size.0 as usize) as f64 / scale,
                        (i / size.0 as usize) as f64 / scale,
                    );
                    if !capsule && corner(x, y) {
                        most_corner = most_corner.max(diff);
                    } else {
                        most = most.max(diff);
                    }
                }
                eprintln!(
                    "trace: edge, {what} at {scale}x, dark {dark}: largest difference {most:.1}, at the corners {most_corner:.1}"
                );
                assert!(
                    most <= 4.0,
                    "{what} at {scale}x, dark {dark}: the compositor's edge differs from the panel's by {most:.1}"
                );
                assert!(
                    capsule || most_corner > 4.0,
                    "{what}: the corners part as the arcs do"
                );
            }
        }
    }
}

/// The deviation below which a compositor draws a layer's content sharp, in device pixels.
const SHARP_BELOW_PX: f64 = 0.3;

/// One layer as a compositor shows it at one moment: its bounds, opacity, scale about the bounds' centre, corner
/// radius and content blur, with the frame it drew.
#[derive(Clone, Debug)]
struct ShownLayer {
    bounds: Rect,
    opacity: f64,
    scale: f64,
    radius: f64,
    blur: f64,
    glass: bool,
    frame: Option<cupertino_ui_test::PresentedLayer>,
}

/// The layers `rest` as the compositor shows them `after` seconds into the `changes` the panel asked for: each
/// property starts where the layer's layout puts it and follows its change on the compositor's own animator.
fn shown_after(
    rest: &[PresentedSurfaceLayer],
    changes: &[(u64, Vec<cupertino_kit::platform::LayerChange>)],
    after: f64,
) -> Vec<ShownLayer> {
    use cupertino_animation::layer::LayerProperty as Animated;
    let index = |p: LayerProperty| match p {
        LayerProperty::X => 0,
        LayerProperty::Y => 1,
        LayerProperty::Width => 2,
        LayerProperty::Height => 3,
        LayerProperty::Opacity => 4,
        LayerProperty::Scale => 5,
        LayerProperty::CornerRadius => 6,
        LayerProperty::ContentBlur => 7,
    };
    rest.iter()
        .map(|layer| {
            let r = layer.rect;
            let mut props = [
                r.x(),
                r.y(),
                r.width(),
                r.height(),
                1.0,
                1.0,
                layer.look.radius,
                0.0,
            ]
            .map(Animated::new);
            for (_, list) in changes.iter().filter(|(k, _)| *k == layer.key) {
                for change in list {
                    let p = &mut props[index(change.property)];
                    match (&change.samples, change.curve) {
                        (_, None) => p.set(change.to),
                        (
                            Some(samples),
                            Some(cupertino_kit::platform::LayerCurve::Timed { duration, .. }),
                        ) => p.animate_samples(samples.clone(), duration, 0.0),
                        (_, Some(curve)) => p.animate(change.to, curve, 0.0, change.velocity),
                    }
                }
            }
            let v = |i: usize| props[i].value(after);
            ShownLayer {
                bounds: Rect::new(v(0), v(1), v(2), v(3)),
                opacity: v(4).clamp(0.0, 1.0),
                scale: v(5).max(0.0),
                radius: v(6).max(0.0),
                blur: v(7).max(0.0),
                glass: layer.look.glass.is_some(),
                frame: layer.frame.clone(),
            }
        })
        .collect()
}

/// `layers` laid over a transparent window `size` pixels large at `scale` as a compositor draws them: each frame
/// blurred by its deviation in device pixels, its origin on the window's device pixels, scaled about the device pixel
/// nearest its bounds' centre, sampled between pixels, at its opacity, back to front.
fn compose(size: (u32, u32), scale: f64, layers: &[ShownLayer]) -> Vec<u8> {
    use cupertino_raster_math::blur::{plan_blur, Edge, Image, Precision};
    let (w, h) = (size.0 as usize, size.1 as usize);
    let mut out = vec![[0.0f32; 4]; w * h];
    for layer in layers {
        let Some(frame) = &layer.frame else { continue };
        if layer.opacity <= 0.0 || frame.width == 0 {
            continue;
        }
        let (fw, fh) = (frame.width as usize, frame.pixel_height as usize);
        let sigma = layer.blur * scale;
        let pad = if sigma >= SHARP_BELOW_PX {
            (3.0 * sigma).ceil() as usize
        } else {
            0
        };
        let (iw, ih) = (fw + 2 * pad, fh + 2 * pad);
        let mut pixels = vec![[0.0f32; 4]; iw * ih];
        for y in 0..fh {
            for x in 0..fw {
                let s = &frame.pixels[y * frame.stride as usize + 4 * x..][..4];
                pixels[(y + pad) * iw + x + pad] = std::array::from_fn(|c| f32::from(s[c]) / 255.0);
            }
        }
        let mut image = Image::new(iw, ih, pixels);
        if pad > 0 {
            let plan = plan_blur(iw as u32, ih as u32, 1.0, sigma as f32, 1.0);
            assert!(plan.levels.is_empty(), "a blur this small is one Gaussian");
            image = image.blur(&plan, Edge::Transparent, Precision::Half);
        }
        let b = layer.bounds;
        let origin = ((b.x() * scale).round(), (b.y() * scale).round());
        let pivot = (
            ((b.x() + b.width() / 2.0) * scale).round(),
            ((b.y() + b.height() / 2.0) * scale).round(),
        );
        let s = layer.scale.max(1e-6);
        let o = layer.opacity as f32;
        let read = |x: i64, y: i64| image.read(x, y, Edge::Transparent);
        // the window's pixels the scaled copy can reach, a pixel either way
        let reach = |pivot: f64, origin: f64, size: usize, most: usize| {
            let at = |u: f64| pivot + (u + origin - pad as f64 - pivot) * s;
            let low = (at(-1.0).floor() - 1.0).max(0.0) as usize;
            let high = (at(size as f64 + 1.0).ceil() + 1.0).clamp(0.0, most as f64) as usize;
            low..high.max(low)
        };
        let (xs, ys) = (
            reach(pivot.0, origin.0, iw, w),
            reach(pivot.1, origin.1, ih, h),
        );
        for (py, row) in out.chunks_mut(w).enumerate().skip(ys.start).take(ys.len()) {
            for (px, d) in row.iter_mut().enumerate().skip(xs.start).take(xs.len()) {
                // the frame's pixel under this one, from its blurred copy's top left
                let u = pivot.0 + (px as f64 + 0.5 - pivot.0) / s - origin.0 + pad as f64 - 0.5;
                let v = pivot.1 + (py as f64 + 0.5 - pivot.1) / s - origin.1 + pad as f64 - 0.5;
                if u < -1.0 || v < -1.0 || u > iw as f64 || v > ih as f64 {
                    continue;
                }
                let (x0, y0) = (u.floor(), v.floor());
                let (fx, fy) = ((u - x0) as f32, (v - y0) as f32);
                let (x0, y0) = (x0 as i64, y0 as i64);
                let corners = [
                    read(x0, y0),
                    read(x0 + 1, y0),
                    read(x0, y0 + 1),
                    read(x0 + 1, y0 + 1),
                ];
                let src: [f32; 4] = std::array::from_fn(|k| {
                    let top = corners[0][k] * (1.0 - fx) + corners[1][k] * fx;
                    let bottom = corners[2][k] * (1.0 - fx) + corners[3][k] * fx;
                    (top * (1.0 - fy) + bottom * fy) * o
                });
                let keep = 1.0 - src[3];
                for k in 0..4 {
                    d[k] = src[k] + d[k] * keep;
                }
            }
        }
    }
    out.iter()
        .flat_map(|p| p.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8))
        .collect()
}

/// With `CUPERTINO_LAUNCHER_MOMENTS` set to a directory, writes the two drawings of a moment there as premultiplied
/// RGBA, `<appearance>-<what>-<ms>-<own|layers>-<width>x<height>.rgba`, to look at side by side.
fn keep_moment(what: &str, dark: bool, own: &Moment, layers: &Moment) {
    let Some(dir) = std::env::var_os("CUPERTINO_LAUNCHER_MOMENTS") else {
        return;
    };
    let look = if dark { "dark" } else { "light" };
    for (moment, by) in [(own, "own"), (layers, "layers")] {
        let (w, h) = moment.size;
        let name = format!(
            "{look}-{what}-{:03.0}-{by}-{w}x{h}.rgba",
            moment.after * 1000.0
        );
        std::fs::write(std::path::Path::new(&dir).join(name), &moment.pixels).unwrap();
    }
}

/// What one moment of a close looks like: the window's pixels and the glass it shows (rect, corner radius, opacity).
struct Moment {
    after: f64,
    size: (u32, u32),
    pixels: Vec<u8>,
    glass: Vec<(Rect, f64, f64)>,
    /// Where the two drawings are known to part, in points: the mode buttons' faces, which the compositor moves onto
    /// whole device pixels, and the caret, which the panel's own drawing moves a device pixel as its enclosure moves.
    apart: Vec<Rect>,
}

/// The search field's close at `scale` in the `dark` or light appearance, from Escape on: drawn by the panel itself
/// (`layers` false) at each frame it draws of it, or handed to the compositor and shown at `moments` (seconds after
/// Escape) by [`shown_after`] and [`compose`].
fn close_moments(scale: f64, dark: bool, layers: bool, moments: &[f64]) -> Vec<Moment> {
    let mut m = drawn(scale, dark, layers);
    m.settle_on(Message::Toggle);
    let rest = m.shown_layers();
    let asked = m.layer_requests().len();
    // the panel at rest first, as it shows before Escape
    let mut out = vec![{
        let (w, h, pixels) = m.window_pixels();
        let pixels = if layers {
            compose((w, h), scale, &shown_after(&rest, &[], 0.0))
        } else {
            pixels
        };
        Moment {
            after: -1.0,
            size: (w, h),
            pixels,
            glass: Vec::new(),
            apart: Vec::new(),
        }
    }];
    m.step(|s, w| key(s, w, ESCAPE, None, Mods::NONE));
    let start = CLOCK.get();
    if layers {
        assert!(m.model().close_handed(), "the close is the compositor's");
        let changes = m.layer_requests()[asked..].to_vec();
        let (w, h, _) = m.window_pixels();
        // the buttons travel a few points in the close, and the caret's blur reaches out past its column
        let mut apart: Vec<Rect> = rest
            .iter()
            .filter(|l| (16..).contains(&l.key) && l.key % 2 == 1)
            .map(|l| l.rect.outset(8.0))
            .collect();
        if let Some(glass) = rest.iter().find(|l| l.key == GLASS_LAYER) {
            let x = glass.rect.x() + crate::view::QUERY_INSET;
            apart.push(Rect::new(
                x - 8.0,
                glass.rect.y(),
                16.0,
                glass.rect.height(),
            ));
        }
        for &after in moments {
            let shown = shown_after(&rest, &changes, after);
            out.push(Moment {
                after,
                size: (w, h),
                pixels: compose((w, h), scale, &shown),
                glass: shown
                    .iter()
                    .filter(|l| l.glass)
                    .map(|l| (l.bounds, l.radius, l.opacity))
                    .collect(),
                apart: apart.clone(),
            });
        }
        return out;
    }
    while out.len() <= moments.len() {
        m.step(|_, _| {});
        let after = CLOCK.get() - start;
        let Some(panel) = m.panel() else { break };
        if after >= crate::motion::CLOSE_DURATION {
            break;
        }
        let (w, h, pixels) = m.window_pixels();
        let glass = m
            .driver
            .runtime()
            .window_system()
            .requests(panel)
            .unwrap()
            .iter()
            .rev()
            .find_map(|r| match r {
                WindowRequest::Materials(regions) => Some(regions.clone()),
                _ => None,
            })
            .unwrap_or_default()
            .iter()
            .map(|r| (r.rect, r.radius, r.alpha))
            .collect();
        out.push(Moment {
            after,
            size: (w, h),
            pixels,
            glass,
            apart: Vec::new(),
        });
    }
    out
}

/// The close the compositor runs is the panel's own drawing of it at the same moments: at each frame the panel draws
/// of its close, the compositor's layers at that moment of the close, blurred, scaled and faded as a compositor draws
/// them, are its pixels but where the two are known to part ([`Moment::apart`]): within 8 of 255 at the first frames,
/// where the blur is a pixel or two wide and two blurs of the same deviation round differently, and within 1 from the
/// close's middle on. Their glass is the glass it stands on (rect, corner and opacity). Run with `--no-capture` for the
/// largest differences, and with `CUPERTINO_LAUNCHER_MOMENTS` for the pictures ([`keep_moment`]).
#[test]
fn the_close_the_compositor_runs_is_the_panel_s_own_drawing_at_the_same_moments() {
    for dark in [true, false] {
        let own = close_moments(1.5, dark, false, &[0.0; 8]);
        assert!(
            own.len() >= 5,
            "the panel drew its close: {}",
            own.len() - 1
        );
        let moments: Vec<f64> = own[1..].iter().map(|m| m.after).collect();
        let handed = close_moments(1.5, dark, true, &moments);
        let (most, _) = compare(&own[0].pixels, &handed[0].pixels, own[0].size.0);
        eprintln!("trace: close, dark {dark}, at rest: largest difference {most}");
        keep_moment("rest", dark, &own[0], &handed[0]);
        assert!(most <= 1, "at rest, dark {dark}: {most}");
        for (a, b) in own[1..].iter().zip(&handed[1..]) {
            assert_eq!(a.size, b.size);
            let ms = a.after * 1000.0;
            let width = a.size.0;
            let (mut most, mut most_apart) = (0, 0);
            for (i, (p, q)) in a.pixels.chunks(4).zip(b.pixels.chunks(4)).enumerate() {
                let d = p
                    .iter()
                    .zip(q)
                    .map(|(u, v)| u.abs_diff(*v))
                    .max()
                    .unwrap_or(0);
                let at = Point::new(
                    (i as u32 % width) as f64 / 1.5,
                    (i as u32 / width) as f64 / 1.5,
                );
                if b.apart.iter().any(|r| r.contains(at)) {
                    most_apart = most_apart.max(d);
                } else {
                    most = most.max(d);
                }
            }
            eprintln!(
                "trace: close, dark {dark}, {ms:.0} ms: largest difference {most}, {most_apart} on the buttons' faces and the caret"
            );
            keep_moment("close", dark, a, b);
            let bound = if a.after < crate::motion::CLOSE_DURATION / 2.0 {
                8
            } else {
                1
            };
            assert!(most <= bound, "close, dark {dark}, {ms:.0} ms: {most}");
            assert_eq!(a.glass.len(), b.glass.len(), "{:?} {:?}", a.glass, b.glass);
            for ((r0, q0, o0), (r1, q1, o1)) in a.glass.iter().zip(&b.glass) {
                let off = (r0.x() - r1.x())
                    .abs()
                    .max((r0.y() - r1.y()).abs())
                    .max((r0.width() - r1.width()).abs())
                    .max((r0.height() - r1.height()).abs());
                assert!(off < 0.02, "glass {r0:?} against {r1:?}");
                assert!(
                    (q0 - q1).abs() < 0.02 && (o0 - o1).abs() < 0.01,
                    "{q0} {o0} / {q1} {o1}"
                );
            }
        }
    }
}

/// A scope opening from hidden hands its open to the compositor: the panel draws its layers once, at rest, and asks
/// the compositor to move them from where the open starts, the glass from 1.1 of its size on the open's spring and
/// everything fading in; then it draws no frame of its window or of a layer, and asks for none, until the open is over.
#[test]
fn a_scope_opens_from_hidden_on_the_compositor_s_clock_without_a_frame() {
    for (what, message) in [
        ("the browser", Message::Browse),
        ("the files scope", Message::Files),
    ] {
        let mut m = drawn(1.5, true, true);
        // the panel's window is made with the open: every change it asks for is the open's
        let asked = 0;
        send(message);
        m.step(|_, _| {});
        assert!(
            m.model().open_handed(),
            "{what}: the open is the compositor's"
        );
        assert!(
            m.model().layered(),
            "{what}: drawn as layers from the first frame"
        );
        // the window is configured and draws its first frame, the layers at rest
        for _ in 0..4 {
            if m.presents().0 > 0 {
                break;
            }
            m.step(|_, _| {});
        }
        assert!(m.model().open_handed(), "{what}: still the compositor's");
        let first = m.presents();
        let keys: Vec<u64> = m.shown_layers().iter().map(|l| l.key).collect();
        assert_eq!(keys, vec![GLASS_LAYER, CONTENT_LAYER, EDGE_LAYER], "{what}");
        let open = m.layer_requests()[asked..].to_vec();
        let of = |key: u64, p: LayerProperty| -> Vec<cupertino_kit::platform::LayerChange> {
            open.iter()
                .filter(|(k, _)| *k == key)
                .flat_map(|(_, c)| c.iter().filter(|c| c.property == p).cloned())
                .collect()
        };
        let rest = m.shown_layers();
        let glass = rest.iter().find(|l| l.key == GLASS_LAYER).unwrap().rect;
        // the glass through the panel's own values, from where the open starts to its rest
        let width = of(GLASS_LAYER, LayerProperty::Width);
        assert_eq!(width.len(), 1, "{what}: carried through samples");
        let samples = width[0].samples.clone().expect("samples");
        assert_eq!(samples.len(), crate::view::layers::OPEN_SAMPLES);
        let start = f64::from(samples[0]);
        assert!((start - glass.width() * crate::motion::ENTER_SCALE).abs() < 1e-3);
        assert_eq!(width[0].to, glass.width());
        // the content's place springs, the compositor putting it on whole device pixels as the panel does
        let y = of(CONTENT_LAYER, LayerProperty::Y);
        assert_eq!(
            y.len(),
            2,
            "{what}: set where it starts, then sprung to its rest"
        );
        assert!(matches!(
            y[1].curve,
            Some(cupertino_kit::platform::LayerCurve::Spring { response, damping })
                if response == crate::motion::OPEN_RESPONSE && damping == crate::motion::OPEN_DAMPING
        ));
        for key in [GLASS_LAYER, EDGE_LAYER, CONTENT_LAYER] {
            let fade = of(key, LayerProperty::Opacity);
            assert_eq!(
                (fade[0].to, fade[1].to),
                (0.0, 1.0),
                "{what}: layer {key} fades in"
            );
        }
        // the edge's scale through the panel's own values, from where the open starts to its rest
        let scale = of(EDGE_LAYER, LayerProperty::Scale);
        let samples = scale[0].samples.clone().expect("carried as samples");
        assert_eq!(
            f64::from(samples[0]),
            crate::motion::ENTER_SCALE as f32 as f64
        );
        assert_eq!((scale[0].to, *samples.last().unwrap()), (1.0, 1.0));
        // the open's frames: none drawn, none asked for, the browser's scroll indicator, which shows as it comes into
        // view, kept as it is meanwhile
        let open = {
            let mut motion = crate::motion::Motion::new(m.model().motion_target());
            motion.show_in(m.model().motion_target(), m.model().presented().header, 0.0);
            motion.moving_left(0.0)
        };
        assert!(
            open < crate::browse::SCROLLER_LINGER,
            "{what}: the open, {open} s, is over before the indicator fades"
        );
        for _ in 0..(open * 60.0).ceil() as usize + 2 {
            m.step(|_, _| {});
            assert!(!m.model().animating(), "{what}: the open asks for no frame");
        }
        assert_eq!(
            m.presents(),
            first,
            "{what}: no frame of the window or its layers"
        );
        send(Message::OpenOver);
        m.step(|_, _| {});
        assert!(!m.model().open_handed(), "{what}: the open is over");
        assert!(m.model().layered(), "{what}: at rest as layers");
        assert_eq!(
            m.model().animating(),
            what == "the browser",
            "{what}: the indicator goes on where it was"
        );
    }
}

/// Under Reduce Motion a scope's open is not handed over: the panel opens drawn in place.
#[test]
fn under_reduce_motion_a_scope_s_open_is_the_panel_s_own() {
    let mut m = drawn(1.5, true, true);
    m.settle_on(Message::ReduceMotion(true));
    send(Message::Browse);
    m.step(|_, _| {});
    assert!(!m.model().open_handed());
    assert!(m.shown_layers().is_empty(), "drawn in place");
    assert!(m.layer_requests().is_empty());
}

/// A scope's open from hidden at `scale` in the `dark` or light appearance, from `message` on: drawn by the panel
/// itself (`layers` false) at each frame it draws of it, or handed to the compositor and shown at `moments` (seconds
/// after the open began, which the compositor's curves start from) by [`shown_after`] and [`compose`].
fn open_moments(
    scale: f64,
    dark: bool,
    layers: bool,
    message: fn() -> Message,
    moments: &[f64],
) -> Vec<Moment> {
    let mut m = drawn(scale, dark, layers);
    let start = CLOCK.get();
    send(message());
    m.step(|_, _| {});
    let mut out = Vec::new();
    if layers {
        assert!(m.model().open_handed(), "the open is the compositor's");
        for _ in 0..4 {
            if m.presents().0 > 0 {
                break;
            }
            m.step(|_, _| {});
        }
        let rest = m.shown_layers();
        let changes = m.layer_requests();
        let (w, h, _) = m.window_pixels();
        for &after in moments {
            let shown = shown_after(&rest, &changes, after);
            out.push(Moment {
                after,
                size: (w, h),
                pixels: compose((w, h), scale, &shown),
                glass: shown
                    .iter()
                    .filter(|l| l.glass)
                    .map(|l| (l.bounds, l.radius, l.opacity))
                    .collect(),
                apart: Vec::new(),
            });
        }
        return out;
    }
    // each frame the panel draws, at the time it is drawn: the first in the step the open began in, the last the first
    // at rest (the caret's blink draws later ones)
    let open = {
        let target = m.model().motion_target();
        let mut motion = crate::motion::Motion::new(target);
        motion.show_in(target, m.model().presented().header, 0.0);
        motion.moving_left(0.0)
    };
    let mut seen = 0;
    let mut after = CLOCK.get() - start;
    for _ in 0..120 {
        if out.len() >= moments.len() {
            break;
        }
        let Some(panel) = m.panel() else {
            m.step(|_, _| {});
            after = CLOCK.get() - start;
            continue;
        };
        let system = m.driver.runtime().window_system();
        let drawn = system
            .frames(panel)
            .unwrap()
            .iter()
            .filter(|f| !f.pixels.is_empty())
            .count();
        if drawn == seen {
            m.step(|_, _| {});
            after = CLOCK.get() - start;
            continue;
        }
        // the first frame shows the panel as the open began, once its window is configured
        if seen == 0 {
            after = 0.0;
        }
        seen = drawn;
        let glass = system
            .requests(panel)
            .unwrap()
            .iter()
            .rev()
            .find_map(|r| match r {
                WindowRequest::Materials(regions) => Some(regions.clone()),
                _ => None,
            })
            .unwrap_or_default()
            .iter()
            .map(|r| (r.rect, r.radius, r.alpha))
            .collect();
        let (w, h, pixels) = m.window_pixels();
        out.push(Moment {
            after,
            size: (w, h),
            pixels,
            glass,
            apart: Vec::new(),
        });
        if !m.model().animating() || after >= open {
            break;
        }
        m.step(|_, _| {});
        after = CLOCK.get() - start;
    }
    out
}

/// How far two frames at `scale` differ, `(apart from the edge, along the edge)`: the largest difference of a channel
/// outside and inside the band of [`EDGE_BAND`] points either side of the enclosure's outline, the rounded rect of
/// `glass` (points), where the panel strokes its edge.
fn apart_from_edge(a: &Moment, b: &Moment, glass: (Rect, f64), scale: f64) -> (u8, u8) {
    let (rect, radius) = glass;
    let (cx, cy) = (
        rect.x() + rect.width() / 2.0,
        rect.y() + rect.height() / 2.0,
    );
    let (hw, hh) = (rect.width() / 2.0, rect.height() / 2.0);
    let radius = radius.min(hw).min(hh);
    let width = a.size.0;
    let (mut content, mut edge) = (0, 0);
    for (i, (p, q)) in a.pixels.chunks(4).zip(b.pixels.chunks(4)).enumerate() {
        let d = p
            .iter()
            .zip(q)
            .map(|(u, v)| u.abs_diff(*v))
            .max()
            .unwrap_or(0);
        if d == 0 {
            continue;
        }
        let x = ((i as u32 % width) as f64 + 0.5) / scale;
        let y = ((i as u32 / width) as f64 + 0.5) / scale;
        let qx = (x - cx).abs() - (hw - radius);
        let qy = (y - cy).abs() - (hh - radius);
        let outside = qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - radius;
        if outside.abs() < EDGE_BAND {
            edge = edge.max(d);
        } else {
            content = content.max(d);
        }
    }
    (content, edge)
}

/// The band either side of the enclosure's outline, in points, that holds its edge stroke and the stroke's
/// antialiasing as the open scales it.
const EDGE_BAND: f64 = 2.0;

/// The open the compositor runs against the panel's own drawing of it at the same moments: at each frame the panel
/// draws of a scope's open from hidden, the compositor's layers at that moment of the open, scaled, moved and faded as
/// a compositor draws them, against its pixels, and their glass against the glass it stands on (rect, corner and
/// opacity), which it is at every moment. Their pixels are the same within 1 of 255 at every moment but along the edge
/// (the content lies on whole device pixels in both, [`crate::view`]'s `PanelRoot::on_pixels`); the edge is the rest's
/// stroke scaled by the compositor where the panel strokes it afresh at the fractional place the scaled shape puts it,
/// a half-point line on a device pixel grid, so there the two part by up to two fifths of the line's opacity. Run with
/// `--no-capture` for the largest differences, and with `CUPERTINO_LAUNCHER_MOMENTS` for the pictures
/// ([`keep_moment`]).
#[test]
fn the_open_the_compositor_runs_is_the_panel_s_own_drawing_at_the_same_moments() {
    type Open = fn() -> Message;
    let scopes: [(&str, Open); 2] = [
        ("browser", || Message::Browse),
        ("files", || Message::Files),
    ];
    for (what, message) in scopes {
        for dark in [true, false] {
            let own = open_moments(1.5, dark, false, message, &[0.0; 40]);
            assert!(
                own.len() >= 12,
                "{what}: the panel drew its open: {}",
                own.len()
            );
            // every other frame of the open and its last, at rest
            let last = own.len() - 1;
            let own: Vec<Moment> = own
                .into_iter()
                .enumerate()
                .filter(|(i, _)| i % 2 == 0 || *i == last)
                .map(|(_, m)| m)
                .collect();
            let moments: Vec<f64> = own.iter().map(|m| m.after).collect();
            let handed = open_moments(1.5, dark, true, message, &moments);
            // the line's own opacity, of which the scaled edge may differ by two fifths
            let line = f64::from(crate::view::edge_alpha(dark, true)) * 255.0;
            let edge_bound = (0.4 * line).ceil() as u8;
            let (mut worst, mut worst_edge) = (0, 0);
            for (a, b) in own.iter().zip(&handed) {
                assert_eq!(a.size, b.size);
                let ms = a.after * 1000.0;
                // the enclosure's glass, which a frame wholly faded out does not stand on
                let (content, edge) = match a.glass.first() {
                    Some(&(rect, radius, _)) => apart_from_edge(a, b, (rect, radius), 1.5),
                    None => (compare(&a.pixels, &b.pixels, a.size.0).0, 0),
                };
                worst = worst.max(content);
                worst_edge = worst_edge.max(edge);
                eprintln!(
                    "trace: open {what}, dark {dark}, {ms:.0} ms: largest difference {content}, {edge} along the edge"
                );
                keep_moment(&format!("open-{what}"), dark, a, b);
                assert!(
                    content <= 1,
                    "{what}, dark {dark}, {ms:.0} ms: {content} apart from the edge"
                );
                assert!(
                    edge <= edge_bound,
                    "{what}, dark {dark}, {ms:.0} ms: {edge} along the edge, over {edge_bound}"
                );
                assert_eq!(a.glass.len(), b.glass.len(), "{:?} {:?}", a.glass, b.glass);
                for ((r0, q0, o0), (r1, q1, o1)) in a.glass.iter().zip(&b.glass) {
                    let off = (r0.x() - r1.x())
                        .abs()
                        .max((r0.y() - r1.y()).abs())
                        .max((r0.width() - r1.width()).abs())
                        .max((r0.height() - r1.height()).abs());
                    assert!(off < 0.05, "{what} {ms:.0} ms: glass {r0:?} against {r1:?}");
                    assert!(
                        (q0 - q1).abs() < 0.05 && (o0 - o1).abs() < 0.01,
                        "{what} {ms:.0} ms: {q0} {o0} / {q1} {o1}"
                    );
                }
            }
            let (first, _) = compare(&own[0].pixels, &handed[0].pixels, own[0].size.0);
            eprintln!(
                "trace: open {what}, dark {dark}: {} of its {} frames, largest {worst} apart from the edge, {worst_edge} \
                 along it (bound {edge_bound}), at the first frame {first}",
                own.len(),
                last + 1
            );
            assert!(
                first <= 1,
                "{what}, dark {dark}: at the first frame {first}"
            );
        }
    }
}
