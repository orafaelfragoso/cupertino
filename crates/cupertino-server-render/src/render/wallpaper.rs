//! Image wallpapers: a PNG or JPEG scaled to cover an output (the whole output is filled, the overflow of the longer
//! side cropped evenly), one per output and appearance. What cannot be read falls back to the next candidate: the
//! user's image, the asset pack's, the procedural wallpaper.
//!
//! The frame loop never touches the file system and never waits: it asks one worker thread for the image of an
//! output (newest request per output wins, older answers are dropped) and draws what it has until the answer is in.
//! The worker opens and decodes a file once and keeps the decoded image, so another output size only scales it;
//! it looks at the file again every few seconds and decodes again only when the file changed.

use std::collections::{HashMap, HashSet};
use std::io::BufReader;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::SystemTime;

use image::imageops::FilterType;
use image::{ImageFormat, ImageReader, Limits, RgbaImage};
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::utils::Transform;
use tiny_skia::Pixmap;

use cupertino_assets::Assets;
use cupertino_glass::cpu::apply_color_ops;
use cupertino_glass::ops::Op;
use cupertino_theme::Appearance;

use super::image::image_element;
use super::CupertinoElement;
use crate::config::WallpaperSettings;

/// Larger files are not read.
const MAX_FILE_BYTES: u64 = 256 << 20;
/// Larger images are not decoded, per side (8192 covers an 8K output) and in memory.
const MAX_SIDE: u32 = 8_192;
const MAX_ALLOC: u64 = 512 << 20;
/// Seconds between looks at the file of a shown image, to notice that it was replaced.
const REVALIDATE_SECONDS: f64 = 5.0;
/// Seconds after which an output nobody asks about is forgotten.
const FORGET_SECONDS: f64 = 60.0;
/// Failed attempts the worker remembers.
const MAX_FAILED: usize = 64;

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

/// The image in `file` decoded to RGBA.
pub fn decode_file(file: std::fs::File) -> Result<RgbaImage, String> {
    let mut reader = ImageReader::new(BufReader::new(file))
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    if !matches!(reader.format(), Some(ImageFormat::Png | ImageFormat::Jpeg)) {
        return Err("only PNG and JPEG images are supported".into());
    }
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(MAX_ALLOC);
    reader.limits(limits);
    Ok(reader.decode().map_err(|e| e.to_string())?.into_rgba8())
}

/// `source` scaled to cover `dst` pixels, premultiplied, and its average colour.
pub fn scale_cover(source: &RgbaImage, dst: (u32, u32)) -> Result<(Pixmap, [f32; 3]), String> {
    let (x, y, w, h) = cover_crop(source.dimensions(), dst);
    let covered = image::GenericImageView::view(source, x, y, w, h);
    let scaled = if (w, h) == dst {
        covered.to_image()
    } else {
        image::imageops::resize(&*covered, dst.0, dst.1, FilterType::CatmullRom)
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

/// The relative luminance of an sRGB channel value from 0 to 1.
fn linear(c: f64) -> f64 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// CIE L* of relative luminance `y`, in tenths (0 black to 1000 white).
fn lightness(y: f64) -> u16 {
    (l_star(y) * 10.0).round().clamp(0.0, 1000.0) as u16
}

/// CIE L* of relative luminance `y`, from 0 to 100.
fn l_star(y: f64) -> f64 {
    if y > 216.0 / 24_389.0 {
        116.0 * y.cbrt() - 16.0
    } else {
        y * 24_389.0 / 27.0
    }
}

/// The lightness of an sRGB colour (channels from 0 to 1), as [`BarBands::lightness`] gives it for a strip of it.
pub fn colour_lightness(rgb: [f32; 3]) -> u16 {
    lightness(luminance(rgb))
}

/// How many bands of equal width across the output the strip under the menu bar is measured in ([`strip_bands`]): a
/// band of a 2560-pixel output is 20 pixels wide, so an item of the bar's status area covers one to a few.
pub const BAR_BANDS: usize = 128;

/// The top of a channel of [`BarBands`]: full red, green or blue.
pub const BAND_CHANNEL_MAX: u16 = u16::MAX;

/// The mean encoded sRGB colour of the wallpaper under the menu bar in each of the bands of equal width across the
/// output, left to right, each channel from 0 to [`BAND_CHANNEL_MAX`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BarBands(pub Vec<[u16; 3]>);

impl BarBands {
    /// [`BAR_BANDS`] bands of one sRGB colour (channels from 0 to 1): a solid colour or the procedural wallpaper.
    pub fn even(rgb: [f32; 3]) -> Self {
        Self(vec![rgb.map(|c| channel(f64::from(c))); BAR_BANDS])
    }

    /// The mean encoded sRGB colour of the whole strip, channels from 0 to 1; black when there are no bands.
    pub fn mean(&self) -> [f64; 3] {
        let n = self.0.len().max(1) as f64;
        [0, 1, 2].map(|c| {
            self.0.iter().map(|b| f64::from(b[c])).sum::<f64>() / n / f64::from(BAND_CHANNEL_MAX)
        })
    }

    /// The relative luminance of the strip's mean colour: the mean of the encoded colour, made linear, weighed as
    /// Rec. 709 weighs red, green and blue.
    pub fn luminance(&self) -> f64 {
        linear_luminance(self.mean())
    }

    /// CIE L* of [`BarBands::luminance`] in tenths (0 black to 1000 white), what a bar older than the bands is told.
    pub fn lightness(&self) -> u16 {
        lightness(self.luminance())
    }
}

/// An sRGB channel from 0 to 1 as a channel of [`BarBands`].
fn channel(c: f64) -> u16 {
    (c.clamp(0.0, 1.0) * f64::from(BAND_CHANNEL_MAX)).round() as u16
}

/// About 16 000 pixels of the top rows of an image, where the menu bar sits, with where each one lies across it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Strip {
    pub samples: Vec<[u8; 4]>,
    /// The image's width in pixels, and the pixels between one sample and the next, row after row.
    width: usize,
    step: usize,
}

/// The [`Strip`] of the top `rows` rows of `pixmap`, whatever the size. The pixels are premultiplied, so a transparent
/// part counts as black; what shows through it is the output's clear colour (0.05, 0.05, 0.07 on every backend), so it
/// reads a little darker than it looks, and only where the image is transparent.
pub fn strip_of(pixmap: &Pixmap, rows: u32) -> Strip {
    let rows = rows.clamp(1, pixmap.height().max(1)) as usize;
    let width = pixmap.width() as usize;
    let (pixels, _) = pixmap.data().as_chunks::<4>();
    let strip = &pixels[..(rows * width).min(pixels.len())];
    let step = (strip.len() / 16_384).max(1);
    Strip {
        samples: strip.iter().step_by(step).copied().collect(),
        width,
        step,
    }
}

/// The colour under the menu bar of `strip`, seen through the colour operations `ops`, in [`BAR_BANDS`] bands of equal
/// width from the left edge to the right: the mean of the encoded sRGB of the samples in each band. A band no sample
/// falls in takes the mean of the whole strip.
pub fn strip_bands(strip: &Strip, ops: &[Op]) -> BarBands {
    let mut bands = [([0.0_f64; 3], 0_u32); BAR_BANDS];
    let mut all = ([0.0_f64; 3], 0_u32);
    for (i, px) in strip.samples.iter().enumerate() {
        let x = (i * strip.step) % strip.width.max(1);
        let rgb = apply_color_ops([0, 1, 2].map(|c| f32::from(px[c]) / 255.0), ops);
        let band = &mut bands[x * BAR_BANDS / strip.width.max(1)];
        for (c, channel) in rgb.iter().enumerate() {
            let v = f64::from(channel.clamp(0.0, 1.0));
            band.0[c] += v;
            all.0[c] += v;
        }
        band.1 += 1;
        all.1 += 1;
    }
    let mean = |(sum, n): ([f64; 3], u32)| sum.map(|s| s / f64::from(n.max(1)));
    let whole = mean(all);
    BarBands(
        bands
            .iter()
            .map(|&band| {
                let rgb = if band.1 == 0 { whole } else { mean(band) };
                rgb.map(channel)
            })
            .collect(),
    )
}

/// The relative luminance of an sRGB colour (channels from 0 to 1).
fn luminance(rgb: [f32; 3]) -> f64 {
    linear_luminance(rgb.map(f64::from))
}

/// The relative luminance of an encoded sRGB colour (channels from 0 to 1): each channel made linear, weighed as Rec. 709
/// weighs red, green and blue.
fn linear_luminance(rgb: [f64; 3]) -> f64 {
    let [r, g, b] = rgb.map(|c| linear(c.clamp(0.0, 1.0)));
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

/// What identifies the contents of a file at a time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct Stamp {
    len: u64,
    modified: Option<SystemTime>,
}

/// The file at `path`, opened without blocking (a pipe or device put there would not hang the worker), checked to be a
/// regular file through the descriptor itself, then switched to blocking reads for the decoder.
fn open_checked(path: &Path) -> Result<(std::fs::File, Stamp), String> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(rustix::fs::OFlags::NONBLOCK.bits() as i32)
        .open(path)
        .map_err(|e| e.to_string())?;
    let meta = file.metadata().map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err("not a regular file".into());
    }
    if meta.len() > MAX_FILE_BYTES {
        return Err(format!("{} bytes is too large", meta.len()));
    }
    let flags = rustix::fs::fcntl_getfl(&file).map_err(|e| e.to_string())?;
    rustix::fs::fcntl_setfl(&file, flags - rustix::fs::OFlags::NONBLOCK)
        .map_err(|e| e.to_string())?;
    let stamp = Stamp {
        len: meta.len(),
        modified: meta.modified().ok(),
    };
    Ok((file, stamp))
}

/// Reads and decodes a file; the worker's, replaceable in tests.
pub type Decoder = Arc<dyn Fn(std::fs::File) -> Result<RgbaImage, String> + Send + Sync>;

struct Request {
    output: u64,
    ticket: u64,
    /// Files to try, best first.
    candidates: Vec<PathBuf>,
    dst: (u32, u32),
    /// Rows of `dst` under the menu bar, measured for its backdrop.
    strip_rows: u32,
    /// The file and state the output shows, for an answer of "unchanged".
    known: Option<(PathBuf, Stamp)>,
    /// The colour operations of the menu bar's material, which the strip is measured through.
    bar_ops: Vec<Op>,
}

enum Outcome {
    /// The file shown is the same.
    Unchanged,
    Loaded {
        pixmap: Pixmap,
        tint: [f32; 3],
        /// The [`Strip`] of the rows under the menu bar.
        strip: Strip,
        /// [`strip_bands`] of `strip` through the request's `bar_ops`.
        bar_bands: BarBands,
        bar_ops: Vec<Op>,
        path: PathBuf,
        stamp: Stamp,
    },
    /// No candidate could be used.
    Failed,
}

struct Reply {
    output: u64,
    ticket: u64,
    outcome: Outcome,
}

/// What the main thread tells the worker.
enum Message {
    Request(Request),
    /// The output is gone or shows no image any more.
    Forget(u64),
}

/// What the worker keeps between requests: the decoded image of the last file, bounded by the decoder's limits, for as
/// long as an output wants that file.
#[derive(Default)]
struct WorkerState {
    source: Option<(PathBuf, Stamp, Arc<RgbaImage>)>,
    failed: HashSet<(PathBuf, Stamp)>,
    logged: HashSet<(PathBuf, String)>,
    /// The files each output last asked for.
    wanted: HashMap<u64, Vec<PathBuf>>,
    /// Bytes of the decoded image held, for the diagnostics and the tests.
    cached: Arc<AtomicUsize>,
}

impl WorkerState {
    /// Let go of the decoded image when no output asks for its file any more.
    fn prune(&mut self) {
        if self
            .source
            .as_ref()
            .is_some_and(|(path, _, _)| !self.wanted.values().any(|files| files.contains(path)))
        {
            self.source = None;
        }
        self.cached.store(
            self.source
                .as_ref()
                .map_or(0, |(_, _, image)| image.as_raw().len()),
            Ordering::SeqCst,
        );
    }

    fn note(&mut self, path: &Path, what: &str) {
        if self.logged.len() >= MAX_FAILED {
            self.logged.clear();
        }
        if self.logged.insert((path.to_path_buf(), what.to_string())) {
            tracing::warn!(path = %path.display(), error = what, "cannot use the wallpaper image: the next one stands in");
        }
    }

    fn process(&mut self, request: &Request, decode: &Decoder, decodes: &AtomicUsize) -> Outcome {
        for path in &request.candidates {
            let (file, stamp) = match open_checked(path) {
                Ok(opened) => opened,
                Err(e) => {
                    self.note(path, &e);
                    continue;
                }
            };
            if request
                .known
                .as_ref()
                .is_some_and(|(known, at)| known == path && *at == stamp)
            {
                return Outcome::Unchanged;
            }
            if self.failed.contains(&(path.clone(), stamp)) {
                continue;
            }
            let source = match &self.source {
                Some((cached, at, image)) if cached == path && *at == stamp => Arc::clone(image),
                _ => {
                    decodes.fetch_add(1, Ordering::SeqCst);
                    match decode(file) {
                        Ok(image) => {
                            let image = Arc::new(image);
                            self.source = Some((path.clone(), stamp, Arc::clone(&image)));
                            image
                        }
                        Err(e) => {
                            if self.failed.len() >= MAX_FAILED {
                                self.failed.clear();
                            }
                            self.failed.insert((path.clone(), stamp));
                            self.note(path, &e);
                            continue;
                        }
                    }
                }
            };
            match scale_cover(&source, request.dst) {
                Ok((pixmap, tint)) => {
                    let strip = strip_of(&pixmap, request.strip_rows);
                    return Outcome::Loaded {
                        bar_bands: strip_bands(&strip, &request.bar_ops),
                        bar_ops: request.bar_ops.clone(),
                        strip,
                        pixmap,
                        tint,
                        path: path.clone(),
                        stamp,
                    };
                }
                Err(e) => self.note(path, &e),
            }
        }
        Outcome::Failed
    }
}

/// The worker thread and its queues.
struct Loader {
    requests: Sender<Message>,
    replies: Receiver<Reply>,
    decodes: Arc<AtomicUsize>,
    cached: Arc<AtomicUsize>,
}

impl Loader {
    fn start(decode: Decoder) -> std::io::Result<Self> {
        let (requests, request_rx) = std::sync::mpsc::channel::<Message>();
        let (reply_tx, replies) = std::sync::mpsc::channel::<Reply>();
        let decodes = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&decodes);
        let mut state = WorkerState::default();
        let cached = Arc::clone(&state.cached);
        std::thread::Builder::new()
            .name("cupertino-wallpaper".into())
            .spawn(move || {
                while let Ok(first) = request_rx.recv() {
                    // what piled up while the last request ran: only the newest of each output is of interest
                    let mut newest: Vec<Request> = Vec::new();
                    let mut take = |message: Message, state: &mut WorkerState| match message {
                        Message::Request(request) => {
                            state
                                .wanted
                                .insert(request.output, request.candidates.clone());
                            match newest.iter_mut().find(|r| r.output == request.output) {
                                Some(slot) => *slot = request,
                                None => newest.push(request),
                            }
                        }
                        Message::Forget(output) => {
                            state.wanted.remove(&output);
                            newest.retain(|r| r.output != output);
                        }
                    };
                    take(first, &mut state);
                    while let Ok(next) = request_rx.try_recv() {
                        take(next, &mut state);
                    }
                    state.prune();
                    for request in newest {
                        let outcome =
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                state.process(&request, &decode, &counter)
                            }))
                            .unwrap_or(Outcome::Failed);
                        state.prune();
                        let reply = Reply {
                            output: request.output,
                            ticket: request.ticket,
                            outcome,
                        };
                        if reply_tx.send(reply).is_err() {
                            return;
                        }
                    }
                }
            })?;
        Ok(Self {
            requests,
            replies,
            decodes,
            cached,
        })
    }
}

/// What the main thread knows of one output's wallpaper.
#[derive(Default)]
struct OutputState {
    /// The candidates and size the last request was for.
    key: u64,
    /// The newest request; an answer with another ticket is out of date.
    ticket: u64,
    in_flight: bool,
    checked_at: f64,
    /// When the output was last asked about; a state nobody asks about any more (an unplugged output) is dropped.
    seen_at: f64,
    /// The file and state the shown image was made from.
    loaded: Option<(PathBuf, Stamp)>,
    shown: Option<Shown>,
}

struct Shown {
    buffer: MemoryRenderBuffer,
    px: (i32, i32),
    tint: [f32; 3],
    /// The [`Strip`] under the menu bar, measured again when the bar's colour operations change.
    strip: Strip,
}

/// The image wallpapers of all outputs.
pub struct Wallpapers {
    /// The asset pack's default pictures, light and dark.
    pack: [Option<PathBuf>; 2],
    decoder: Decoder,
    loader: Option<Loader>,
    outputs: HashMap<u64, OutputState>,
    next_ticket: u64,
    /// Average colour of the image drawn last, for the materials' tint; `None` while the procedural wallpaper shows.
    pub tint: Option<[f32; 3]>,
    /// The colour under the menu bar of each output whose new image has just been put up, until taken.
    measured: Vec<(u64, BarBands)>,
    /// The colour last handed out for each output, so the procedural wallpaper's is handed out once per change.
    sent: HashMap<u64, BarBands>,
    /// The colour operations of the menu bar's material, which the colour under it is measured through.
    bar_ops: Vec<Op>,
    /// The appearance and Reduce Transparency the operations are those of.
    bar_material: Option<(Appearance, bool)>,
    /// When each output was last asked about; what is kept for one nobody asks about any more is dropped.
    seen: HashMap<u64, f64>,
}

impl Wallpapers {
    pub fn new(assets: &Assets) -> Self {
        Self::with_decoder(
            [assets.wallpaper_path(false), assets.wallpaper_path(true)],
            Arc::new(decode_file),
        )
    }

    fn with_decoder(pack: [Option<PathBuf>; 2], decoder: Decoder) -> Self {
        Self {
            pack,
            decoder,
            loader: None,
            outputs: HashMap::new(),
            next_ticket: 0,
            tint: None,
            measured: Vec::new(),
            sent: HashMap::new(),
            bar_ops: Vec::new(),
            bar_material: None,
            seen: HashMap::new(),
        }
    }

    /// The menu bar's material is that of `material` (the appearance, and Reduce Transparency), whose colour operations
    /// `ops` gives; it is asked only when `material` changes.
    pub fn set_bar_material(
        &mut self,
        material: (Appearance, bool),
        ops: impl FnOnce() -> Vec<Op>,
    ) {
        if self.bar_material != Some(material) {
            self.bar_material = Some(material);
            self.set_bar_ops(&ops());
        }
    }

    /// The menu bar's material now has the colour operations `ops` (another appearance or material option): every
    /// image shown is measured through them again, and a colour that changed is handed out. An output with the
    /// procedural wallpaper is measured through them at its next frame; each of those changes asks every output for one.
    fn set_bar_ops(&mut self, ops: &[Op]) {
        if self.bar_ops == ops {
            return;
        }
        self.bar_ops = ops.to_vec();
        for (output, state) in &self.outputs {
            let Some(shown) = &state.shown else { continue };
            let bands = strip_bands(&shown.strip, ops);
            if self.sent.get(output) != Some(&bands) {
                self.sent.insert(*output, bands.clone());
                self.measured.push((*output, bands));
            }
        }
    }

    /// An image is being read: frames are wanted to pick it up.
    pub fn pending(&self) -> bool {
        self.outputs.values().any(|o| o.in_flight)
    }

    /// The images to try for an appearance, best first: the user's, then the pack's.
    fn candidate_paths<'a>(
        &'a self,
        config: &'a WallpaperSettings,
        dark: bool,
    ) -> impl Iterator<Item = &'a Path> + Clone + 'a {
        config
            .for_appearance(dark)
            .into_iter()
            .chain(self.pack[usize::from(dark)].as_deref())
    }

    /// [`Self::candidate_paths`], owned for a request to the worker.
    fn candidates(&self, config: &WallpaperSettings, dark: bool) -> Vec<PathBuf> {
        self.candidate_paths(config, dark)
            .map(Path::to_path_buf)
            .collect()
    }

    /// The output shows no image any more (or is gone): drop what is kept for it, here and in the worker.
    fn forget(&mut self, output: u64) {
        if self.outputs.remove(&output).is_some() {
            if let Some(loader) = &self.loader {
                // a stopped worker has nothing left to forget
                let _ = loader.requests.send(Message::Forget(output));
            }
        }
    }

    /// Take in the answers the worker has, and put a new image up at once, whichever output's frame takes it in: an
    /// output that does not draw now (off, or locked) has it ready, and nothing waits on it.
    fn collect(&mut self) {
        let Some(loader) = &self.loader else { return };
        while let Ok(reply) = loader.replies.try_recv() {
            let Some(state) = self.outputs.get_mut(&reply.output) else {
                continue;
            };
            if reply.ticket != state.ticket {
                continue;
            }
            state.in_flight = false;
            match reply.outcome {
                Outcome::Unchanged => {}
                Outcome::Loaded {
                    pixmap,
                    tint,
                    strip,
                    bar_bands,
                    bar_ops,
                    path,
                    stamp,
                } => {
                    // the operations changed while the image was read
                    let bar_bands = if bar_ops == self.bar_ops {
                        bar_bands
                    } else {
                        strip_bands(&strip, &self.bar_ops)
                    };
                    let size = (pixmap.width() as i32, pixmap.height() as i32);
                    let buffer = MemoryRenderBuffer::from_slice(
                        pixmap.data(),
                        Fourcc::Abgr8888,
                        size,
                        1,
                        Transform::Normal,
                        None,
                    );
                    state.shown = Some(Shown {
                        buffer,
                        px: size,
                        tint,
                        strip,
                    });
                    state.loaded = Some((path, stamp));
                    // every image put up is handed out, so its output draws it at once even at the same colour
                    self.measured.push((reply.output, bar_bands.clone()));
                    self.sent.insert(reply.output, bar_bands);
                }
                Outcome::Failed => {
                    state.loaded = None;
                    state.shown = None;
                }
            }
        }
    }

    fn request(
        &mut self,
        output: u64,
        candidates: Vec<PathBuf>,
        dst: (u32, u32),
        strip_rows: u32,
        now: f64,
        known: Option<(PathBuf, Stamp)>,
    ) {
        if self.loader.is_none() {
            match Loader::start(Arc::clone(&self.decoder)) {
                Ok(loader) => self.loader = Some(loader),
                Err(e) => {
                    tracing::warn!(error = %e, "cannot start reading the wallpaper image");
                    return;
                }
            }
        }
        self.next_ticket += 1;
        let ticket = self.next_ticket;
        let state = self.outputs.entry(output).or_default();
        state.ticket = ticket;
        state.in_flight = true;
        state.checked_at = now;
        let request = Request {
            output,
            ticket,
            candidates,
            dst,
            strip_rows,
            known,
            bar_ops: self.bar_ops.clone(),
        };
        if self
            .loader
            .as_ref()
            .is_some_and(|l| l.requests.send(Message::Request(request)).is_err())
        {
            tracing::warn!("the wallpaper worker has stopped");
            self.loader = None;
            if let Some(state) = self.outputs.get_mut(&output) {
                state.in_flight = false;
            }
        }
    }

    /// [`Self::update`] for the candidates whose [`key_of`] is `key`, made by `candidates` only when a request is
    /// sent: a frame that changes nothing copies no path.
    fn update_keyed(
        &mut self,
        output: u64,
        key: u64,
        candidates: impl FnOnce(&Self) -> Vec<PathBuf>,
        dst: (u32, u32),
        strip_rows: u32,
        now: f64,
    ) {
        self.collect();
        let idle: Vec<u64> = self
            .outputs
            .iter()
            .filter(|(_, s)| !s.in_flight && now - s.seen_at >= FORGET_SECONDS)
            .map(|(output, _)| *output)
            .collect();
        for output in idle {
            self.forget(output);
        }
        let state = self.outputs.entry(output).or_default();
        state.seen_at = now;
        if state.key != key || state.ticket == 0 {
            state.key = key;
            state.loaded = None;
            let candidates = candidates(self);
            self.request(output, candidates, dst, strip_rows, now, None);
        } else if !state.in_flight && now - state.checked_at >= REVALIDATE_SECONDS {
            let known = state.loaded.clone();
            let candidates = candidates(self);
            self.request(output, candidates, dst, strip_rows, now, known);
        }
    }

    /// The wallpaper element of an output of `px` device pixels at `scale`, or `None` for the procedural wallpaper.
    /// Until the image is in, the output keeps what it showed. `strip_rows` are the rows under the menu bar.
    #[allow(clippy::too_many_arguments)]
    pub fn element(
        &mut self,
        renderer: &mut GlesRenderer,
        output: u64,
        config: &WallpaperSettings,
        dark: bool,
        px: (i32, i32),
        strip_rows: u32,
        scale: f64,
        now: f64,
    ) -> Option<CupertinoElement> {
        let (buffer, px) = self.shown(output, config, dark, px, strip_rows, now)?;
        image_element(renderer, &buffer, px, 0.0, 0.0, scale, 1.0)
    }

    /// The pixels the wallpaper of an output of `px` device pixels shows, or `None` for the procedural wallpaper.
    /// Until the image is in, the output keeps what it showed. The colour of its top `strip_rows` rows, under the
    /// menu bar, is measured through the colour operations given to [`Wallpapers::set_bar_material`] once per image and
    /// once per change of those, and handed out by [`Wallpapers::take_bar_bands`].
    pub fn shown(
        &mut self,
        output: u64,
        config: &WallpaperSettings,
        dark: bool,
        px: (i32, i32),
        strip_rows: u32,
        now: f64,
    ) -> Option<(MemoryRenderBuffer, (i32, i32))> {
        self.seen.insert(output, now);
        self.seen.retain(|output, at| {
            let asked = now - *at < FORGET_SECONDS;
            if !asked {
                self.sent.remove(output);
            }
            asked
        });
        let dst = (u32::try_from(px.0).ok()?, u32::try_from(px.1).ok()?);
        if dst.0 == 0 || dst.1 == 0 {
            return None;
        }
        let paths = self.candidate_paths(config, dark);
        let (none, key) = (
            paths.clone().next().is_none(),
            key_of(paths, dst, strip_rows),
        );
        if none {
            self.forget(output);
            self.tint = None;
            self.procedural(output, super::core::wallpaper_tint(dark));
            return None;
        }
        self.update_keyed(
            output,
            key,
            |w| w.candidates(config, dark),
            dst,
            strip_rows,
            now,
        );
        let state = self.outputs.get(&output)?;
        // no candidate could be used and none is being read: the procedural wallpaper stays
        if !state.in_flight && state.shown.is_none() {
            self.tint = None;
            self.procedural(output, super::core::wallpaper_tint(dark));
            return None;
        }
        match state.shown.as_ref().filter(|s| s.px == px) {
            Some(shown) => {
                self.tint = Some(shown.tint);
                Some((shown.buffer.clone(), shown.px))
            }
            None => {
                self.tint = None;
                None
            }
        }
    }

    /// A solid colour shows on `output`: nothing is read, the desktop's tint and the colour under its bar are that
    /// colour's.
    pub fn solid(&mut self, output: u64, colour: [f32; 3], now: f64) {
        self.seen.insert(output, now);
        self.forget(output);
        self.tint = Some(colour);
        self.procedural(output, colour);
    }

    /// The procedural wallpaper shows on `output`: the colour under its bar is the wallpaper's average colour
    /// ([`super::core::wallpaper_tint`]) through the bar's colour operations, handed out when it differs from
    /// the last one.
    fn procedural(&mut self, output: u64, tint: [f32; 3]) {
        let bands = BarBands::even(apply_color_ops(tint, &self.bar_ops));
        if self.sent.get(&output) != Some(&bands) {
            self.sent.insert(output, bands.clone());
            self.measured.push((output, bands));
        }
    }

    /// The colour under the menu bar of each output whose new image was put up since the last call.
    pub fn take_bar_bands(&mut self) -> Vec<(u64, BarBands)> {
        std::mem::take(&mut self.measured)
    }

    /// Bytes of the decoded image the worker holds: none once no output wants its file.
    pub fn cached_source_bytes(&self) -> usize {
        self.loader
            .as_ref()
            .map_or(0, |l| l.cached.load(Ordering::SeqCst))
    }

    /// Images the worker has decoded so far (one per file and change of the file).
    pub fn decodes(&self) -> usize {
        self.loader
            .as_ref()
            .map_or(0, |l| l.decodes.load(Ordering::SeqCst))
    }
}

/// What a request is for: the files and the size.
fn key_of<'a>(candidates: impl Iterator<Item = &'a Path>, dst: (u32, u32), strip_rows: u32) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let mut count = 0usize;
    for c in candidates {
        c.hash(&mut h);
        count += 1;
    }
    count.hash(&mut h);
    dst.hash(&mut h);
    strip_rows.hash(&mut h);
    h.finish()
}
