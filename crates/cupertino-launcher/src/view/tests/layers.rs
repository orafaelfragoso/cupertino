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
    send(Message::Toggle);
    m.step(|_, _| {});
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
