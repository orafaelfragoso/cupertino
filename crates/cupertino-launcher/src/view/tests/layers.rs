//! The panel drawn as compositor layers ([`crate::view::layers`]): at rest its layers carry the pixels it draws in
//! place and the glass it stands on, and its close runs on the compositor's clock without a frame of the panel's.

use super::*;
use crate::motion::CLOSE_DURATION;
use crate::view::layers::{
    button_face_layer, button_glass_layer, BODY_LAYER, GLASS_LAYER, OPTIONS_LAYER, ROW_LAYER,
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
    /// without its glass, over the edge a glass layer's look carries, which the compositor draws along the glass.
    fn composed(&self) -> (u32, u32, Vec<u8>) {
        let (w, h, mut out) = self.window_pixels();
        let shown = self.shown_layers();
        if let Some(edge) = shown
            .iter()
            .find(|l| l.look.edge.width > 0.0)
            .map(|l| l.look.edge)
        {
            let model = compositor_edge((w, h), self.scale, &shown, edge);
            for (d, (a, _)) in out.chunks_mut(4).zip(model) {
                let keep = 1.0 - f32::from(d[3]) / 255.0;
                let alpha = f32::from(d[3]) + 255.0 * a * keep;
                d[3] = alpha.round().clamp(0.0, 255.0) as u8;
            }
        }
        for layer in shown {
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
    m.finish_handed_open();
    assert_eq!(m.model().nav.view, View::Browse);
    out.push(take(&mut m, "the browser"));
    to_files(&mut m);
    out.push(take(&mut m, "the files scope"));
    out
}

/// From the browser at rest, the files scope at rest: the browser folds (the compositor telling when a fold it ran is
/// over) and closes, the files scope opens and a letter is typed, which shows the caret afresh whether or not the
/// compositor ran the open.
fn to_files(m: &mut Metered) {
    for _ in 0..2 {
        m.step(|s, w| key(s, w, ESCAPE, None, Mods::NONE));
        m.until_quiet(4, 200);
        m.finish_handed_open();
        m.until_quiet(4, 200);
    }
    send(Message::CloseOver);
    m.settle_on(Message::Files);
    assert_eq!(m.model().nav.view, View::Files);
    m.step(|s, w| key(s, w, 'x' as u32, Some('x'), Mods::NONE));
    m.until_quiet(4, 200);
}

/// The panel at rest in each view (the search field, its results, the browser and the files scope), drawn in place and
/// drawn as layers, its query row and its body each a layer of its own, a scope's edge the compositor's along its
/// glass: the layers laid over the window's frame (and that edge) are the window's frame drawn in place, pixel for
/// pixel, at whole and fractional scales and in both appearances; and the glass the layers carry is the glass the
/// panel stands on in place, the window standing on none of its own.
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
                    b.keys.contains(&GLASS_LAYER)
                        && b.keys.contains(&ROW_LAYER)
                        && b.keys.contains(&BODY_LAYER),
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
                    ROW_LAYER,
                    BODY_LAYER,
                    button_glass_layer(0),
                    button_face_layer(0)
                ],
                "the field's glass, query row and body, then the button's glass and face"
            );
            // every view: the glass, carrying the edge the compositor draws along it, the query row (and the
            // browser's view options button) and the body
            for rest in &layered[1..] {
                assert_eq!(rest.keys, scope_layers(rest.what), "{}", rest.what);
                assert_eq!(
                    rest.glass[0].1.edge,
                    crate::view::layers::glass_edge(dark),
                    "{}",
                    rest.what
                );
            }
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
            ROW_LAYER,
            BODY_LAYER,
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
    m.finish_handed_open();
    out.push(take(&mut m, "the browser"));
    to_files(&mut m);
    out.push(take(&mut m, "the files scope"));
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
                soft: 0.0,
            })
        })
        .collect();
    let (w, h) = (size.0 as usize, size.1 as usize);
    let mut out = vec![(0.0, f32::INFINITY); w * h];
    for (i, px) in out.iter_mut().enumerate() {
        let p = [(i % w) as f32 + 0.5, (i / w) as f32 + 0.5];
        for c in &containers {
            let d = c.field(p).0;
            let a = c.edge_alpha_at(p);
            *px = (1.0 - (1.0 - px.0) * (1.0 - a), px.1.min(d));
        }
    }
    out
}

/// The edge the compositor draws along the panel's glass is the edge the panel draws itself: at rest, at 1, 1.5 and 2
/// in both appearances, outside the glass (inside it the faces draw their symbols), the glass's own model of the edge
/// of [`crate::view::layers::glass_edge`] in each group's look, the search field and its mode button joined in one,
/// is the panel's own edge drawn in place within 1 of 255 along every capsule, every straight side and a scope's
/// continuous corners, which the panel strokes along the glass's own outline (`--no-capture` prints the sides and the
/// corners apart).
#[test]
fn the_edge_the_compositor_draws_along_the_glass_is_the_edge_the_panel_draws() {
    for scale in [1.0, 1.5, 2.0] {
        for dark in [false, true] {
            // the panel drawn in place: outside its glass the window shows its edge alone
            let in_place = walk(scale, dark, false);
            for ((what, size, layers), flat) in
                resting_layers(scale, dark).into_iter().zip(in_place)
            {
                assert_eq!((flat.what, flat.size), (what, size));
                let model =
                    compositor_edge(size, scale, &layers, crate::view::layers::glass_edge(dark));
                // the layers' content lies over the edge, as the compositor draws it on the glass under them
                let content = compose(size, scale, &shown_after(&layers, &[], 0.0));
                let model: Vec<(f32, f32)> = model
                    .into_iter()
                    .zip(content.chunks(4))
                    .map(|((a, d), c)| {
                        let c = f32::from(c[3]) / 255.0;
                        (c + a * (1.0 - c), d)
                    })
                    .collect();
                let own: Vec<f32> = flat
                    .pixels
                    .chunks(4)
                    .map(|p| f32::from(p[3]) / 255.0)
                    .collect();
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
                    most <= 1.0 && most_corner <= 1.0,
                    "{what} at {scale}x, dark {dark}: the compositor's edge differs from the panel's by {most:.1}, \
                     at the corners {most_corner:.1}"
                );
            }
        }
    }
}

/// The deviation below which a compositor draws a layer's content sharp, in device pixels.
const SHARP_BELOW_PX: f64 = 0.3;

/// One layer as a compositor shows it at one moment: its bounds, opacity, scale about the bounds' centre, corner
/// radius and content blur, whether it is cut to its shape, with the frame it drew.
#[derive(Clone, Debug)]
struct ShownLayer {
    bounds: Rect,
    opacity: f64,
    scale: f64,
    radius: f64,
    blur: f64,
    glass: bool,
    masks: bool,
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
                masks: layer.look.masks,
                frame: layer.frame.clone(),
            }
        })
        .collect()
}

/// `layers` laid over a transparent window `size` pixels large at `scale` as a compositor draws them: each frame
/// blurred by its deviation in device pixels, its origin on the window's device pixels, scaled about the device pixel
/// nearest its bounds' centre, sampled between pixels, at its opacity, cut to its bounds with its continuous corners
/// where it masks, back to front.
fn compose(size: (u32, u32), scale: f64, layers: &[ShownLayer]) -> Vec<u8> {
    let (w, h) = (size.0 as usize, size.1 as usize);
    compose_over(size, scale, layers, vec![[0.0f32; 4]; w * h])
}

/// [`compose`] with the edge a compositor draws along the glass of `layers` (one glass group, the edge of its first
/// layer `edge`) under them: each shape's outline cut by the others ([`cupertino_glass::glass::Container::edge_alpha_at`]
/// on the shapes as shown, in device pixels), at the group's opacity, the most of its layers'.
fn compose_with_edge(
    size: (u32, u32),
    scale: f64,
    layers: &[ShownLayer],
    edge: cupertino_kit::platform::LayerEdge,
) -> Vec<u8> {
    use cupertino_glass::glass::{Container, GlassEdge, GlassMode, GlassShape};
    let s = scale as f32;
    let glass: Vec<&ShownLayer> = layers
        .iter()
        .filter(|l| l.glass && l.opacity > 0.0 && l.bounds.width() > 0.0)
        .collect();
    let (w, h) = (size.0 as usize, size.1 as usize);
    let mut base = vec![[0.0f32; 4]; w * h];
    if !glass.is_empty() {
        let shapes = glass
            .iter()
            .map(|l| {
                let b = l.bounds;
                GlassShape::new(
                    [
                        b.x() as f32 * s,
                        b.y() as f32 * s,
                        b.width() as f32 * s,
                        b.height() as f32 * s,
                    ],
                    l.radius as f32 * s,
                )
            })
            .collect();
        let opacity = glass.iter().map(|l| l.opacity).fold(0.0, f64::max) as f32;
        // the edge softens by the first layer's content blur, as its content would be drawn: scaled with it, and none
        // where the content would be drawn sharp
        let first = glass[0];
        let sigma = first.blur * scale;
        let soft = if sigma >= SHARP_BELOW_PX {
            (sigma * first.scale) as f32
        } else {
            0.0
        };
        let c = Container::new(shapes, 0.0, GlassMode::default()).with_edge(
            GlassEdge {
                width: edge.width as f32 * s,
                color: edge.color,
                soft: 0.0,
            }
            .soft(soft),
        );
        for (i, px) in base.iter_mut().enumerate() {
            let p = [(i % w) as f32 + 0.5, (i / w) as f32 + 0.5];
            let a = c.edge_alpha_at(p) * opacity;
            *px = [edge.color[0] * a, edge.color[1] * a, edge.color[2] * a, a];
        }
    }
    compose_over(size, scale, layers, base)
}

/// [`compose`] over `base`, premultiplied pixels of the window's size.
fn compose_over(
    size: (u32, u32),
    scale: f64,
    layers: &[ShownLayer],
    base: Vec<[f32; 4]>,
) -> Vec<u8> {
    use cupertino_raster_math::blur::{plan_blur, Edge, Image, Precision};
    let (w, h) = (size.0 as usize, size.1 as usize);
    let mut out = base;
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
        // the share of a device pixel inside the layer's shape as shown, where it cuts its frame to it: its bounds on
        // the device pixels its content lies on
        let mask = layer.masks.then(|| {
            let f = scale as f32;
            cupertino_glass::glass::GlassShape::new(
                [
                    origin.0 as f32,
                    origin.1 as f32,
                    b.width() as f32 * f,
                    b.height() as f32 * f,
                ],
                layer.radius as f32 * f,
            )
        });
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
                let cut = mask.as_ref().map_or(1.0, |m| {
                    (0.5 - m.sdf([px as f32 + 0.5, py as f32 + 0.5])).clamp(0.0, 1.0)
                });
                let src: [f32; 4] = std::array::from_fn(|k| {
                    let top = corners[0][k] * (1.0 - fx) + corners[1][k] * fx;
                    let bottom = corners[2][k] * (1.0 - fx) + corners[3][k] * fx;
                    (top * (1.0 - fy) + bottom * fy) * o * cut
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

/// The close of the panel `open` opens (the search field or a scope) at `scale` in the `dark` or light appearance, from
/// Escape on (two for a scope, the first folding it into the field, or the scope's own close where it opens alone):
/// drawn by the panel itself (`layers` false) at each frame it draws of it, or handed to the compositor and shown at
/// `moments` (seconds after Escape) by [`shown_after`] and [`compose_with_edge`], the edge the compositor draws along
/// the glass.
fn close_moments(
    scale: f64,
    dark: bool,
    layers: bool,
    open: fn() -> Message,
    moments: &[f64],
) -> Vec<Moment> {
    let mut m = drawn(scale, dark, layers);
    m.settle_on(open());
    let rest = m.shown_layers();
    let asked = m.layer_requests().len();
    let edge = rest
        .iter()
        .find(|l| l.look.edge.width > 0.0)
        .map(|l| l.look.edge);
    let composed = |size, shown: &[ShownLayer]| match edge {
        Some(edge) => compose_with_edge(size, scale, shown, edge),
        None => compose(size, scale, shown),
    };
    // the panel at rest first, as it shows before Escape
    let mut out = vec![{
        let (w, h, pixels) = m.window_pixels();
        let pixels = if layers {
            composed((w, h), &shown_after(&rest, &[], 0.0))
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
    // the field closes on Escape, a scope on the shortcut that opened the panel (Escape folds it into the field)
    if matches!(open(), Message::Toggle) {
        m.step(|s, w| key(s, w, ESCAPE, None, Mods::NONE));
    } else {
        send(Message::Toggle);
        m.step(|_, _| {});
    }
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
                pixels: composed((w, h), &shown),
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

/// How far a close the compositor runs may stand from the panel's own drawing along the outline, of 255: the panel
/// blurs its edge with its content as it closes, and the compositor blurs the edge it draws along the glass by the
/// glass layer's blur, the content's, so the two part there by 2 in the dark appearance and 1 in the light at the
/// close's first frame (by 100 and 29 when the compositor's edge stayed sharp).
const CLOSE_ALONG_OUTLINE: u8 = 4;

/// How far either side of the glass's outline, in points, a close's edge reaches: the edge stroke, and the panel's own
/// blur of it at its widest, three deviations either way.
const CLOSE_EDGE_BAND: f64 = EDGE_BAND + 3.0 * crate::view::layers::CLOSE_DEVIATION;

/// The close the compositor runs is the panel's own drawing of it at the same moments, for the search field in both
/// appearances and the browser and the files scope in the dark one (in the light in
/// [`a_scope_s_close_in_the_light_is_the_panel_s_own_drawing_at_the_same_moments`]): at each
/// frame the panel draws of its close, the compositor's layers at that moment of the close, blurred, scaled and faded
/// as a compositor draws them, with the edge it draws along the glass blurred by the glass layer's blur, are its pixels
/// at rest within 1 of 255, and part way but where the two are known to part ([`Moment::apart`]) and along the outline
/// ([`CLOSE_EDGE_BAND`]): within 8 at the first frames, where the blur is a pixel or two wide and two blurs of the same
/// deviation round differently, and within 1 from the close's middle on. Along the outline they part by up to
/// [`CLOSE_ALONG_OUTLINE`]. Their glass is the glass it stands on (rect, corner and opacity). Run with `--no-capture`
/// for the largest differences, and with `CUPERTINO_LAUNCHER_MOMENTS` for the pictures ([`keep_moment`]).
#[test]
fn the_close_the_compositor_runs_is_the_panel_s_own_drawing_at_the_same_moments() {
    let panels: [(&str, Opening); 3] = [
        ("the search field", || Message::Toggle),
        ("the browser", || Message::Browse),
        ("the files scope", || Message::Files),
    ];
    for (n, (what, open)) in panels.into_iter().enumerate() {
        for dark in [true, false] {
            if n == 0 || dark {
                the_close_at_the_same_moments(what, open, dark);
            }
        }
    }
}

/// [`the_close_the_compositor_runs_is_the_panel_s_own_drawing_at_the_same_moments`] for the browser and the files scope
/// in the light appearance, a test of its own so that each stays within the test runner's time.
#[test]
fn a_scope_s_close_in_the_light_is_the_panel_s_own_drawing_at_the_same_moments() {
    the_close_at_the_same_moments("the browser", || Message::Browse, false);
    the_close_at_the_same_moments("the files scope", || Message::Files, false);
}

/// [`the_close_the_compositor_runs_is_the_panel_s_own_drawing_at_the_same_moments`] for the panel `open` opens.
fn the_close_at_the_same_moments(what: &str, open: fn() -> Message, dark: bool) {
    {
        let own = close_moments(1.5, dark, false, open, &[0.0; 8]);
        assert!(
            own.len() >= 5,
            "{what}: the panel drew its close: {}",
            own.len() - 1
        );
        let moments: Vec<f64> = own[1..].iter().map(|m| m.after).collect();
        let handed = close_moments(1.5, dark, true, open, &moments);
        // at rest apart from the caret, which may blink between the two
        let caret = handed.get(1).map_or(Vec::new(), |m| m.apart.clone());
        let width = own[0].size.0;
        let most = own[0]
            .pixels
            .chunks(4)
            .zip(handed[0].pixels.chunks(4))
            .enumerate()
            .filter(|(i, _)| {
                let at = Point::new(
                    (*i as u32 % width) as f64 / 1.5,
                    (*i as u32 / width) as f64 / 1.5,
                );
                !caret.iter().any(|r| r.contains(at))
            })
            .map(|(_, (p, q))| {
                p.iter()
                    .zip(q)
                    .map(|(u, v)| u.abs_diff(*v))
                    .max()
                    .unwrap_or(0)
            })
            .max()
            .unwrap_or(0);
        eprintln!("trace: close {what}, dark {dark}, at rest: largest difference {most}");
        keep_moment(&format!("rest-{what}"), dark, &own[0], &handed[0]);
        assert!(most <= 1, "{what} at rest, dark {dark}: {most}");
        let mut worst_edge = 0;
        for (a, b) in own[1..].iter().zip(&handed[1..]) {
            assert_eq!(a.size, b.size);
            let ms = a.after * 1000.0;
            let width = a.size.0;
            let shapes: Vec<cupertino_glass::glass::GlassShape> = a
                .glass
                .iter()
                .filter(|g| g.2 > 0.0)
                .map(|(r, radius, _)| {
                    cupertino_glass::glass::GlassShape::new(
                        [
                            r.x() as f32,
                            r.y() as f32,
                            r.width() as f32,
                            r.height() as f32,
                        ],
                        *radius as f32,
                    )
                })
                .collect();
            let (mut most, mut most_apart, mut edge) = (0, 0, 0);
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
                let at = Point::new(
                    (i as u32 % width) as f64 / 1.5,
                    (i as u32 / width) as f64 / 1.5,
                );
                let near = shapes
                    .iter()
                    .any(|s| f64::from(s.sdf([at.x as f32, at.y as f32])).abs() < CLOSE_EDGE_BAND);
                if b.apart.iter().any(|r| r.contains(at)) {
                    most_apart = most_apart.max(d);
                } else if near {
                    edge = edge.max(d);
                } else {
                    most = most.max(d);
                }
            }
            worst_edge = worst_edge.max(edge);
            eprintln!(
                "trace: close {what}, dark {dark}, {ms:.0} ms: largest difference {most}, {edge} along the outline, \
                 {most_apart} on the buttons' faces and the caret"
            );
            assert!(
                edge <= CLOSE_ALONG_OUTLINE,
                "{what}, dark {dark}, {ms:.0} ms: {edge} along the outline"
            );
            keep_moment(&format!("close-{what}"), dark, a, b);
            let bound = if a.after < crate::motion::CLOSE_DURATION / 2.0 {
                8
            } else {
                1
            };
            assert!(
                most <= bound,
                "close {what}, dark {dark}, {ms:.0} ms: {most}"
            );
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
        eprintln!("trace: close {what}, dark {dark}: largest {worst_edge} along the outline");
    }
}

/// A scope opening from hidden hands its open to the compositor: the panel draws its layers once, at rest (the glass,
/// carrying the edge the compositor draws along it, the query row and the body; no edge layer), and asks the
/// compositor to move them from where the open starts, the glass from 1.1 of its size on the open's spring and
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
        assert_eq!(keys, scope_layers(what), "{what}");
        let look = m.shown_layers()[0].look;
        assert_eq!(
            look.edge,
            crate::view::layers::glass_edge(true),
            "{what}: the glass carries the edge"
        );
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
        let y = of(ROW_LAYER, LayerProperty::Y);
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
        for key in [GLASS_LAYER, ROW_LAYER, BODY_LAYER] {
            let fade = of(key, LayerProperty::Opacity);
            assert_eq!(
                (fade[0].to, fade[1].to),
                (0.0, 1.0),
                "{what}: layer {key} fades in"
            );
        }
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
        // the panel's own motion is over, the compositor's may not be: the open ends when the compositor tells
        assert!(
            m.model().open_handed(),
            "{what}: the open is the compositor's until its layers rest"
        );
        m.layers_settle();
        assert!(!m.model().open_handed(), "{what}: the open is over");
        assert!(m.model().layered(), "{what}: at rest as layers");
        assert_eq!(
            m.model().animating(),
            what == "the browser",
            "{what}: the indicator goes on where it was"
        );
    }
}

/// An open the compositor runs ends when the compositor tells the panel's layers came to rest, never on the panel's
/// own clock, which starts a frame or more before the compositor first shows the open: past the panel's own motion it
/// is still the compositor's, drawing nothing; the word that the layers rest, come while the panel's own motion still
/// runs (a compositor that skipped to the end), ends it only once that motion is over; and without the word the timer
/// well past the open's end takes it back, the timer of that open only, not one an earlier open left.
#[test]
fn a_handed_open_ends_on_the_compositor_s_word_or_on_the_timer_well_past_it() {
    let mut m = drawn(1.5, true, true);
    send(Message::Files);
    m.step(|_, _| {});
    assert!(m.model().open_handed(), "the open is the compositor's");
    let open = {
        let mut motion = crate::motion::Motion::new(m.model().motion_target());
        motion.show_in(m.model().motion_target(), m.model().presented().header, 0.0);
        motion.moving_left(0.0)
    };
    m.layers_settle();
    assert!(
        m.model().open_handed(),
        "come to rest early, the panel's own motion still runs"
    );
    for _ in 0..(open * 60.0).ceil() as usize + 30 {
        m.step(|_, _| {});
        assert!(
            m.model().open_handed(),
            "nothing ends it on the panel's clock"
        );
        assert!(!m.model().animating(), "the open asks for no frame");
    }
    let before = m.presents();
    // an earlier open's timer, come late, ends nothing
    let number = m.model().open_number;
    send(Message::OpenOver(number - 1));
    m.step(|_, _| {});
    assert!(m.model().open_handed(), "another open's timer");
    // the fallback timer, asked for the open's end and a second more
    send(Message::OpenOver(number));
    m.step(|_, _| {});
    assert!(!m.model().open_handed(), "the timer takes it back");
    assert!(m.model().layered(), "at rest as layers");
    m.until_quiet(4, 200);
    assert_eq!(
        m.presents(),
        before,
        "at rest, it drew what its layers show"
    );
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
        // the edge the compositor draws along the glass, where the panel hands it over
        let edge = rest
            .iter()
            .find(|l| l.look.edge.width > 0.0)
            .map(|l| l.look.edge);
        for &after in moments {
            let shown = shown_after(&rest, &changes, after);
            out.push(Moment {
                after,
                size: (w, h),
                pixels: match edge {
                    Some(edge) => compose_with_edge((w, h), scale, &shown, edge),
                    None => compose((w, h), scale, &shown),
                },
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
        let header = m.model().presented().header;
        let mut motion = crate::motion::Motion::new(target);
        motion.show_in(target, header, 0.0);
        if header == crate::motion::Header::Search {
            motion.set_modes(m.model().modes().len(), 0.0, 0.0);
        }
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
/// (the content lies on whole device pixels in both, [`crate::view`]'s `PanelRoot::on_pixels`); along the edge, which
/// the compositor draws along the glass at every moment as the panel strokes its own afresh, within 4. Run with
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
            // the compositor's edge along the glass, as the panel strokes its own
            let edge_bound = 4;
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

/// The search field opening from hidden hands its open to the compositor: the panel draws its layers once, at rest
/// (the field's glass, its content and each mode button's glass and face), the field's glass carrying the edge the
/// compositor draws along the glass group, and asks the compositor to move them from where the open starts, each value
/// through the panel's own; then it draws no frame of its window or a layer, and asks for none, until the open is over,
/// and at rest the glass carries the edge still.
#[test]
fn the_search_field_opens_from_hidden_on_the_compositor_s_clock_without_a_frame() {
    use crate::view::layers::{glass_edge, OPEN_SAMPLES};
    use crate::view::modes::{JOIN_CONTAINER, JOIN_SPACING};
    let mut m = drawn(1.5, true, true);
    send(Message::Toggle);
    m.step(|_, _| {});
    assert!(
        m.model().field_open_handed(),
        "the open is the compositor's"
    );
    for _ in 0..4 {
        if m.presents().0 > 0 {
            break;
        }
        m.step(|_, _| {});
    }
    let first = m.presents();
    let shown = m.shown_layers();
    let count = m.model().modes().len();
    let mut want = vec![GLASS_LAYER, ROW_LAYER, BODY_LAYER];
    for i in 0..count {
        want.extend([button_glass_layer(i), button_face_layer(i)]);
    }
    let keys: Vec<u64> = shown.iter().map(|l| l.key).collect();
    assert_eq!(keys, want, "no edge layer");
    let glass = shown.iter().find(|l| l.key == GLASS_LAYER).unwrap();
    assert_eq!(
        glass.look.edge,
        glass_edge(true),
        "the edge drawn along the glass"
    );
    for l in shown.iter().filter(|l| l.look.glass.is_some()) {
        assert_eq!(
            l.look.group,
            (JOIN_CONTAINER, JOIN_SPACING),
            "one glass group"
        );
    }
    let open = m.layer_requests();
    let of = |key: u64, p: LayerProperty| -> Vec<cupertino_kit::platform::LayerChange> {
        open.iter()
            .filter(|(k, _)| *k == key)
            .flat_map(|(_, c)| c.iter().filter(|c| c.property == p).cloned())
            .collect()
    };
    // the field's glass from 1.1 of the whole group's width to its own, through the panel's values
    let width = of(GLASS_LAYER, LayerProperty::Width);
    assert_eq!(width.len(), 1, "carried through samples");
    let samples = width[0].samples.clone().unwrap();
    assert_eq!(samples.len(), OPEN_SAMPLES);
    let start = f64::from(samples[0]);
    assert!((start - crate::motion::GROUP_WIDTH * crate::motion::ENTER_SCALE).abs() < 1e-3);
    assert_eq!(width[0].to, glass.rect.width());
    // its height never changes, so it is not sent
    assert!(of(GLASS_LAYER, LayerProperty::Height).is_empty());
    // a button buds out of the field's end, a bud at first
    let side = of(button_glass_layer(0), LayerProperty::Width);
    let side = side[0].samples.clone().unwrap();
    assert!(
        (f64::from(side[0]) - crate::modes::BUTTON_SIZE * crate::modes::BUD_START).abs() < 1e-3
    );
    assert_eq!(f64::from(*side.last().unwrap()), crate::modes::BUTTON_SIZE);
    // its symbol fades in and sharpens after a delay
    let blur = of(button_face_layer(0), LayerProperty::ContentBlur);
    let blur = blur[0].samples.clone().unwrap();
    assert_eq!((blur[0], *blur.last().unwrap()), (3.0, 0.0));
    let fade = of(button_face_layer(0), LayerProperty::Opacity);
    let fade = fade[0].samples.clone().unwrap();
    assert_eq!((fade[0], *fade.last().unwrap()), (0.0, 1.0));
    // the open's frames: none drawn, none asked for
    for _ in 0..90 {
        m.step(|_, _| {});
        assert!(!m.model().animating(), "the open asks for no frame");
    }
    assert_eq!(m.presents(), first, "no frame of the window or its layers");
    // the panel's own motion is over, the compositor's may not be: the open ends when the compositor tells
    assert!(
        m.model().open_handed(),
        "the compositor's until its layers rest"
    );
    m.layers_settle();
    assert!(!m.model().open_handed(), "the open is over");
    m.until_quiet(4, 200);
    // at rest the glass carries the edge still
    let rest = m.shown_layers();
    let keys: Vec<u64> = rest.iter().map(|l| l.key).collect();
    assert_eq!(keys, want, "no edge layer at rest either");
    let glass = rest.iter().find(|l| l.key == GLASS_LAYER).unwrap();
    assert_eq!(glass.look.edge, glass_edge(true));
}

/// Under Reduce Motion the search field's open is not handed over: the panel opens drawn in place.
#[test]
fn under_reduce_motion_the_search_field_s_open_is_the_panel_s_own() {
    let mut m = drawn(1.5, true, true);
    m.settle_on(Message::ReduceMotion(true));
    send(Message::Toggle);
    m.step(|_, _| {});
    assert!(!m.model().open_handed());
    assert!(m.layer_requests().is_empty());
}

/// The search field's open the compositor runs against the panel's own drawing of it at the same moments: at each
/// frame the panel draws of its open from hidden (every other one and its last, at 1.5 in both appearances), the
/// compositor's layers at that moment of the open, with the edge it draws along the glass group, against its pixels,
/// and their glass against the glass it stands on (rect, corner and opacity), which it is at every moment. Run with
/// `--no-capture` for the largest differences, and with `CUPERTINO_LAUNCHER_MOMENTS` for the pictures
/// ([`keep_moment`]).
#[test]
fn the_search_field_s_open_the_compositor_runs_is_the_panel_s_own_drawing_at_the_same_moments() {
    for dark in [true, false] {
        let own = open_moments(1.5, dark, false, || Message::Toggle, &[0.0; 80]);
        assert!(own.len() >= 20, "the panel drew its open: {}", own.len());
        let last = own.len() - 1;
        let own: Vec<Moment> = own
            .into_iter()
            .enumerate()
            .filter(|(i, _)| i % 2 == 0 || *i == last)
            .map(|(_, m)| m)
            .collect();
        let moments: Vec<f64> = own.iter().map(|m| m.after).collect();
        let handed = open_moments(1.5, dark, true, || Message::Toggle, &moments);
        // the caret, which the field holds while the compositor runs the open and the panel's own drawing blinks
        let field = handed.last().unwrap().glass[0].0;
        let caret = Rect::new(
            field.x() + crate::view::QUERY_INSET - 8.0,
            field.y(),
            16.0,
            field.height(),
        );
        // where the shapes cross, a stroke round each cut by the others is drawn once by the compositor and by two
        // antialiased strokes and clips by the panel: within an eighth of the line's opacity
        let line = f64::from(crate::view::edge_alpha(dark, true)) * 255.0;
        let bound = (line / 8.0).ceil() as u8;
        let (mut worst, mut worst_caret) = (0, 0);
        for (a, b) in own.iter().zip(&handed) {
            assert_eq!(a.size, b.size);
            let ms = a.after * 1000.0;
            let width = a.size.0;
            let (mut most, mut most_caret) = (0, 0);
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
                if caret.contains(at) {
                    most_caret = most_caret.max(d);
                } else {
                    most = most.max(d);
                }
            }
            worst = worst.max(most);
            worst_caret = worst_caret.max(most_caret);
            eprintln!(
                "trace: field open, dark {dark}, {ms:.0} ms: largest difference {most}, {most_caret} at the caret"
            );
            keep_moment("open-field", dark, a, b);
            assert!(
                most <= bound,
                "dark {dark}, {ms:.0} ms: {most} apart from the caret, over {bound}"
            );
            let shows = |g: &[(Rect, f64, f64)]| -> Vec<(Rect, f64, f64)> {
                g.iter().filter(|g| g.2 > 0.0).copied().collect()
            };
            let (ga, gb) = (shows(&a.glass), shows(&b.glass));
            assert_eq!(ga.len(), gb.len(), "{ms:.0} ms: {ga:?} {gb:?}");
            for ((r0, q0, o0), (r1, q1, o1)) in ga.iter().zip(&gb) {
                let off = (r0.x() - r1.x())
                    .abs()
                    .max((r0.y() - r1.y()).abs())
                    .max((r0.width() - r1.width()).abs())
                    .max((r0.height() - r1.height()).abs());
                assert!(off < 0.05, "{ms:.0} ms: glass {r0:?} against {r1:?}");
                assert!(
                    (q0 - q1).abs() < 0.05 && (o0 - o1).abs() < 0.01,
                    "{ms:.0} ms: {q0} {o0} / {q1} {o1}"
                );
            }
        }
        eprintln!(
            "trace: field open, dark {dark}: {} of its {} frames, largest {worst} apart from the caret (bound \
             {bound}), {worst_caret} at the caret",
            own.len(),
            last + 1
        );
        let (first, _) = compare(&own[0].pixels, &handed[0].pixels, own[0].size.0);
        let (end, _) = compare(
            &own.last().unwrap().pixels,
            &handed.last().unwrap().pixels,
            own[0].size.0,
        );
        assert!(
            first <= 1 && end <= 1,
            "dark {dark}: first {first}, last {end}"
        );
    }
}

impl Metered {
    /// How many frames each of the panel's layers drew so far, by key.
    fn layer_frames(&self) -> std::collections::BTreeMap<u64, usize> {
        let panel = self.panel().expect("the panel is shown");
        let mut out = std::collections::BTreeMap::new();
        for l in self
            .driver
            .runtime()
            .window_system()
            .surface_layers(panel)
            .unwrap()
            .iter()
            .flatten()
            .filter(|l| l.frame.is_some())
        {
            *out.entry(l.key).or_default() += 1;
        }
        out
    }

    /// The panel's window frames so far that drew pixels, not those that commit with no damage to show its layers'.
    fn window_frames(&self) -> usize {
        let panel = self.panel().expect("the panel is shown");
        self.driver
            .runtime()
            .window_system()
            .frames(panel)
            .unwrap()
            .iter()
            .filter(|f| !f.pixels.is_empty() && !f.damage.is_empty())
            .count()
    }
}

/// How a scope opens from hidden.
type Opening = fn() -> Message;

/// The scopes a fold folds from, and how each opens.
const FOLDING: [(&str, Opening); 2] = [
    ("the browser", || Message::Browse),
    ("the files scope", || Message::Files),
];

/// Escape from a scope at rest as layers hands its fold into the search field to the compositor: the glass moves
/// through the panel's own values on the shape spring from the scope to the field, its corner held to half its height,
/// the body fades through its own curve and the row does not, and the edge is the compositor's along the glass. Once
/// the fold began, the panel draws no frame of its window and none of a layer but one of the query row, at the
/// header's switch, and asks for none, until the compositor tells the layers came to rest.
#[test]
fn a_scope_folds_on_the_compositor_s_clock_drawing_its_row_once() {
    use crate::view::layers::{glass_edge, OPEN_SAMPLES};
    for (what, open) in FOLDING {
        let mut m = drawn(1.5, true, true);
        m.settle_on(open());
        assert!(m.model().layered(), "{what}: at rest as layers");
        let asked = m.layer_requests().len();
        let resting = (m.window_frames(), m.layer_frames());
        m.step(|s, w| key(s, w, ESCAPE, None, Mods::NONE));
        assert!(
            m.model().fold_handed(),
            "{what}: the fold is the compositor's"
        );
        eprintln!(
            "trace: fold {what}: at rest {resting:?}, at Escape {} window frames, layers {:?}",
            m.window_frames(),
            m.layer_frames()
        );
        let start = CLOCK.get();
        let length = m.model().motion.shaping_left(start);
        let switch = m
            .model()
            .motion
            .header_switch(start)
            .expect("the header switches part way");
        let keys: Vec<u64> = m.shown_layers().iter().map(|l| l.key).collect();
        assert_eq!(
            keys,
            scope_layers(what),
            "{what}: the edge drawn along the glass"
        );
        let glass = m
            .shown_layers()
            .into_iter()
            .find(|l| l.key == GLASS_LAYER)
            .unwrap();
        assert_eq!(glass.look.edge, glass_edge(true), "{what}");
        let changes = m.layer_requests()[asked..].to_vec();
        let of = |key: u64, p: LayerProperty| -> Vec<cupertino_kit::platform::LayerChange> {
            changes
                .iter()
                .filter(|(k, _)| *k == key)
                .flat_map(|(_, c)| c.iter().filter(|c| c.property == p).cloned())
                .collect()
        };
        // the glass from the scope's shape to the field's, through the panel's own values
        match of(GLASS_LAYER, LayerProperty::Width).first() {
            Some(width) => {
                let samples = width.samples.clone().expect("samples");
                assert_eq!(samples.len(), OPEN_SAMPLES);
                assert_eq!(f64::from(samples[0]), glass.rect.width(), "{what}");
                assert!(
                    (f64::from(*samples.last().unwrap()) - crate::motion::GROUP_WIDTH).abs() < 1e-3,
                    "{what}: to the field's whole width, {}",
                    samples.last().unwrap()
                );
            }
            // as wide as the field: its width is left out
            None => assert_eq!(glass.rect.width(), crate::motion::GROUP_WIDTH, "{what}"),
        }
        let height = of(GLASS_LAYER, LayerProperty::Height)[0]
            .samples
            .clone()
            .unwrap();
        let corner = of(GLASS_LAYER, LayerProperty::CornerRadius)[0]
            .samples
            .clone()
            .unwrap();
        for (h, r) in height.iter().zip(corner.iter()) {
            assert!(
                *r <= h / 2.0 + 1e-4,
                "{what}: the corner held to half the height"
            );
        }
        assert_eq!(
            *corner.last().unwrap(),
            *height.last().unwrap() / 2.0,
            "{what}: a capsule"
        );
        // the body fades out on its own curve, the row not at all
        let fade = of(BODY_LAYER, LayerProperty::Opacity);
        let fade = fade[0]
            .samples
            .clone()
            .expect("the body fades through samples");
        assert_eq!((fade[0], *fade.last().unwrap()), (1.0, 0.0), "{what}");
        assert!(of(ROW_LAYER, LayerProperty::Opacity).is_empty(), "{what}");
        assert_eq!(
            of(ROW_LAYER, LayerProperty::X).len(),
            of(GLASS_LAYER, LayerProperty::X).len(),
            "{what}: the row keeps to the enclosure's left edge"
        );
        assert!(
            of(ROW_LAYER, LayerProperty::Y).is_empty(),
            "{what}: the top edge stays"
        );
        // the fold's first frame lays the edge along the glass; from then on the row's one frame at the switch alone
        m.step(|_, _| {});
        let (windows, layers) = (m.window_frames(), m.layer_frames());
        eprintln!(
            "trace: fold {what}: {length:.3} s, the header switching at {:.3} s; at its first frame {windows} window \
             frames, layers {layers:?}",
            switch.0 - start
        );
        let mut steps = 0;
        let number = m.model().open_number;
        let mut switched = false;
        while CLOCK.get() < start + length + 0.1 {
            // the timer the panel asked for the switch, on this host's clock
            if !switched && CLOCK.get() >= switch.0 {
                switched = true;
                send(Message::HeaderSwitch(number));
            }
            m.step(|_, _| {});
            steps += 1;
            assert!(!m.model().animating(), "{what}: the fold asks for no frame");
        }
        assert!(
            m.model().fold_handed(),
            "{what}: until the compositor tells"
        );
        let mut more = m.layer_frames();
        for (k, n) in &layers {
            *more.entry(*k).or_default() -= n;
        }
        more.retain(|_, n| *n > 0);
        eprintln!("trace: fold {what}: over {steps} steps, new frames {more:?}");
        assert_eq!(m.window_frames(), windows, "{what}: no frame of the window");
        assert_eq!(
            more.into_iter().collect::<Vec<_>>(),
            vec![(ROW_LAYER, 1)],
            "{what}: one frame of the row, at the switch"
        );
        m.layers_settle();
        assert!(!m.model().fold_handed(), "{what}: the fold is over");
        assert_eq!(m.model().presented().header, crate::motion::Header::Search);
    }
}

/// Under Reduce Motion nothing of a fold is handed over: the panel draws it in place.
#[test]
fn under_reduce_motion_a_scope_s_fold_is_the_panel_s_own() {
    let mut m = drawn(1.5, true, true);
    m.settle_on(Message::ReduceMotion(true));
    m.settle_on(Message::Browse);
    let asked = m.layer_requests().len();
    m.step(|s, w| key(s, w, ESCAPE, None, Mods::NONE));
    assert!(!m.model().fold_handed());
    assert_eq!(m.layer_requests().len(), asked);
}

/// How a fold or an unfold goes: the view it starts from at rest, opened from hidden by its message, and the scope's
/// shortcut it unfolds by, or Escape for a fold.
#[derive(Clone, Copy)]
struct Folding {
    from: Opening,
    unfold: Option<Opening>,
}

impl Folding {
    /// A scope folding into the search field.
    fn fold(open: Opening) -> Folding {
        Folding {
            from: open,
            unfold: None,
        }
    }

    /// The search field unfolding into a scope by its shortcut.
    fn unfold(shortcut: Opening) -> Folding {
        Folding {
            from: || Message::Toggle,
            unfold: Some(shortcut),
        }
    }

    /// The panel at rest, as the fold or unfold starts from it.
    fn rest(self, m: &mut Metered) {
        m.settle_on((self.from)());
    }

    /// Starts the fold (Escape) or the unfold (the scope's shortcut).
    fn start(self, m: &mut Metered) {
        match self.unfold {
            Some(shortcut) => {
                send(shortcut());
                m.step(|_, _| {});
            }
            None => {
                m.step(|s, w| key(s, w, ESCAPE, None, Mods::NONE));
            }
        }
    }

    /// The view it ends in.
    fn header(self) -> crate::motion::Header {
        match self.unfold.map(|f| f()) {
            Some(Message::Browse) => crate::motion::Header::Browse,
            Some(_) => crate::motion::Header::Files,
            None => crate::motion::Header::Search,
        }
    }
}

/// A scope's fold into the search field, or the field's unfold into a scope, at `scale` in the `dark` or light
/// appearance, from Escape or the scope's shortcut on, the view it starts from at rest (`how`): drawn by the panel
/// itself (`layers` false) at each frame it draws of it, with whether its header had switched; or handed to the
/// compositor and shown at `moments` (seconds after it began, and whether the header had switched by then) by
/// [`shown_after`] and [`compose_with_edge`], the query row's second frame from the switch on.
fn fold_moments(
    scale: f64,
    dark: bool,
    layers: bool,
    how: Folding,
    moments: &[(f64, bool)],
) -> Vec<(Moment, bool)> {
    let mut m = drawn(scale, dark, layers);
    how.rest(&mut m);
    let asked = m.layer_requests().len();
    let seen = m.window_frames();
    how.start(&mut m);
    let start = CLOCK.get();
    let length = m.model().motion.shaping_left(start);
    let materials = |m: &Metered| -> Vec<(Rect, f64, f64)> {
        let panel = m.panel().unwrap();
        m.driver
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
            .collect()
    };
    let mut out = Vec::new();
    if layers {
        assert!(m.model().fold_handed(), "the fold is the compositor's");
        // the first frame, which draws what the layers show as it starts
        m.step(|_, _| {});
        let before = m.shown_layers();
        let changes = m.layer_requests()[asked..].to_vec();
        let (w, h, _) = m.window_pixels();
        let after = match m.model().motion.header_switch(start) {
            Some((switch, _)) => {
                let number = m.model().open_number;
                while CLOCK.get() < switch {
                    m.step(|_, _| {});
                }
                send(Message::HeaderSwitch(number));
                m.step(|_, _| {});
                m.shown_layers()
            }
            None => before.clone(),
        };
        let edge = before
            .iter()
            .find(|l| l.look.edge.width > 0.0)
            .map(|l| l.look.edge)
            .expect("the edge drawn along the glass");
        for &(at, switched) in moments {
            let rest = if switched { &after } else { &before };
            let shown = shown_after(rest, &changes, at);
            out.push((
                Moment {
                    after: at,
                    size: (w, h),
                    pixels: compose_with_edge((w, h), scale, &shown, edge),
                    glass: shown
                        .iter()
                        .filter(|l| l.glass)
                        .map(|l| (l.bounds, l.radius, l.opacity))
                        .collect(),
                    apart: Vec::new(),
                },
                switched,
            ));
        }
        return out;
    }
    // each frame the panel draws, at the time it is drawn, to the fold's end (the buttons bud out later)
    let mut drawn = seen;
    loop {
        let after = CLOCK.get() - start;
        let now = m.window_frames();
        // a frame drawn while the window waits for its new size lays the panel out for a window it is not yet
        let extent = m.model().extent();
        let whole = |v: f64| (v * scale).round() as u32;
        let (w, h, _) = m.window_pixels();
        let laid_out = (whole(extent.width), whole(extent.height)) == (w, h);
        if std::mem::replace(&mut drawn, now) != now && laid_out {
            let (w, h, pixels) = m.window_pixels();
            out.push((
                Moment {
                    after,
                    size: (w, h),
                    pixels,
                    glass: materials(&m),
                    apart: Vec::new(),
                },
                m.model().presented().header == how.header(),
            ));
        }
        if after >= length || out.len() >= 60 {
            break;
        }
        m.step(|_, _| {});
    }
    out
}

/// The fold the compositor runs against the panel's own drawing of it at the same moments: at each frame the panel
/// draws of a scope's fold into the search field (every other one and its last, at 1.5, both scopes and appearances),
/// the compositor's layers at that moment, with the edge it draws along the glass, against its pixels, and their glass
/// against the glass it stands on (rect, corner and opacity), which it is at every moment. Apart from the outline and
/// the caret with the glyph beside it (the field holds its caret while the compositor runs the fold where the panel's
/// own drawing blinks it, and the panel's own moving caret falls a device pixel apart from the row it is drawn over),
/// every frame is the panel's within 1 of 255. Along the outline the compositor's edge and the panel's both follow the
/// glass's outline, from a scope's continuous corners to the field's capsule: within 4 of 255. Run with
/// `--no-capture` for the figures, and with `CUPERTINO_LAUNCHER_MOMENTS` for the pictures ([`keep_moment`]).
#[test]
fn the_fold_the_compositor_runs_is_the_panel_s_own_drawing_at_the_same_moments() {
    for (what, open) in FOLDING {
        for dark in [true, false] {
            let own = fold_moments(1.5, dark, false, Folding::fold(open), &[]);
            assert!(
                own.len() >= 12,
                "{what}: the panel drew its fold: {}",
                own.len()
            );
            let last = own.len() - 1;
            let own: Vec<(Moment, bool)> = own
                .into_iter()
                .enumerate()
                .filter(|(i, _)| i % 2 == 0 || *i == last)
                .map(|(_, m)| m)
                .collect();
            let moments: Vec<(f64, bool)> = own.iter().map(|(m, s)| (m.after, *s)).collect();
            let handed = fold_moments(1.5, dark, true, Folding::fold(open), &moments);
            let line = f64::from(crate::view::edge_alpha(dark, true)) * 255.0;
            let (mut worst, mut worst_edge, mut worst_caret) = (0, 0, 0);
            for ((a, _), (b, _)) in own.iter().zip(&handed) {
                assert_eq!(a.size, b.size);
                let ms = a.after * 1000.0;
                let (rect, radius, _) = a.glass[0];
                let radius = radius.min(rect.width() / 2.0).min(rect.height() / 2.0);
                let caret = Rect::new(
                    rect.x() + crate::view::QUERY_INSET - 8.0,
                    rect.y(),
                    32.0,
                    crate::view::COLLAPSED_HEIGHT,
                );
                let (cx, cy) = (
                    rect.x() + rect.width() / 2.0,
                    rect.y() + rect.height() / 2.0,
                );
                let (hw, hh) = (rect.width() / 2.0, rect.height() / 2.0);
                let width = a.size.0;
                let (mut content, mut edge, mut at_caret) = (0, 0, 0);
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
                    let x = ((i as u32 % width) as f64 + 0.5) / 1.5;
                    let y = ((i as u32 / width) as f64 + 0.5) / 1.5;
                    let qx = (x - cx).abs() - (hw - radius);
                    let qy = (y - cy).abs() - (hh - radius);
                    let outside = qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - radius;
                    if outside.abs() < EDGE_BAND {
                        edge = edge.max(d);
                    } else if caret.contains(Point::new(x, y)) {
                        at_caret = at_caret.max(d);
                    } else {
                        content = content.max(d);
                    }
                }
                worst = worst.max(content);
                worst_edge = worst_edge.max(edge);
                worst_caret = worst_caret.max(at_caret);
                eprintln!(
                    "trace: fold {what}, dark {dark}, {ms:.0} ms: largest difference {content}, {edge} along the \
                     outline, {at_caret} at the caret"
                );
                keep_moment(&format!("fold-{what}"), dark, a, b);
                assert!(
                    content <= 1,
                    "{what}, dark {dark}, {ms:.0} ms: {content} apart from the outline"
                );
                assert!(
                    edge <= 4,
                    "{what}, dark {dark}, {ms:.0} ms: {edge} along the outline"
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
            eprintln!(
                "trace: fold {what}, dark {dark}: {} of its {} frames, largest {worst} apart from the outline and the \
                 caret, {worst_edge} along the outline (the line {line:.0}), {worst_caret} at the caret",
                own.len(),
                last + 1
            );
        }
    }
}

/// The scopes the search field unfolds into by their shortcuts.
const UNFOLDING: [(&str, Opening); 2] = [
    ("the browser", || Message::Browse),
    ("the files scope", || Message::Files),
];

/// A scope's shortcut from the search field at rest as layers hands its unfold to the compositor: the mode buttons
/// merge into the field through the panel's own values, the glass grows through them from the field's capsule to the
/// scope's shape, the scope's body, drawn at once as the scope rests, fades in through its own curve cut to the glass
/// as the glass grows, and the edge is the compositor's along the glass throughout. Once its first frame is drawn the
/// panel draws no frame of its window and none of a layer but one of the query row, at the header's switch (into the
/// files scope; into the browser the header switches at once), and asks for none, until the compositor tells the
/// layers came to rest.
#[test]
fn the_search_field_unfolds_on_the_compositor_s_clock_drawing_its_row_at_most_once() {
    use crate::view::layers::glass_edge;
    for (what, shortcut) in UNFOLDING {
        let mut m = drawn(1.5, true, true);
        m.settle_on(Message::Toggle);
        assert!(m.model().layered(), "{what}: the field at rest as layers");
        let count = m.model().modes().len();
        let rest_glass = m
            .shown_layers()
            .into_iter()
            .find(|l| l.key == GLASS_LAYER)
            .unwrap();
        assert_eq!(rest_glass.look.edge, glass_edge(true), "{what}: at rest");
        let asked = m.layer_requests().len();
        send(shortcut());
        m.step(|_, _| {});
        assert!(
            m.model().fold_handed(),
            "{what}: the unfold is the compositor's"
        );
        let start = CLOCK.get();
        let length = m.model().motion.shaping_left(start);
        let switch = m.model().motion.header_switch(start);
        let changes = m.layer_requests()[asked..].to_vec();
        let of = |key: u64, p: LayerProperty| -> Vec<cupertino_kit::platform::LayerChange> {
            changes
                .iter()
                .filter(|(k, _)| *k == key)
                .flat_map(|(_, c)| c.iter().filter(|c| c.property == p).cloned())
                .collect()
        };
        // the glass from the field's capsule to the scope's shape through the panel's own values
        let height = of(GLASS_LAYER, LayerProperty::Height)[0]
            .samples
            .clone()
            .expect("samples");
        assert_eq!(
            f64::from(height[0]),
            crate::view::COLLAPSED_HEIGHT,
            "{what}"
        );
        let target = m.model().motion_target();
        assert_eq!(f64::from(*height.last().unwrap()), target.height, "{what}");
        // the body fades in on its own curve, cut to the glass as the glass grows; the row does not fade
        let fade = of(BODY_LAYER, LayerProperty::Opacity)[0]
            .samples
            .clone()
            .unwrap();
        assert_eq!((fade[0], *fade.last().unwrap()), (0.0, 1.0), "{what}");
        assert!(of(ROW_LAYER, LayerProperty::Opacity).is_empty(), "{what}");
        assert_eq!(
            of(BODY_LAYER, LayerProperty::Height),
            of(GLASS_LAYER, LayerProperty::Height),
            "{what}: the body's shape is the glass's"
        );
        let body = m
            .shown_layers()
            .into_iter()
            .find(|l| l.key == BODY_LAYER)
            .unwrap();
        assert!(body.look.masks, "{what}: the body is cut to its shape");
        assert_eq!(
            (body.rect.width(), body.rect.height()),
            (target.width, target.height),
            "{what}: the body drawn as the scope rests"
        );
        // the mode buttons merge through the panel's own values, and are gone once merged
        for index in 0..count {
            let shown = of(button_glass_layer(index), LayerProperty::Opacity);
            let face = of(button_face_layer(index), LayerProperty::Opacity);
            match m
                .shown_layers()
                .iter()
                .any(|l| l.key == button_glass_layer(index))
            {
                true => {
                    let shown = shown[0].samples.clone().expect("samples");
                    assert_eq!((shown[0], *shown.last().unwrap()), (1.0, 0.0), "{what}");
                    let face = face[0].samples.clone().expect("samples");
                    assert_eq!(*face.last().unwrap(), 0.0, "{what}");
                }
                // into the browser the header switches at once and the buttons are gone from the first frame
                false => assert!(switch.is_none(), "{what}: buttons gone at once"),
            }
        }
        // the unfold's first frame: the window at its size through the unfold, and every layer drawn as it starts
        m.step(|_, _| {});
        let (windows, layers) = (m.window_frames(), m.layer_frames());
        eprintln!(
            "trace: unfold {what}: {length:.3} s, the header switching at {:?}; at its first frame {windows} window \
             frames, layers {layers:?}",
            switch.map(|(at, _)| at - start)
        );
        let mut steps = 0;
        let number = m.model().open_number;
        let mut switched = false;
        while CLOCK.get() < start + length + 0.1 {
            if let Some((at, _)) = switch.filter(|(at, _)| !switched && CLOCK.get() >= *at) {
                let _ = at;
                switched = true;
                send(Message::HeaderSwitch(number));
            }
            m.step(|_, _| {});
            steps += 1;
            assert!(
                !m.model().animating(),
                "{what}: the unfold asks for no frame"
            );
        }
        assert!(
            m.model().fold_handed(),
            "{what}: until the compositor tells"
        );
        let mut more = m.layer_frames();
        for (k, n) in &layers {
            *more.entry(*k).or_default() -= n;
        }
        more.retain(|_, n| *n > 0);
        eprintln!("trace: unfold {what}: over {steps} steps, new frames {more:?}");
        assert_eq!(m.window_frames(), windows, "{what}: no frame of the window");
        let want = if switch.is_some() {
            vec![(ROW_LAYER, 1)]
        } else {
            Vec::new()
        };
        assert_eq!(
            more.into_iter().collect::<Vec<_>>(),
            want,
            "{what}: one frame of the row at the switch, if it switches part way"
        );
        m.layers_settle();
        assert!(!m.model().fold_handed(), "{what}: the unfold is over");
        assert_eq!(
            m.model().presented().header,
            Folding::unfold(shortcut).header()
        );
        m.until_quiet(4, 200);
        let body = m
            .shown_layers()
            .into_iter()
            .find(|l| l.key == BODY_LAYER)
            .unwrap();
        assert!(!body.look.masks, "{what}: at rest the body is not cut");
    }
}

/// Under Reduce Motion nothing of an unfold is handed over: the panel draws it in place.
#[test]
fn under_reduce_motion_the_search_field_s_unfold_is_the_panel_s_own() {
    for (what, shortcut) in UNFOLDING {
        let mut m = drawn(1.5, true, true);
        m.settle_on(Message::ReduceMotion(true));
        m.settle_on(Message::Toggle);
        let asked = m.layer_requests().len();
        send(shortcut());
        m.step(|_, _| {});
        assert!(!m.model().fold_handed(), "{what}");
        assert_eq!(m.layer_requests().len(), asked, "{what}");
    }
}

/// How far the compositor's drawing `b` of a moment is from the panel's own `a` at 1.5: `(apart from the outline and
/// the caret, along the outline, at the caret)`, the outline that of each glass shape `a` shows (the enclosure's and
/// the mode buttons'), the caret the field's at the enclosure's left.
fn apart_along_outline(a: &Moment, b: &Moment) -> (u8, u8, u8) {
    let shapes: Vec<cupertino_glass::glass::GlassShape> = a
        .glass
        .iter()
        .filter(|g| g.2 > 0.0)
        .map(|(r, radius, _)| {
            cupertino_glass::glass::GlassShape::new(
                [
                    r.x() as f32,
                    r.y() as f32,
                    r.width() as f32,
                    r.height() as f32,
                ],
                *radius as f32,
            )
        })
        .collect();
    let (rect, _, _) = a.glass[0];
    let caret = Rect::new(
        rect.x() + crate::view::QUERY_INSET - 8.0,
        rect.y(),
        32.0,
        crate::view::COLLAPSED_HEIGHT,
    );
    let width = a.size.0;
    let (mut content, mut edge, mut at_caret) = (0, 0, 0);
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
        let x = ((i as u32 % width) as f64 + 0.5) / 1.5;
        let y = ((i as u32 / width) as f64 + 0.5) / 1.5;
        let near = shapes
            .iter()
            .any(|s| f64::from(s.sdf([x as f32, y as f32])).abs() < EDGE_BAND);
        if near {
            edge = edge.max(d);
        } else if caret.contains(Point::new(x, y)) {
            at_caret = at_caret.max(d);
        } else {
            content = content.max(d);
        }
    }
    (content, edge, at_caret)
}

/// The layers of the scope `what` names at rest: the glass, the query row, the browser's view options button, the body.
fn scope_layers(what: &str) -> Vec<u64> {
    if what == "the browser" {
        vec![GLASS_LAYER, ROW_LAYER, OPTIONS_LAYER, BODY_LAYER]
    } else {
        vec![GLASS_LAYER, ROW_LAYER, BODY_LAYER]
    }
}

/// How far the unfold the compositor runs may stand from the panel's own drawing along the outline, of 255: where the
/// body's content crosses the glass's moving edge the panel's own drawing cuts it a fraction of a device pixel further
/// in than the compositor's cut (20 of 255 at most), and while the files scope's shape waits as the field's capsule
/// under the scope's header the panel's own edge parts from the compositor's along the capsule's ends by up to 18.
const UNFOLD_ALONG_OUTLINE: u8 = 20;

/// The unfold the compositor runs against the panel's own drawing of it at the same moments: at each frame the panel
/// draws of the search field's unfold into a scope by its shortcut (every other one and its last, at 1.5, both scopes
/// and appearances), the compositor's layers at that moment, the body cut to the glass and the edge drawn along it,
/// against its pixels, and their glass against the glass it stands on (rect, corner and opacity), which it is at every
/// moment. The last frame is the panel's within 1 of 255; the first and those part way, apart from the outline and the
/// caret (the field holds its caret while the compositor runs the unfold), within 1, and along the outline, which the
/// compositor's edge and the panel's both follow from the field's capsule to a scope's continuous corners, within
/// [`UNFOLD_ALONG_OUTLINE`].
/// Run with `--no-capture` for the figures, and with `CUPERTINO_LAUNCHER_MOMENTS` for the pictures ([`keep_moment`]).
#[test]
fn the_unfold_the_compositor_runs_is_the_panel_s_own_drawing_at_the_same_moments() {
    for (what, shortcut) in UNFOLDING {
        for dark in [true, false] {
            let how = Folding::unfold(shortcut);
            let mut own = fold_moments(1.5, dark, false, how, &[]);
            // the window grows to hold the scope as the unfold starts: a frame the panel drew before at the field's
            // size is not the unfold's
            let size = own.last().map(|(m, _)| m.size);
            own.retain(|(m, _)| Some(m.size) == size);
            assert!(
                own.len() >= 8,
                "{what}: the panel drew its unfold: {}",
                own.len()
            );
            let last = own.len() - 1;
            let own: Vec<(Moment, bool)> = own
                .into_iter()
                .enumerate()
                .filter(|(i, _)| i % 2 == 0 || *i == last)
                .map(|(_, m)| m)
                .collect();
            let moments: Vec<(f64, bool)> = own.iter().map(|(m, s)| (m.after, *s)).collect();
            let handed = fold_moments(1.5, dark, true, how, &moments);
            let line = f64::from(crate::view::edge_alpha(dark, true)) * 255.0;
            let (mut worst, mut worst_edge, mut worst_caret) = (0, 0, 0);
            let shown = |g: &[(Rect, f64, f64)]| -> Vec<(Rect, f64, f64)> {
                g.iter().filter(|g| g.2 > 0.0).copied().collect()
            };
            for (n, ((a, _), (b, _))) in own.iter().zip(&handed).enumerate() {
                assert_eq!(a.size, b.size);
                let ms = a.after * 1000.0;
                let (content, edge, at_caret) = apart_along_outline(a, b);
                worst = worst.max(content);
                worst_edge = worst_edge.max(edge);
                worst_caret = worst_caret.max(at_caret);
                eprintln!(
                    "trace: unfold {what}, dark {dark}, {ms:.0} ms: largest difference {content}, {edge} along the \
                     outline, {at_caret} at the caret"
                );
                keep_moment(&format!("unfold-{what}"), dark, a, b);
                if n == own.len() - 1 {
                    let (most, _) = compare(&a.pixels, &b.pixels, a.size.0);
                    assert!(
                        most <= 1 || at_caret == most,
                        "{what}, dark {dark}, {ms:.0} ms: the last frame {most} apart"
                    );
                }
                assert!(
                    content <= 1,
                    "{what}, dark {dark}, {ms:.0} ms: {content} apart from the outline"
                );
                assert!(
                    edge <= UNFOLD_ALONG_OUTLINE,
                    "{what}, dark {dark}, {ms:.0} ms: {edge} along the outline"
                );
                let (ga, gb) = (shown(&a.glass), shown(&b.glass));
                assert_eq!(ga.len(), gb.len(), "{what} {ms:.0} ms: {ga:?} {gb:?}");
                for ((r0, q0, o0), (r1, q1, o1)) in ga.iter().zip(&gb) {
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
            eprintln!(
                "trace: unfold {what}, dark {dark}: {} of its {} frames, largest {worst} apart from the outline and \
                 the caret, {worst_edge} along the outline (the line {line:.0}), {worst_caret} at the caret",
                own.len(),
                last + 1
            );
        }
    }
}
