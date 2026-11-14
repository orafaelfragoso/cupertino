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

    /// Bring the output's state up to date with what is wanted: no file system access, no waiting.
    #[cfg(test)]
    fn update(
        &mut self,
        output: u64,
        candidates: Vec<PathBuf>,
        dst: (u32, u32),
        strip_rows: u32,
        now: f64,
    ) {
        let key = key_of(candidates.iter().map(PathBuf::as_path), dst, strip_rows);
        self.update_keyed(output, key, |_| candidates, dst, strip_rows, now);
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
            self.procedural(output, crate::render::scene::wallpaper_tint(dark));
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
            self.procedural(output, crate::render::scene::wallpaper_tint(dark));
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
    /// ([`crate::render::scene::wallpaper_tint`]) through the bar's colour operations, handed out when it differs from
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::Duration;

    /// The image at `path` scaled to cover `dst`.
    fn load_cover(path: &Path, dst: (u32, u32)) -> Result<(Pixmap, [f32; 3]), String> {
        let (file, _) = open_checked(path)?;
        scale_cover(&decode_file(file)?, dst)
    }

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

    /// A pixmap of `size` whose rows above `split` are `top` and the others `rest` (opaque sRGB greys).
    fn split_pixmap(size: (u32, u32), split: u32, top: u8, rest: u8) -> Pixmap {
        let mut pixmap = Pixmap::new(size.0, size.1).unwrap();
        let width = size.0 as usize;
        let (pixels, _) = pixmap.data_mut().as_chunks_mut::<4>();
        for (i, px) in pixels.iter_mut().enumerate() {
            let v = if ((i / width) as u32) < split {
                top
            } else {
                rest
            };
            *px = [v, v, v, 255];
        }
        pixmap
    }

    /// The bands of the top `rows` rows of `pixmap` with no material on them.
    fn raw_bands(pixmap: &Pixmap, rows: u32) -> BarBands {
        strip_bands(&strip_of(pixmap, rows), &[])
    }

    /// The colour operations of the menu bar's material in `appearance`.
    fn bar_ops(appearance: cupertino_theme::Appearance) -> Vec<Op> {
        let recipe = cupertino_theme::Theme::load()
            .unwrap()
            .recipe(
                appearance,
                "MenuBar",
                &cupertino_theme::MaterialOptions::default(),
            )
            .unwrap();
        cupertino_glass::ops::color_ops(&recipe.backdrop.unwrap().ops)
    }

    /// The bands of an even strip of sRGB grey `v`.
    fn grey_bands(v: u8) -> BarBands {
        BarBands::even([f32::from(v) / 255.0; 3])
    }

    #[test]
    fn a_strip_is_measured_through_the_bar_material() {
        let grey = strip_of(&split_pixmap((64, 48), 48, 119, 119), 24);
        let ops = bar_ops(cupertino_theme::Appearance::Light);
        let through = BarBands::even(apply_color_ops([119.0 / 255.0; 3], &ops));
        assert_eq!(strip_bands(&grey, &ops), through);
        assert_eq!(strip_bands(&grey, &[]), grey_bands(119));
        assert_ne!(through, grey_bands(119), "the material changes mid-grey");
    }

    #[test]
    fn a_change_of_the_bar_material_hands_out_the_colour_again_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_png(dir.path(), "w.png", (32, 32), |_, _| [119, 119, 119, 255]);
        let mut rig = rig();
        let ops = bar_ops(cupertino_theme::Appearance::Light);
        let through = BarBands::even(apply_color_ops([119.0 / 255.0; 3], &ops));
        rig.wallpapers.set_bar_ops(&ops);
        rig.settle(&path, (32, 32), 0.0);
        assert_eq!(rig.wallpapers.take_bar_bands(), [(1, through.clone())]);
        rig.wallpapers.set_bar_ops(&ops);
        assert_eq!(rig.wallpapers.take_bar_bands(), []);
        // Reduce Transparency: the bar is solid and has no colour operations
        rig.wallpapers.set_bar_ops(&[]);
        assert_eq!(rig.wallpapers.take_bar_bands(), [(1, grey_bands(119))]);
        rig.wallpapers.set_bar_ops(&[]);
        assert_eq!(rig.wallpapers.take_bar_bands(), []);
        rig.wallpapers.set_bar_ops(&ops);
        assert_eq!(rig.wallpapers.take_bar_bands(), [(1, through)]);
    }

    #[test]
    fn a_frame_that_changes_nothing_copies_no_wallpaper_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_png(dir.path(), "w.png", (32, 32), |_, _| [119, 119, 119, 255]);
        let mut rig = rig();
        rig.settle(&path, (32, 32), 0.0);
        let key = key_of(std::iter::once(path.as_path()), (32, 32), 1);
        let made = std::cell::Cell::new(0);
        for frame in 1..=10 {
            rig.wallpapers.update_keyed(
                1,
                key,
                |_| {
                    made.set(made.get() + 1);
                    vec![path.clone()]
                },
                (32, 32),
                1,
                f64::from(frame) / 60.0,
            );
        }
        assert_eq!(
            made.get(),
            0,
            "the same image at the same size asks for nothing"
        );
        let other = key_of(std::iter::once(path.as_path()), (64, 32), 1);
        rig.wallpapers.update_keyed(
            1,
            other,
            |_| {
                made.set(made.get() + 1);
                vec![path.clone()]
            },
            (64, 32),
            1,
            0.5,
        );
        assert_eq!(made.get(), 1, "another size asks for the image again");
    }

    #[test]
    fn an_image_read_while_the_bar_material_changes_is_measured_through_the_new_one() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_png(dir.path(), "w.png", (32, 32), |_, _| [119, 119, 119, 255]);
        let mut rig = rig();
        rig.wallpapers
            .set_bar_ops(&bar_ops(cupertino_theme::Appearance::Light));
        let hold = rig.gate.lock().unwrap();
        rig.wallpapers
            .update(1, vec![path.clone()], (32, 32), 1, 0.0);
        rig.wallpapers.set_bar_ops(&[]);
        drop(hold);
        rig.settle(&path, (32, 32), 0.0);
        assert_eq!(rig.wallpapers.take_bar_bands(), [(1, grey_bands(119))]);
    }

    #[test]
    fn the_bar_materials_operations_are_asked_for_once_per_change() {
        use cupertino_theme::Appearance;
        let mut wallpapers = Wallpapers::with_decoder([None, None], Arc::new(decode_file));
        let asked = std::cell::Cell::new(0);
        let ops = || {
            asked.set(asked.get() + 1);
            bar_ops(Appearance::Light)
        };
        for _ in 0..3 {
            wallpapers.set_bar_material((Appearance::Light, false), ops);
        }
        assert_eq!(asked.get(), 1);
        wallpapers.set_bar_material((Appearance::Light, true), Vec::new);
        wallpapers.set_bar_material((Appearance::Light, false), ops);
        assert_eq!(asked.get(), 2);
    }

    #[test]
    fn an_output_nobody_asked_about_for_a_while_is_told_its_colour_again() {
        let mut wallpapers = Wallpapers::with_decoder([None, None], Arc::new(decode_file));
        let none = WallpaperSettings::default();
        let light = BarBands::even(crate::render::scene::wallpaper_tint(false));
        wallpapers.shown(1, &none, false, (8, 8), 1, 0.0);
        assert_eq!(wallpapers.take_bar_bands(), [(1, light.clone())]);
        // output 1 is unplugged; a minute later another output draws, then output 1 comes back
        wallpapers.shown(2, &none, false, (8, 8), 1, FORGET_SECONDS);
        assert_eq!(wallpapers.take_bar_bands(), [(2, light.clone())]);
        wallpapers.shown(1, &none, false, (8, 8), 1, FORGET_SECONDS + 1.0);
        assert_eq!(wallpapers.take_bar_bands(), [(1, light)]);
        wallpapers.shown(1, &none, false, (8, 8), 1, FORGET_SECONDS + 2.0);
        assert_eq!(wallpapers.take_bar_bands(), []);
    }

    /// The strip's measure is the relative luminance of its mean encoded colour: black 0, white 1, and sRGB 151 (0.592
    /// encoded) about 0.31, under the 0.334 that splits dark text from light.
    #[test]
    fn the_luminance_under_the_bar_is_that_of_the_mean_encoded_colour() {
        let black = raw_bands(&split_pixmap((64, 48), 48, 0, 0), 24);
        let white = raw_bands(&split_pixmap((64, 48), 48, 255, 255), 24);
        assert_eq!(black.luminance(), 0.0);
        assert!((white.luminance() - 1.0).abs() < 1e-9);
        assert_eq!((black.lightness(), white.lightness()), (0, 1000));
        let grey = raw_bands(&split_pixmap((64, 48), 48, 151, 151), 24).luminance();
        assert!((grey - 0.3095).abs() < 0.001, "{grey}");
        // sRGB 119 is L* 50.0, what an older bar is told
        let mid = raw_bands(&split_pixmap((64, 48), 48, 119, 119), 24).lightness();
        assert!(mid.abs_diff(500) <= 3, "{mid}");
    }

    /// Black and white rows in equal parts average to encoded grey 0.5 before the luminance is taken: about 0.214, not
    /// the 0.5 the mean of each pixel's luminance would give.
    #[test]
    fn the_colour_is_averaged_encoded_before_it_is_made_linear() {
        let mut pixmap = Pixmap::new(64, 24).unwrap();
        let (pixels, _) = pixmap.data_mut().as_chunks_mut::<4>();
        for (i, px) in pixels.iter_mut().enumerate() {
            let v = if (i / 64) % 2 == 0 { 255 } else { 0 };
            *px = [v, v, v, 255];
        }
        let y = raw_bands(&pixmap, 24).luminance();
        assert!((y - 0.2140).abs() < 0.001, "{y}");
    }

    /// A strip whose left half is black and right half white is black in the left bands and white in the right ones,
    /// and its whole mean is encoded grey 0.5.
    #[test]
    fn a_split_strip_is_measured_band_by_band_from_left_to_right() {
        let (width, rows) = (2560_u32, 48_u32);
        let mut pixmap = Pixmap::new(width, rows).unwrap();
        let (pixels, _) = pixmap.data_mut().as_chunks_mut::<4>();
        for (i, px) in pixels.iter_mut().enumerate() {
            let v = if i % width as usize >= width as usize / 2 {
                255
            } else {
                0
            };
            *px = [v, v, v, 255];
        }
        let bands = raw_bands(&pixmap, rows);
        assert_eq!(bands.0.len(), BAR_BANDS);
        for (i, band) in bands.0.iter().enumerate() {
            let want = if i < BAR_BANDS / 2 {
                0
            } else {
                BAND_CHANNEL_MAX
            };
            assert_eq!(*band, [want; 3], "band {i}");
        }
        assert!((bands.mean()[0] - 0.5).abs() < 1e-9);
        // a gradient from black to white rises band by band
        let mut ramp = Pixmap::new(width, 8).unwrap();
        let (pixels, _) = ramp.data_mut().as_chunks_mut::<4>();
        for (i, px) in pixels.iter_mut().enumerate() {
            let v = ((i % width as usize) * 255 / (width as usize - 1)) as u8;
            *px = [v, v, v, 255];
        }
        let ramp = raw_bands(&ramp, 8);
        assert!(ramp.0.windows(2).all(|w| w[0][0] <= w[1][0]));
        assert!(ramp.0[0][0] < 1_000 && ramp.0[BAR_BANDS - 1][0] > 64_000);
    }

    #[test]
    fn a_colour_measures_as_a_strip_of_it_does() {
        for grey in [0u8, 40, 119, 200, 255] {
            let v = f32::from(grey) / 255.0;
            assert_eq!(
                BarBands::even([v; 3]),
                raw_bands(&split_pixmap((16, 16), 16, grey, grey), 8),
                "{grey}"
            );
            assert_eq!(colour_lightness([v; 3]), BarBands::even([v; 3]).lightness());
        }
        let light = BarBands::even(crate::render::scene::wallpaper_tint(false)).luminance();
        let dark = BarBands::even(crate::render::scene::wallpaper_tint(true)).luminance();
        assert!(light > 0.5 && dark < 0.1, "{light} {dark}");
    }

    #[test]
    fn the_procedural_wallpapers_colour_is_handed_out_once_per_change() {
        let mut wallpapers = Wallpapers::with_decoder([None, None], Arc::new(decode_file));
        let none = WallpaperSettings::default();
        let light = BarBands::even(crate::render::scene::wallpaper_tint(false));
        for _ in 0..3 {
            assert!(wallpapers.shown(1, &none, false, (8, 8), 1, 0.0).is_none());
        }
        assert_eq!(wallpapers.take_bar_bands(), [(1, light)]);
        wallpapers.shown(1, &none, true, (8, 8), 1, 0.0);
        wallpapers.shown(1, &none, true, (8, 8), 1, 0.0);
        let dark = BarBands::even(crate::render::scene::wallpaper_tint(true));
        assert_eq!(wallpapers.take_bar_bands(), [(1, dark)]);
    }

    #[test]
    fn only_the_rows_under_the_bar_are_measured() {
        let pixmap = split_pixmap((300, 200), 24, 10, 245);
        assert!(
            raw_bands(&pixmap, 24).luminance() < 0.01,
            "the bar's rows are dark"
        );
        assert!(
            raw_bands(&pixmap, 200).luminance() > 0.6,
            "the whole image is mostly light"
        );
        // a strip taller than the image measures the image
        assert_eq!(raw_bands(&pixmap, 900), raw_bands(&pixmap, 200));
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
        let wallpapers = Wallpapers::with_decoder(
            [Some("/pack/day.png".into()), Some("/pack/night.png".into())],
            Arc::new(decode_file),
        );
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

    /// A wallpaper pipeline whose decoder counts and can be held up.
    struct Rig {
        wallpapers: Wallpapers,
        gate: Arc<Mutex<()>>,
    }

    fn rig() -> Rig {
        let gate = Arc::new(Mutex::new(()));
        let held = Arc::clone(&gate);
        let decoder: Decoder = Arc::new(move |file| {
            drop(held.lock().unwrap());
            decode_file(file)
        });
        Rig {
            wallpapers: Wallpapers::with_decoder([None, None], decoder),
            gate,
        }
    }

    impl Rig {
        /// Ask for `path` at `dst` as the frame loop does, until nothing is in flight.
        fn settle(&mut self, path: &Path, dst: (u32, u32), now: f64) {
            let end = std::time::Instant::now() + Duration::from_secs(10);
            loop {
                self.wallpapers
                    .update(1, vec![path.to_path_buf()], dst, 1, now);
                if !self.wallpapers.pending() {
                    return;
                }
                assert!(std::time::Instant::now() < end, "the worker did not answer");
                std::thread::sleep(Duration::from_millis(2));
            }
        }

        fn shown_size(&self) -> Option<(u32, u32)> {
            self.wallpapers.outputs[&1]
                .shown
                .as_ref()
                .map(|s| (s.px.0 as u32, s.px.1 as u32))
        }
    }

    #[test]
    fn one_file_is_decoded_once_however_the_output_changes() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_png(dir.path(), "w.png", (64, 32), |_, _| [1, 2, 3, 255]);
        let mut rig = rig();
        // the decoder is held up while the size changes fifty times
        let hold = rig.gate.lock().unwrap();
        for width in 100..150 {
            rig.wallpapers
                .update(1, vec![path.clone()], (width, 80), 1, 0.0);
        }
        drop(hold);
        rig.settle(&path, (149, 80), 0.0);
        assert_eq!(
            rig.shown_size(),
            Some((149, 80)),
            "the newest size is the one that comes out"
        );
        assert_eq!(rig.wallpapers.decodes(), 1);
        // another size of the same file only scales it
        rig.wallpapers.outputs.get_mut(&1).unwrap().shown = None;
        rig.settle(&path, (320, 200), 0.0);
        assert_eq!(rig.shown_size(), Some((320, 200)));
        assert_eq!(rig.wallpapers.decodes(), 1);
    }

    #[test]
    fn the_frame_loop_does_not_wait_for_the_worker() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_png(dir.path(), "w.png", (16, 16), |_, _| [9, 9, 9, 255]);
        let mut rig = rig();
        let hold = rig.gate.lock().unwrap();
        let started = std::time::Instant::now();
        for frame in 0..100 {
            rig.wallpapers
                .update(1, vec![path.clone()], (32, 32), 1, f64::from(frame) / 60.0);
        }
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "100 frames took {:?} with the decoder held up",
            started.elapsed()
        );
        assert!(rig.wallpapers.pending());
        assert!(rig.wallpapers.outputs[&1].shown.is_none());
        drop(hold);
        rig.settle(&path, (32, 32), 2.0);
        assert!(rig.shown_size().is_some());
    }

    #[test]
    fn a_file_is_looked_at_again_every_few_seconds_and_decoded_again_only_when_it_changed() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_png(dir.path(), "w.png", (16, 16), |_, _| [200, 0, 0, 255]);
        let mut rig = rig();
        rig.settle(&path, (8, 8), 0.0);
        assert_eq!(rig.wallpapers.decodes(), 1);
        rig.wallpapers.outputs.get_mut(&1).unwrap().shown = None;
        // not due yet, then due with the file unchanged: nothing is decoded or delivered again
        rig.settle(&path, (8, 8), 1.0);
        rig.settle(&path, (8, 8), 6.0);
        assert_eq!(rig.wallpapers.decodes(), 1);
        assert!(rig.shown_size().is_none());
        // the file is replaced by a larger one
        write_png(dir.path(), "w.png", (32, 32), |_, _| [0, 200, 0, 255]);
        rig.settle(&path, (8, 8), 12.0);
        assert_eq!(rig.wallpapers.decodes(), 2);
        let tint = rig.wallpapers.outputs[&1].shown.as_ref().unwrap().tint;
        assert!((tint[1] - 200.0 / 255.0).abs() < 0.01, "{tint:?}");
    }

    #[test]
    fn a_broken_file_is_decoded_once_and_a_good_candidate_after_it_is_used() {
        let dir = tempfile::tempdir().unwrap();
        let broken = dir.path().join("broken.png");
        std::fs::write(&broken, b"not an image").unwrap();
        let good = write_png(dir.path(), "good.png", (8, 8), |_, _| [5, 6, 7, 255]);
        let mut rig = rig();
        let candidates = vec![broken.clone(), dir.path().join("missing.png"), good];
        let end = std::time::Instant::now() + Duration::from_secs(10);
        for now in [0.0, 6.0, 12.0] {
            loop {
                rig.wallpapers.update(1, candidates.clone(), (4, 4), 1, now);
                if !rig.wallpapers.pending() {
                    break;
                }
                assert!(std::time::Instant::now() < end);
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        // broken once, good once; the revalidations decode nothing
        assert_eq!(rig.wallpapers.decodes(), 2);
        assert!(rig.wallpapers.outputs[&1].loaded.is_some());
    }

    #[test]
    fn a_pipe_in_place_of_the_file_does_not_block_the_worker() {
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("w.png");
        rustix::fs::mknodat(
            rustix::fs::CWD,
            &fifo,
            rustix::fs::FileType::Fifo,
            rustix::fs::Mode::from_raw_mode(0o600),
            0,
        )
        .unwrap();
        assert!(open_checked(&fifo).unwrap_err().contains("regular"));
        let mut rig = rig();
        rig.settle(&fifo, (4, 4), 0.0);
        assert_eq!(rig.wallpapers.decodes(), 0);
        assert!(rig.wallpapers.outputs[&1].loaded.is_none());
    }

    #[test]
    fn the_decoded_image_is_let_go_when_no_output_wants_its_file_any_more() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_png(dir.path(), "w.png", (16, 8), |_, _| [1, 2, 3, 255]);
        let mut rig = rig();
        let until = |rig: &mut Rig, now: f64, done: &dyn Fn(&Wallpapers) -> bool| {
            let end = std::time::Instant::now() + Duration::from_secs(10);
            while !done(&rig.wallpapers) {
                assert!(std::time::Instant::now() < end, "the worker did not let go");
                rig.wallpapers.update(1, vec![path.clone()], (8, 4), 1, now);
                std::thread::sleep(Duration::from_millis(2));
            }
        };
        // two outputs share the file: one going away keeps the image for the other
        rig.settle(&path, (8, 4), 0.0);
        rig.wallpapers.update(2, vec![path.clone()], (8, 4), 1, 0.0);
        until(&mut rig, 0.0, &|w| {
            !w.pending() && w.cached_source_bytes() == 16 * 8 * 4
        });
        rig.wallpapers.forget(2);
        rig.settle(&path, (4, 2), 1.0);
        assert_eq!(rig.wallpapers.cached_source_bytes(), 16 * 8 * 4);
        // the wallpaper goes back to procedural: nothing wants the file
        rig.wallpapers.forget(1);
        let end = std::time::Instant::now() + Duration::from_secs(10);
        while rig.wallpapers.cached_source_bytes() != 0 {
            assert!(std::time::Instant::now() < end, "the image stayed");
            std::thread::sleep(Duration::from_millis(2));
        }
        // an output nobody asks about for a minute is forgotten too
        rig.wallpapers
            .update(3, vec![path.clone()], (8, 4), 1, 100.0);
        let end = std::time::Instant::now() + Duration::from_secs(10);
        while rig.wallpapers.cached_source_bytes() == 0 {
            assert!(std::time::Instant::now() < end);
            rig.wallpapers
                .update(3, vec![path.clone()], (8, 4), 1, 100.0);
            std::thread::sleep(Duration::from_millis(2));
        }
        rig.wallpapers
            .update(4, vec![dir.path().join("other.png")], (8, 4), 1, 200.0);
        let end = std::time::Instant::now() + Duration::from_secs(10);
        while rig.wallpapers.cached_source_bytes() != 0 {
            assert!(
                std::time::Instant::now() < end,
                "the idle output kept its image"
            );
            rig.wallpapers
                .update(4, vec![dir.path().join("other.png")], (8, 4), 1, 200.0);
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}
