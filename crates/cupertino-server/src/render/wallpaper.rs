//! Image wallpapers: a PNG or JPEG scaled to cover an output (the whole output is filled, the overflow of the longer
//! side cropped evenly), one per output and appearance. The image is decoded and scaled once per file, output size
//! and appearance, then drawn as a memory buffer; what cannot be read falls back to the next candidate: the user's
//! image, the asset pack's, the procedural wallpaper.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use image::imageops::FilterType;
use image::{ImageFormat, ImageReader, Limits};
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::backend::renderer::gles::GlesRenderer;
use tiny_skia::Pixmap;

use cupertino_assets::Assets;

use super::image::{image_element, ImageCache};
use super::CupertinoElement;
use crate::config::WallpaperSettings;

/// Larger files are not read.
const MAX_FILE_BYTES: u64 = 256 << 20;
/// Larger images are not decoded, per side and in memory.
const MAX_SIDE: u32 = 16_384;
const MAX_ALLOC: u64 = 1 << 30;
/// Distinct images kept for their average colour.
const MAX_TINTS: usize = 16;

/// The area of an image of `src` pixels that covers `dst` pixels at the same aspect ratio, centred: `(x, y, width,
/// height)`.
pub fn cover_crop(src: (u32, u32), dst: (u32, u32)) -> (u32, u32, u32, u32) {
    let ((sw, sh), (dw, dh)) = (src, dst);
    if u64::from(sw) * u64::from(dh) > u64::from(sh) * u64::from(dw) {
        // wider than the output: the sides are cropped
        let w = u32::try_from(u64::from(sh) * u64::from(dw) / u64::from(dh))
            .unwrap_or(sw)
            .clamp(1, sw);
        ((sw - w) / 2, 0, w, sh)
    } else {
        let h = u32::try_from(u64::from(sw) * u64::from(dh) / u64::from(dw))
            .unwrap_or(sh)
            .clamp(1, sh);
        (0, (sh - h) / 2, sw, h)
    }
}

/// The image at `path` scaled to cover `dst` pixels, premultiplied, and its average colour.
pub fn load_cover(path: &Path, dst: (u32, u32)) -> Result<(Pixmap, [f32; 3]), String> {
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err("not a regular file".into());
    }
    if meta.len() > MAX_FILE_BYTES {
        return Err(format!("{} bytes is too large", meta.len()));
    }
    let mut reader = ImageReader::open(path)
        .and_then(ImageReader::with_guessed_format)
        .map_err(|e| e.to_string())?;
    if !matches!(reader.format(), Some(ImageFormat::Png | ImageFormat::Jpeg)) {
        return Err("only PNG and JPEG images are supported".into());
    }
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(MAX_ALLOC);
    reader.limits(limits);
    let source = reader.decode().map_err(|e| e.to_string())?.into_rgba8();
    let (x, y, w, h) = cover_crop(source.dimensions(), dst);
    let covered = image::imageops::crop_imm(&source, x, y, w, h).to_image();
    let scaled = if (w, h) == dst {
        covered
    } else {
        image::imageops::resize(&covered, dst.0, dst.1, FilterType::CatmullRom)
    };
    let mut pixmap = Pixmap::new(dst.0, dst.1).ok_or("the output has no area")?;
    let mut sum = [0u64; 3];
    let mut sampled = 0u64;
    // about 65 000 pixels give the average of any size
    let step = (u64::from(dst.0) * u64::from(dst.1) / 65_536).max(1) as usize;
    let (pixels, _) = pixmap.data_mut().as_chunks_mut::<4>();
    for (i, (out, px)) in pixels.iter_mut().zip(scaled.pixels()).enumerate() {
        let a = u16::from(px[3]);
        for c in 0..3 {
            out[c] = ((u16::from(px[c]) * a + 127) / 255) as u8;
        }
        out[3] = px[3];
        if i % step == 0 {
            for c in 0..3 {
                sum[c] += u64::from(px[c]);
            }
            sampled += 1;
        }
    }
    let n = sampled.max(1) as f32 * 255.0;
    Ok((
        pixmap,
        [sum[0] as f32 / n, sum[1] as f32 / n, sum[2] as f32 / n],
    ))
}

/// The result of reading one image.
type Loaded = Result<(Pixmap, [f32; 3]), String>;

/// What the wallpaper needs besides the buffers of the image cache.
pub struct Wallpapers {
    /// The asset pack's default pictures, light and dark.
    pack: [Option<PathBuf>; 2],
    /// Images being read on threads of their own, by signature: reading and scaling a large photograph takes a
    /// noticeable part of a second, which the frame loop must not wait for.
    jobs: HashMap<u64, std::sync::mpsc::Receiver<Loaded>>,
    /// Images read and not yet handed to the image cache.
    done: HashMap<u64, (Pixmap, [f32; 3])>,
    /// Images that could not be read, by the signature of the attempt: not tried again until the file or the output
    /// changes.
    failed: HashSet<u64>,
    /// Average colour of the loaded images by signature.
    tints: HashMap<u64, [f32; 3]>,
    /// What each output shows, kept so that it stays up while the next image loads.
    shown: HashMap<u64, Shown>,
    /// Files whose absence was logged.
    missing: HashSet<PathBuf>,
    /// Average colour of the image drawn last, for the materials' tint; `None` while the procedural wallpaper shows.
    pub tint: Option<[f32; 3]>,
}

struct Shown {
    buffer: MemoryRenderBuffer,
    px: (i32, i32),
    tint: Option<[f32; 3]>,
}

impl Wallpapers {
    pub fn new(assets: &Assets) -> Self {
        Self {
            pack: [assets.wallpaper_path(false), assets.wallpaper_path(true)],
            jobs: HashMap::new(),
            done: HashMap::new(),
            failed: HashSet::new(),
            tints: HashMap::new(),
            shown: HashMap::new(),
            missing: HashSet::new(),
            tint: None,
        }
    }

    /// An image is being read: frames are wanted to pick it up.
    pub fn pending(&self) -> bool {
        !self.jobs.is_empty()
    }

    /// The images to try for an appearance, best first: the user's, then the pack's.
    fn candidates(&self, config: &WallpaperSettings, dark: bool) -> Vec<PathBuf> {
        config
            .for_appearance(dark)
            .map(Path::to_path_buf)
            .into_iter()
            .chain(self.pack[usize::from(dark)].clone())
            .collect()
    }

    /// Take in the images the reading threads have finished.
    fn collect(&mut self) {
        use std::sync::mpsc::TryRecvError;
        let mut finished = Vec::new();
        for (sig, rx) in &self.jobs {
            match rx.try_recv() {
                Ok(result) => finished.push((*sig, result)),
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    finished.push((*sig, Err("the reading thread ended".into())));
                }
            }
        }
        for (sig, result) in finished {
            self.jobs.remove(&sig);
            match result {
                Ok(loaded) => {
                    if self.done.len() >= MAX_TINTS {
                        self.done.clear();
                    }
                    self.done.insert(sig, loaded);
                }
                Err(e) => {
                    tracing::warn!(error = %e, "cannot use the wallpaper image: the next one stands in");
                    self.failed.insert(sig);
                }
            }
        }
    }

    /// The wallpaper element of an output of `px` device pixels at `scale`, or `None` for the procedural wallpaper.
    /// An image is read on a thread; until it is in, the output keeps what it showed.
    #[allow(clippy::too_many_arguments)]
    pub fn element(
        &mut self,
        renderer: &mut GlesRenderer,
        images: &mut ImageCache,
        output: u64,
        config: &WallpaperSettings,
        dark: bool,
        px: (i32, i32),
        scale: f64,
    ) -> Option<CupertinoElement> {
        let dst = (u32::try_from(px.0).ok()?, u32::try_from(px.1).ok()?);
        if dst.0 == 0 || dst.1 == 0 {
            return None;
        }
        self.collect();
        for path in self.candidates(config, dark) {
            let Ok(meta) = std::fs::metadata(&path) else {
                if self.missing.insert(path.clone()) {
                    tracing::warn!(path = %path.display(), "the wallpaper image is not there: the next one stands in");
                }
                continue;
            };
            self.missing.remove(&path);
            let sig = signature(&path, &meta, dst);
            if self.failed.contains(&sig) {
                continue;
            }
            let mut fresh = None;
            let cut = images.get(("wallpaper", output), sig, || {
                self.done.remove(&sig).map(|(pixmap, tint)| {
                    fresh = Some(tint);
                    (pixmap, [0; 4])
                })
            });
            if let Some(tint) = fresh {
                if self.tints.len() >= MAX_TINTS {
                    self.tints.clear();
                }
                self.tints.insert(sig, tint);
            }
            if let Some((buffer, size, _)) = cut {
                let tint = self.tints.get(&sig).copied();
                self.tint = tint;
                let element = image_element(renderer, &buffer, size, 0.0, 0.0, scale, 1.0);
                self.shown.insert(
                    output,
                    Shown {
                        buffer,
                        px: size,
                        tint,
                    },
                );
                return element;
            }
            if let std::collections::hash_map::Entry::Vacant(job) = self.jobs.entry(sig) {
                let (tx, rx) = std::sync::mpsc::channel();
                let job_path = path.clone();
                let spawned = std::thread::Builder::new()
                    .name("cupertino-wallpaper".into())
                    .spawn(move || drop(tx.send(load_cover(&job_path, dst))));
                match spawned {
                    Ok(_) => {
                        job.insert(rx);
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "cannot start reading the wallpaper image");
                        self.failed.insert(sig);
                        continue;
                    }
                }
            }
            // reading: keep showing what the output showed, if it fits
            return match self.shown.get(&output).filter(|s| s.px == (px.0, px.1)) {
                Some(shown) => {
                    self.tint = shown.tint;
                    image_element(renderer, &shown.buffer, shown.px, 0.0, 0.0, scale, 1.0)
                }
                None => {
                    self.tint = None;
                    None
                }
            };
        }
        self.shown.remove(&output);
        self.tint = None;
        None
    }
}

/// Identity of one attempt: the file as it is now and the size it is scaled to.
fn signature(path: &Path, meta: &std::fs::Metadata, dst: (u32, u32)) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut h);
    meta.len().hash(&mut h);
    meta.modified().ok().hash(&mut h);
    dst.hash(&mut h);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_png(
        dir: &Path,
        name: &str,
        size: (u32, u32),
        pixel: impl Fn(u32, u32) -> [u8; 4],
    ) -> PathBuf {
        let mut img = image::RgbaImage::new(size.0, size.1);
        for (x, y, p) in img.enumerate_pixels_mut() {
            *p = image::Rgba(pixel(x, y));
        }
        let path = dir.join(name);
        img.save(&path).unwrap();
        path
    }

    #[test]
    fn the_cover_area_is_centred_and_has_the_aspect_of_the_output() {
        assert_eq!(cover_crop((400, 200), (100, 100)), (100, 0, 200, 200));
        assert_eq!(cover_crop((200, 400), (100, 50)), (0, 150, 200, 100));
        assert_eq!(cover_crop((300, 300), (100, 100)), (0, 0, 300, 300));
        assert_eq!(cover_crop((3840, 2160), (1000, 1000)), (840, 0, 2160, 2160));
        assert_eq!(cover_crop((1, 1), (7, 3)), (0, 0, 1, 1));
    }

    #[test]
    fn an_image_covers_the_output_and_its_overflow_is_cropped() {
        let dir = tempfile::tempdir().unwrap();
        // 8 x 4: the left half red, the right half blue; a 4 x 4 output shows the middle two columns of each
        let path = write_png(dir.path(), "w.png", (8, 4), |x, _| {
            if x < 4 {
                [255, 0, 0, 255]
            } else {
                [0, 0, 255, 255]
            }
        });
        let (pixmap, tint) = load_cover(&path, (4, 4)).unwrap();
        let at = |x: u32, y: u32| pixmap.pixel(x, y).unwrap();
        assert_eq!((at(0, 0).red(), at(0, 0).blue()), (255, 0));
        assert_eq!((at(1, 3).red(), at(1, 3).blue()), (255, 0));
        assert_eq!((at(2, 1).red(), at(2, 1).blue()), (0, 255));
        assert_eq!((at(3, 3).red(), at(3, 3).blue()), (0, 255));
        assert!(
            (tint[0] - 0.5).abs() < 0.01 && (tint[2] - 0.5).abs() < 0.01,
            "{tint:?}"
        );
    }

    #[test]
    fn an_image_is_scaled_to_the_output() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_png(dir.path(), "w.png", (8, 8), |_, _| [10, 200, 30, 255]);
        let (pixmap, _) = load_cover(&path, (3, 5)).unwrap();
        assert_eq!((pixmap.width(), pixmap.height()), (3, 5));
        let p = pixmap.pixel(1, 2).unwrap();
        assert_eq!(
            (p.red(), p.green(), p.blue(), p.alpha()),
            (10, 200, 30, 255)
        );
    }

    #[test]
    fn a_jpeg_is_read_too() {
        let dir = tempfile::tempdir().unwrap();
        let mut img = image::RgbImage::new(16, 16);
        for p in img.pixels_mut() {
            *p = image::Rgb([200, 40, 40]);
        }
        let path = dir.path().join("w.jpg");
        img.save(&path).unwrap();
        let (pixmap, _) = load_cover(&path, (4, 4)).unwrap();
        let p = pixmap.pixel(2, 2).unwrap();
        assert!(p.red() > 190 && p.green() < 60 && p.alpha() == 255, "{p:?}");
    }

    #[test]
    fn other_formats_and_other_things_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let bmp = dir.path().join("w.bmp");
        std::fs::write(&bmp, [b"BM".as_slice(), &[0; 62]].concat()).unwrap();
        assert!(load_cover(&bmp, (2, 2))
            .unwrap_err()
            .contains("PNG and JPEG"));
        let text = dir.path().join("w.png");
        std::fs::write(&text, b"not an image").unwrap();
        assert!(load_cover(&text, (2, 2)).is_err());
        assert!(load_cover(dir.path(), (2, 2))
            .unwrap_err()
            .contains("regular"));
        assert!(load_cover(&dir.path().join("none.png"), (2, 2)).is_err());
    }

    #[test]
    fn the_users_image_comes_before_the_packs_and_the_dark_one_falls_back_to_the_light() {
        let wallpapers = Wallpapers {
            pack: [Some("/pack/day.png".into()), Some("/pack/night.png".into())],
            jobs: HashMap::new(),
            done: HashMap::new(),
            failed: HashSet::new(),
            tints: HashMap::new(),
            shown: HashMap::new(),
            missing: HashSet::new(),
            tint: None,
        };
        let paths = |c: &WallpaperSettings, dark| {
            wallpapers
                .candidates(c, dark)
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
        };
        let none = WallpaperSettings::default();
        assert_eq!(paths(&none, false), ["/pack/day.png"]);
        assert_eq!(paths(&none, true), ["/pack/night.png"]);
        let light_only = WallpaperSettings {
            light: Some("/u/a.png".into()),
            dark: None,
        };
        assert_eq!(paths(&light_only, false), ["/u/a.png", "/pack/day.png"]);
        assert_eq!(paths(&light_only, true), ["/u/a.png", "/pack/night.png"]);
        let pair = WallpaperSettings {
            light: Some("/u/a.png".into()),
            dark: Some("/u/b.png".into()),
        };
        assert_eq!(paths(&pair, true), ["/u/b.png", "/pack/night.png"]);
        let dark_only = WallpaperSettings {
            light: None,
            dark: Some("/u/b.png".into()),
        };
        assert_eq!(paths(&dark_only, false), ["/pack/day.png"]);
    }
}
