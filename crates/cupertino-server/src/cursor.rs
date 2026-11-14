//! Cursor images: themed cursors loaded through `xcursor` (by name, for the cursor-shape
//! protocol), recoloured when the user picked other cursor colours, converted to memory render
//! buffers, with a built-in arrow as the fallback; and shake to locate, which grows the cursor
//! while the pointer is shaken (`cupertino_input::ShakeDetector`).

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use cupertino_assets::CursorTheme;
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::input::pointer::CursorIcon;
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::calloop::RegistrationToken;
use smithay::utils::{Logical, Physical, Point, Transform};
use xcursor::parser::{parse_xcursor, Image};

use crate::state::Cupertino;

/// One cursor frame ready for rendering.
pub struct CursorFrame {
    /// The image at buffer scale 1.
    pub buffer: MemoryRenderBuffer,
    /// Hotspot in image pixels.
    pub hotspot: Point<i32, Physical>,
    /// Width and height of the image, pixels.
    pub pixels: (u32, u32),
    /// Nominal size of the image (the XCursor size it was drawn for), pixels.
    pub nominal: u32,
}

impl CursorFrame {
    /// Logical points per image pixel for a cursor drawn `size` points nominal (the nominal size times the
    /// cursor scale): the image's own scale, whatever the output's.
    fn points_per_pixel(&self, size: f64) -> f64 {
        size / f64::from(self.nominal)
    }

    /// The hotspot in logical points from the image's top-left corner, for a cursor drawn `size` points nominal.
    pub fn hotspot_at(&self, size: f64) -> Point<f64, Logical> {
        let k = self.points_per_pixel(size);
        Point::from((f64::from(self.hotspot.x) * k, f64::from(self.hotspot.y) * k))
    }

    /// The image's size in logical points for a cursor drawn `size` points nominal.
    pub fn size_at(&self, size: f64) -> (f64, f64) {
        let k = self.points_per_pixel(size);
        (f64::from(self.pixels.0) * k, f64::from(self.pixels.1) * k)
    }
}

/// Fill and outline colours (sRGB, straight alpha) of the cursors the server draws. The design
/// stores both and redraws its cursors with them once the user changed either.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CursorColors {
    pub fill: [u8; 4],
    pub outline: [u8; 4],
}

impl CursorColors {
    /// Black fill, white outline.
    pub const DEFAULT: CursorColors = CursorColors {
        fill: [0, 0, 0, 255],
        outline: [255, 255, 255, 255],
    };
}

/// A cursor's name and the index of its image in the theme's list: each animation frame has a buffer of its own.
type BufferKey = (&'static str, usize);
type CursorBuffer = (Point<i32, Physical>, MemoryRenderBuffer);

/// The cursors the server draws, read from the XCursor theme of the asset directory only (no host theme).
pub struct CursorLoader {
    theme: Option<CursorTheme>,
    size: u32,
    colors: CursorColors,
    cache: HashMap<&'static str, Vec<Image>>,
    buffers: HashMap<BufferKey, CursorBuffer>,
}

/// Built-in arrow (white with a black outline), `size` pixels tall.
pub fn fallback_arrow(size: u32) -> Image {
    let s = size.max(12) as usize;
    let mut px = vec![0u8; s * s * 4];
    let h = s as f32;
    // triangle (0,0) - (0.55h,0.8h)... classic pointer silhouette
    let inside = |x: f32, y: f32, grow: f32| -> bool {
        let (x, y) = (x / h, y / h);
        let g = grow / h;
        y >= -g
            && y <= 0.82 + g
            && x >= -g
            && x <= 0.58 * (y.max(0.0) + 0.0) + 0.18 + g * 2.0
            && x <= 0.70 * y + g * 2.0 + 0.02
    };
    for y in 0..s {
        for x in 0..s {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let i = (y * s + x) * 4;
            if inside(fx, fy, -1.2) {
                px[i..i + 4].copy_from_slice(&[255, 255, 255, 255]);
            } else if inside(fx, fy, 0.8) {
                px[i..i + 4].copy_from_slice(&[0, 0, 0, 255]);
            }
        }
    }
    Image {
        size,
        width: s as u32,
        height: s as u32,
        xhot: 1,
        yhot: 1,
        delay: 0,
        pixels_rgba: px,
        pixels_argb: vec![],
    }
}

/// Redraw a black-filled, white-outlined cursor image with `colors`.
///
/// approximate: the design redraws its vector cursors with the two colours; a
/// raster theme has only pixels, so each pixel's lightness picks between the outline (light) and
/// the fill (dark) colour. Dark pixels that are not opaque are the drop shadow, which stays black:
/// a cursor's fill is always opaque, inside its outline. `pixels` is XCursor data: premultiplied
/// ARGB32, little-endian (bytes B, G, R, A).
pub fn recolor(pixels: &mut [u8], colors: CursorColors) {
    let channel = |c: [u8; 4], i: usize| f32::from(c[i]) / 255.0;
    for px in pixels.as_chunks_mut::<4>().0 {
        let a = f32::from(px[3]) / 255.0;
        if a == 0.0 {
            continue;
        }
        let light =
            ((f32::from(px[0]) + f32::from(px[1]) + f32::from(px[2])) / (3.0 * 255.0) / a).min(1.0);
        let dark = if px[3] == 255 {
            colors.fill
        } else {
            [0, 0, 0, 255]
        };
        let mix = |i: usize| channel(colors.outline, i) * light + channel(dark, i) * (1.0 - light);
        let alpha = a * mix(3);
        let q = |v: f32| (v * alpha * 255.0).round().clamp(0.0, 255.0) as u8;
        // straight sRGB components 0 = red, 1 = green, 2 = blue, written as B, G, R
        let (r, g, b) = (mix(0), mix(1), mix(2));
        *px = [q(b), q(g), q(r), (alpha * 255.0).round() as u8];
    }
}

impl CursorLoader {
    /// The asset packs' `theme`, at the nominal size `XCURSOR_SIZE` (default 24), drawn in `colors`. Without a theme,
    /// or with one that has neither a `default` nor a `left_ptr` cursor (reported on stderr), the cursors are the
    /// built-in procedural set (`cupertino-cursor`), and the clients the server starts keep their own cursor theme.
    pub fn load(theme: Option<CursorTheme>, colors: CursorColors) -> Self {
        let size = std::env::var("XCURSOR_SIZE")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(24);
        let mut loader = Self {
            theme,
            size,
            colors,
            cache: HashMap::new(),
            buffers: HashMap::new(),
        };
        if let Some(t) = loader.theme.as_ref().filter(|_| {
            ["default", "left_ptr"]
                .iter()
                .all(|n| loader.read(n).is_none())
        }) {
            eprintln!("cupertino-server: the cursor theme {:?} in {} has no `default` or `left_ptr` cursor; drawing the built-in cursors", t.name, t.search_dir.display());
            loader.theme = None;
        }
        loader
    }

    /// Draw the cursors in `colors` from now on (`cursor_fill`, `cursor_outline` changed): the images are
    /// recoloured as they are next used.
    pub fn set_colors(&mut self, colors: CursorColors) {
        if self.colors != colors {
            self.colors = colors;
            self.cache.clear();
            self.buffers.clear();
        }
    }

    /// The theme the cursors are loaded from, `None` for the built-in arrow. The clients the server starts get its
    /// name as `XCURSOR_THEME` and its directory as `XCURSOR_PATH`, so their own cursors match the ones the server
    /// draws and they find no other theme.
    pub fn theme(&self) -> Option<&CursorTheme> {
        self.theme.as_ref()
    }

    /// The images of the cursor `name`: the file of the theme (aliases are symlinks inside the theme), or without a
    /// theme the procedural cursor. `None` when the theme has no such file or the procedural set no such cursor.
    fn read(&self, name: &str) -> Option<Vec<Image>> {
        let Some(theme) = self.theme.as_ref() else {
            return self.procedural(name);
        };
        let path: PathBuf = theme
            .search_dir
            .join(&theme.name)
            .join("cursors")
            .join(name);
        parse_xcursor(&std::fs::read(path).ok()?)
    }

    /// The procedural cursor `name` at one, two and four times the nominal size (output scales and the cursor scale
    /// pick from them as from a theme's sizes).
    fn procedural(&self, name: &str) -> Option<Vec<Image>> {
        let kind = cupertino_cursor::procedural::Kind::from_name(name)?;
        let mut options = cupertino_cursor::theme::BuildOptions::new(PathBuf::new());
        options.sizes = vec![self.size, self.size * 2, self.size * 4];
        let file = cupertino_cursor::theme::procedural_cursor(kind, &options, Vec::new())
            .to_xcursor()
            .unwrap_or_else(|e| panic!("procedural cursor {name}: {e}"));
        parse_xcursor(&file)
    }

    /// Nominal cursor size at scale 1, pixels per logical point.
    pub fn size(&self) -> u32 {
        self.size
    }

    /// `XCURSOR_SIZE` for the clients the server starts, so the cursors they draw follow the user
    /// cursor scale: the nominal size times `scale`, rounded.
    pub fn client_size(&self, scale: f64) -> u32 {
        (f64::from(self.size) * scale).round() as u32
    }

    fn images(&mut self, icon: CursorIcon) -> &Vec<Image> {
        let name = icon.name();
        if !self.cache.contains_key(name) {
            let mut imgs = self
                .read(name)
                .or_else(|| self.read("default"))
                .or_else(|| self.read("left_ptr"))
                .unwrap_or_else(|| vec![fallback_arrow(self.size)]);
            if self.colors != CursorColors::DEFAULT {
                for img in &mut imgs {
                    recolor(&mut img.pixels_rgba, self.colors);
                }
            }
            self.cache.insert(name, imgs);
        }
        &self.cache[name]
    }

    /// Frame for `icon` drawn `pixels` tall (nominal size times cursor scale times output scale)
    /// at animation time `time`: the smallest image of the theme at least that size, else the
    /// largest, so the renderer scales down where it can.
    pub fn frame(&mut self, icon: CursorIcon, pixels: u32, time: Duration) -> CursorFrame {
        let name = icon.name();
        let index = pick_frame(self.images(icon), pixels, time.as_millis() as u32);
        // the image stays in the cache: only its pixels' first frame makes a buffer of them
        let img = &self.cache[name][index];
        let key = (name, index);
        let entry = self.buffers.entry(key).or_insert_with(|| {
            let buf = MemoryRenderBuffer::from_slice(
                &img.pixels_rgba,
                Fourcc::Argb8888,
                (img.width as i32, img.height as i32),
                1,
                Transform::Normal,
                None,
            );
            (Point::from((img.xhot as i32, img.yhot as i32)), buf)
        });
        CursorFrame {
            buffer: entry.1.clone(),
            hotspot: entry.0,
            pixels: (img.width, img.height),
            nominal: img.size,
        }
    }
}

/// The image for `size`: the smallest nominal size at least `size`, else the largest; then the
/// animation frame of that size at `millis`.
fn pick_frame(images: &[Image], size: u32, millis: u32) -> usize {
    let largest = images
        .iter()
        .map(|i| i.size)
        .max()
        .expect("at least one image");
    let nominal = images
        .iter()
        .map(|i| i.size)
        .filter(|s| *s >= size)
        .min()
        .unwrap_or(largest);
    let same: Vec<usize> = (0..images.len())
        .filter(|&i| images[i].size == nominal)
        .collect();
    let total: u32 = same.iter().map(|&i| images[i].delay).sum();
    if total == 0 {
        return same[0];
    }
    let mut m = millis % total;
    for &i in &same {
        if m < images[i].delay {
            return i;
        }
        m -= images[i].delay;
    }
    same[0]
}

/// Shake to locate: the detector and the timer that runs its evaluations while the cursor is grown.
#[derive(Debug, Default)]
pub struct CursorShake {
    detector: cupertino_input::ShakeDetector,
    timer: Option<RegistrationToken>,
}

impl Cupertino {
    /// Feed a pointer motion to `pos` to the shake detector. A recognised shake grows the cursor
    /// and starts the evaluations that shrink it again.
    pub fn shake_motion(&mut self, pos: Point<f64, Logical>) {
        if !self.config.pointer.shake_to_locate {
            return;
        }
        let now = self.now();
        if !self
            .cursor_shake
            .detector
            .motion(cupertino_input::Point::new(pos.x, pos.y), now)
        {
            return;
        }
        self.request_redraw();
        if self.cursor_shake.timer.is_some() {
            return;
        }
        let interval = Duration::from_secs_f64(cupertino_input::SHAKE_EVALUATION_INTERVAL_S);
        let timer =
            self.loop_handle
                .insert_source(Timer::from_duration(interval), move |_, _, st| {
                    if st.evaluate_cursor_shake() {
                        TimeoutAction::ToDuration(interval)
                    } else {
                        st.cursor_shake.timer = None;
                        TimeoutAction::Drop
                    }
                });
        match timer {
            Ok(token) => self.cursor_shake.timer = Some(token),
            Err(e) => {
                // without the evaluations the cursor would stay grown: give the shake up
                tracing::error!(error = %e, "shake-to-locate timer not inserted; the cursor keeps its size");
                self.cursor_shake.detector.release();
            }
        }
    }

    /// One evaluation of the shake multiplier at the current time; true while the cursor is
    /// still grown.
    pub fn evaluate_cursor_shake(&mut self) -> bool {
        let before = self.cursor_shake.detector.multiplier();
        let now = self.now();
        let after = self.cursor_shake.detector.evaluate(now);
        if after != before {
            self.request_redraw();
        }
        after > 1.0
    }

    /// The shake multiplier, 1 while the cursor has its normal size.
    pub fn shake_multiplier(&self) -> f64 {
        self.cursor_shake.detector.multiplier()
    }

    /// Scale of the cursors the server draws: the user cursor scale times the shake multiplier
    /// (product), kept below the display server's limit.
    pub fn cursor_scale(&self) -> f64 {
        (self.config.pointer.cursor_scale * self.shake_multiplier())
            .min(cupertino_input::CURSOR_SCALE_LIMIT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_arrow_has_visible_pixels() {
        let a = fallback_arrow(24);
        let opaque = a.pixels_rgba.chunks(4).filter(|p| p[3] == 255).count();
        assert!(opaque > 40, "{opaque}");
        assert_eq!(a.pixels_rgba.len(), (a.width * a.height * 4) as usize);
    }

    fn image(size: u32, delay: u32, v: u8) -> Image {
        Image {
            size,
            width: size,
            height: size,
            xhot: 0,
            yhot: 0,
            delay,
            pixels_rgba: vec![v; (size * size * 4) as usize],
            pixels_argb: vec![],
        }
    }

    #[test]
    fn frame_selection_follows_delay() {
        let imgs = vec![image(24, 100, 1), image(24, 100, 2)];
        let pick = |millis| &imgs[pick_frame(&imgs, 24, millis)];
        assert_eq!(pick(50).pixels_rgba[0], 1);
        assert_eq!(pick(150).pixels_rgba[0], 2);
        assert_eq!(pick(250).pixels_rgba[0], 1);
        // frames of one size, delay and hotspot are told apart, so each keeps a buffer of its own
        assert_ne!(pick_frame(&imgs, 24, 50), pick_frame(&imgs, 24, 150));
    }

    #[test]
    fn each_frame_of_an_animated_cursor_is_drawn_from_a_buffer_of_its_own() {
        // two frames of one size, delay and hotspot that differ only in their pixels
        let mut loader = CursorLoader::load(None, CursorColors::DEFAULT);
        loader.cache.insert(
            CursorIcon::Default.name(),
            vec![image(24, 100, 1), image(24, 100, 2)],
        );
        for millis in [50, 150, 250, 350] {
            loader.frame(CursorIcon::Default, 24, Duration::from_millis(millis));
        }
        assert_eq!(
            loader.buffers.len(),
            2,
            "the second frame is not shown from the first one's buffer"
        );
    }

    #[test]
    fn the_smallest_image_at_least_the_size_is_picked() {
        let imgs = vec![image(24, 0, 1), image(48, 0, 2), image(64, 0, 3)];
        let size = |pixels| imgs[pick_frame(&imgs, pixels, 0)].size;
        assert_eq!(size(24), 24);
        assert_eq!(size(30), 48, "scaled down from 48 rather than up from 24");
        assert_eq!(
            size(120),
            64,
            "past the largest image the largest is scaled up"
        );
    }

    #[test]
    fn recolouring_maps_fill_and_outline_and_keeps_the_shadow_black() {
        let colors = CursorColors {
            fill: [255, 0, 0, 255],
            outline: [0, 0, 255, 255],
        };
        // B, G, R, A premultiplied: opaque black (fill), opaque white (outline), half-transparent black
        // (shadow), half-transparent white (antialiased outline edge), transparent
        let mut px = vec![
            0, 0, 0, 255, 255, 255, 255, 255, 0, 0, 0, 128, 128, 128, 128, 128, 0, 0, 0, 0,
        ];
        recolor(&mut px, colors);
        assert_eq!(&px[0..4], &[0, 0, 255, 255], "fill → red");
        assert_eq!(&px[4..8], &[255, 0, 0, 255], "outline → blue");
        assert_eq!(&px[8..12], &[0, 0, 0, 128], "shadow stays black");
        assert_eq!(
            &px[12..16],
            &[128, 0, 0, 128],
            "outline edge → blue, same coverage"
        );
        assert_eq!(&px[16..20], &[0, 0, 0, 0]);
    }

    /// A one-image XCursor file: a `size` x `size` image filled with `argb`.
    fn xcursor_file(size: u32, argb: u32) -> Vec<u8> {
        let mut f = Vec::new();
        let words = |f: &mut Vec<u8>, w: &[u32]| {
            w.iter().for_each(|v| f.extend_from_slice(&v.to_le_bytes()))
        };
        f.extend_from_slice(b"Xcur");
        words(
            &mut f,
            &[
                16,
                1,
                1,
                0xfffd_0002,
                size,
                28,
                36,
                0xfffd_0002,
                size,
                1,
                size,
                size,
                0,
                0,
                0,
            ],
        );
        (0..size * size).for_each(|_| words(&mut f, &[argb]));
        f
    }

    fn theme_dir(files: &[(&str, u32)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let cursors = dir.path().join("fixture/cursors");
        std::fs::create_dir_all(&cursors).unwrap();
        for (name, size) in files {
            std::fs::write(cursors.join(name), xcursor_file(*size, 0xff00_00ff)).unwrap();
        }
        dir
    }

    fn theme(dir: &tempfile::TempDir) -> CursorTheme {
        CursorTheme {
            name: "fixture".into(),
            search_dir: dir.path().to_path_buf(),
        }
    }

    #[test]
    fn cursors_come_from_the_asset_theme_with_default_for_unknown_names() {
        let dir = theme_dir(&[("default", 24), ("text", 32)]);
        let mut loader = CursorLoader::load(Some(theme(&dir)), CursorColors::DEFAULT);
        assert_eq!(loader.theme(), Some(&theme(&dir)));
        assert_eq!(
            loader.frame(CursorIcon::Text, 32, Duration::ZERO).nominal,
            32
        );
        assert_eq!(
            loader
                .frame(CursorIcon::Pointer, 24, Duration::ZERO)
                .nominal,
            24,
            "an unknown name draws `default`"
        );
    }

    #[test]
    fn without_a_usable_theme_the_cursors_are_the_procedural_set() {
        let dir = theme_dir(&[("text", 32)]);
        for theme in [Some(theme(&dir)), None] {
            let mut loader = CursorLoader::load(theme, CursorColors::DEFAULT);
            assert_eq!(
                loader.theme(),
                None,
                "a theme without `default` or `left_ptr` is dropped"
            );
            let text = loader.frame(CursorIcon::Text, 48, Duration::ZERO);
            assert_eq!(text.nominal, 48, "drawn at twice the nominal 24");
            let arrow = fallback_arrow(24);
            assert_ne!(
                loader.frame(CursorIcon::Default, 24, Duration::ZERO).pixels,
                (arrow.width, arrow.height),
                "not the last-resort arrow"
            );
        }
    }

    /// The built-in cursors keep each role's canvas and hotspot in logical points at output scales 1, 1.5 and 2 and
    /// at a doubled cursor size: the image picked for the output's pixels is scaled so its hotspot stays on the
    /// pointer's point.
    #[test]
    fn built_in_hotspots_stay_on_the_pointer_at_every_output_scale() {
        let mut loader = CursorLoader {
            theme: None,
            size: 24,
            colors: CursorColors::DEFAULT,
            cache: HashMap::new(),
            buffers: HashMap::new(),
        };
        for (icon, canvas, hotspot) in [
            (CursorIcon::Default, (28.0, 40.0), (5.0, 5.0)),
            (CursorIcon::Text, (23.0, 22.0), (12.0, 11.0)),
            (CursorIcon::Pointer, (32.0, 32.0), (12.0, 8.0)),
            (CursorIcon::Grab, (32.0, 32.0), (15.0, 16.0)),
            (CursorIcon::Grabbing, (32.0, 32.0), (15.0, 16.0)),
            (CursorIcon::ColResize, (30.0, 24.0), (15.0, 12.0)),
            (CursorIcon::RowResize, (24.0, 28.0), (12.0, 14.0)),
        ] {
            for cursor_scale in [1.0, 2.0] {
                for output_scale in [1.0, 1.5, 2.0] {
                    // as `Cupertino::cursor_plan` asks for it
                    let size = f64::from(loader.size()) * cursor_scale;
                    let frame =
                        loader.frame(icon, (size * output_scale).ceil() as u32, Duration::ZERO);
                    assert!(
                        f64::from(frame.pixels.1) >= canvas.1 * cursor_scale * output_scale,
                        "{icon:?} drawn from an image at least as large as the output needs"
                    );
                    let at = frame.hotspot_at(size);
                    assert_eq!(
                        (at.x, at.y),
                        (hotspot.0 * cursor_scale, hotspot.1 * cursor_scale),
                        "{icon:?} cursor ×{cursor_scale} output ×{output_scale}"
                    );
                    assert_eq!(
                        frame.size_at(size),
                        (canvas.0 * cursor_scale, canvas.1 * cursor_scale),
                        "{icon:?} cursor ×{cursor_scale} output ×{output_scale}"
                    );
                }
            }
        }
    }

    #[test]
    fn clients_get_the_scaled_nominal_size() {
        let loader = CursorLoader {
            theme: None,
            size: 24,
            colors: CursorColors::DEFAULT,
            cache: HashMap::new(),
            buffers: HashMap::new(),
        };
        assert_eq!(
            [
                loader.client_size(1.0),
                loader.client_size(2.5),
                loader.client_size(4.0)
            ],
            [24, 60, 96]
        );
    }
}
