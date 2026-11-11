use cupertino_wayland_client::{Connection, OutputId, State};
use image::imageops::FilterType;
use serde::{Deserialize, Serialize};
use std::io::{Cursor, Read, Seek, SeekFrom};
use std::os::fd::AsFd;
use std::sync::{Arc, Mutex};
use wayland_client::protocol::{wl_buffer, wl_output, wl_shm, wl_shm_pool};
use wayland_client::{Connection as WlConnection, Dispatch, QueueHandle, WEnum};
use wayland_protocols::ext::image_capture_source::v1::client::{
    ext_image_capture_source_v1::ExtImageCaptureSourceV1 as Source,
    ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1 as SourceManager,
};
use wayland_protocols::ext::image_copy_capture::v1::client::{
    ext_image_copy_capture_frame_v1::{self as frame, ExtImageCopyCaptureFrameV1 as Frame},
    ext_image_copy_capture_manager_v1::{ExtImageCopyCaptureManagerV1 as CopyManager, Options},
    ext_image_copy_capture_session_v1::{self as session, ExtImageCopyCaptureSessionV1 as Session},
};

/// Maximum bytes in one upright capture; bounds the controller's memory independently of an output's mode.
const MAX_CAPTURE_BYTES: usize = 128 * 1024 * 1024;
/// How a whole output shrinks to its logical size. Catmull-Rom is a sharp cubic: at the common 2/3 and 1/2 ratios it
/// keeps one-pixel text strokes and edges at full contrast, where a triangle (bilinear) filter greys them and nearest
/// neighbour drops whole rows and columns of a glyph; it rings less than Lanczos and costs two thirds of its taps.
const DOWNSCALE_FILTER: FilterType = FilterType::CatmullRom;
/// How far a frame's horizontal and vertical device pixels per logical point may differ, in pixels over the output's
/// sides, before the frame is taken not to be upright: each side of the buffer rounds to a whole pixel.
const UPRIGHT_SLACK_PX: f64 = 1.0;
/// Snaps a region edge that lands on a device pixel boundary up to floating-point error onto that boundary.
const EDGE_EPSILON: f64 = 1e-6;
struct Quiet;
#[derive(Default)]
pub struct Progress {
    pub size: Option<(u32, u32)>,
    pub constraints: bool,
    pub ready: bool,
    pub failed: bool,
    /// The frame's buffer is not upright: the compositor said it applied a transform other than normal.
    pub transformed: bool,
}
#[derive(Clone)]
struct Shared(Arc<Mutex<Progress>>);
pub struct Capturer {
    source: SourceManager,
    copy: CopyManager,
    shm: wl_shm::WlShm,
}
pub struct Capture {
    source: Source,
    session: Session,
    frame: Option<Frame>,
    buffer: Option<wl_buffer::WlBuffer>,
    file: Option<std::fs::File>,
    buffer_size: Option<(u32, u32)>,
    state: Shared,
}
impl Capturer {
    pub fn bind(conn: &mut Connection) -> Option<Self> {
        Some(Self {
            source: conn.bind_extension(1..=1, Quiet)?,
            copy: conn.bind_extension(1..=1, Quiet)?,
            shm: conn.bind_extension(1..=1, Quiet)?,
        })
    }
    pub fn start(&self, conn: &Connection, output: OutputId) -> Result<Capture, String> {
        let output = conn.wl_output(output).ok_or("output disappeared")?;
        let qh = conn.queue_handle();
        let shared = Shared(Arc::new(Mutex::new(Progress::default())));
        let source = self.source.create_source(&output, &qh, Quiet);
        let session = self
            .copy
            .create_session(&source, Options::empty(), &qh, shared.clone());
        Ok(Capture {
            source,
            session,
            frame: None,
            buffer: None,
            file: None,
            buffer_size: None,
            state: shared,
        })
    }
    pub fn prepare(&self, conn: &Connection, capture: &mut Capture) -> Result<(), String> {
        let (w, h) = capture
            .state
            .0
            .lock()
            .unwrap()
            .size
            .ok_or("capture has no buffer constraints")?;
        let bytes = (w as usize)
            .checked_mul(h as usize)
            .and_then(|p| p.checked_mul(4))
            .filter(|n| *n > 0 && *n <= MAX_CAPTURE_BYTES)
            .ok_or("capture exceeds memory limit")?;
        let fd =
            rustix::fs::memfd_create("cupertino-control-frame", rustix::fs::MemfdFlags::CLOEXEC)
                .map_err(|e| e.to_string())?;
        let file = std::fs::File::from(fd);
        file.set_len(bytes as u64).map_err(|e| e.to_string())?;
        let qh = conn.queue_handle();
        let pool = self.shm.create_pool(file.as_fd(), bytes as i32, &qh, Quiet);
        let buffer = pool.create_buffer(
            0,
            w as i32,
            h as i32,
            (w * 4) as i32,
            wl_shm::Format::Argb8888,
            &qh,
            Quiet,
        );
        pool.destroy();
        let frame = capture.session.create_frame(&qh, capture.state.clone());
        frame.attach_buffer(&buffer);
        frame.capture();
        capture.buffer_size = Some((w, h));
        capture.file = Some(file);
        capture.buffer = Some(buffer);
        capture.frame = Some(frame);
        Ok(())
    }
}
impl Drop for Capturer {
    fn drop(&mut self) {
        self.copy.destroy();
        self.source.destroy();
    }
}
impl Capture {
    pub fn progress(&self) -> (bool, bool, bool) {
        let p = self.state.0.lock().unwrap();
        (p.constraints, p.ready, p.failed)
    }
    /// The copied frame, which outlives the capture's protocol objects.
    pub fn pixels(mut self) -> Result<Pixels, String> {
        let p = self.state.0.lock().unwrap();
        if !p.ready || p.failed {
            return Err("capture did not complete".into());
        }
        let upright = !p.transformed;
        drop(p);
        let (width, height) = self.buffer_size.ok_or("capture size is unavailable")?;
        let file = self.file.take().ok_or("capture buffer is unavailable")?;
        Ok(Pixels {
            width,
            height,
            upright,
            file,
        })
    }
}
/// A copied frame: `width` × `height` pixels of 32-bit blue, green, red and alpha in `file`.
pub struct Pixels {
    pub width: u32,
    pub height: u32,
    /// The buffer is upright: its rows run along the output's logical x axis, top first.
    pub upright: bool,
    pub file: std::fs::File,
}
impl Pixels {
    /// The part of the frame `plan` keeps, scaled to its image size, as a PNG image. Encoding takes long for a large
    /// image: it runs off the controller's loop.
    pub fn png(mut self, plan: &Plan) -> Result<Vec<u8>, String> {
        let (w, h) = (self.width, self.height);
        let Crop {
            x,
            y,
            width: cw,
            height: ch,
        } = plan.crop;
        if cw == 0
            || ch == 0
            || x.checked_add(cw).is_none_or(|r| r > w)
            || y.checked_add(ch).is_none_or(|b| b > h)
        {
            return Err("capture crop lies outside the frame".into());
        }
        let row = (w as usize) * 4;
        let rows = row
            .checked_mul(ch as usize)
            .filter(|n| *n <= MAX_CAPTURE_BYTES)
            .ok_or("capture exceeds memory limit")?;
        // only the rows of the crop are read
        let mut bytes = vec![0; rows];
        let file = &mut self.file;
        file.seek(SeekFrom::Start(y as u64 * row as u64))
            .and_then(|_| file.read_exact(&mut bytes))
            .map_err(|e| e.to_string())?;
        let (left, kept) = (x as usize * 4, cw as usize * 4);
        let mut rgba = Vec::with_capacity(kept * ch as usize);
        for line in bytes.chunks_exact(row) {
            rgba.extend_from_slice(&line[left..left + kept]);
        }
        for pixel in rgba.as_chunks_mut::<4>().0 {
            pixel.swap(0, 2);
        }
        let mut image =
            image::RgbaImage::from_raw(cw, ch, rgba).ok_or("capture has an invalid layout")?;
        if plan.size != (cw, ch) {
            image = image::imageops::resize(&image, plan.size.0, plan.size.1, DOWNSCALE_FILTER);
        }
        let mut png = Cursor::new(Vec::new());
        image
            .write_to(&mut png, image::ImageFormat::Png)
            .map_err(|e| e.to_string())?;
        Ok(png.into_inner())
    }
}

/// A rectangle in logical points relative to the captured output, the space pointer motion uses.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Region {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}
impl Region {
    /// Whether the region is a rectangle of positive size entirely inside an output of `output` logical points. A
    /// region reaching outside is refused rather than clamped, so the image is always what was asked for.
    pub fn check(&self, output: (u32, u32)) -> Result<(), String> {
        let Self {
            x,
            y,
            width,
            height,
        } = *self;
        if ![x, y, width, height].iter().all(|v| v.is_finite()) {
            return Err("capture region needs finite numbers".into());
        }
        if width <= 0.0 || height <= 0.0 {
            return Err("capture region needs a positive width and height".into());
        }
        let (ow, oh) = (f64::from(output.0), f64::from(output.1));
        if x < 0.0 || y < 0.0 || x + width > ow || y + height > oh {
            return Err(format!(
                "capture region must lie inside the granted output, 0,0 to {}x{} logical points",
                output.0, output.1
            ));
        }
        Ok(())
    }
}

/// A rectangle of device pixels in a frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Crop {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// What a frame becomes: the device pixels kept, the image size they scale to, and the logical rectangle the image
/// covers. Image pixel `(px, py)` is the logical point `(region.x + px * region.width / size.0, region.y + py *
/// region.height / size.1)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plan {
    pub crop: Crop,
    pub size: (u32, u32),
    pub region: Region,
}

/// The plan for an upright frame of `frame` device pixels of an output of `output` logical points. Without a region the
/// whole output shrinks to one image pixel per logical point (a frame with fewer pixels than points stays as it is);
/// a region is cropped at the frame's own density, its edges moved outward to whole device pixels.
pub fn plan(frame: (u32, u32), output: (u32, u32), region: Option<Region>) -> Result<Plan, String> {
    let (fw, fh) = frame;
    let (ow, oh) = output;
    if fw == 0 || fh == 0 {
        return Err("capture frame is empty".into());
    }
    if ow == 0 || oh == 0 {
        return Err("output geometry is unavailable".into());
    }
    let (sx, sy) = (f64::from(fw) / f64::from(ow), f64::from(fh) / f64::from(oh));
    // an upright frame has one density in both directions; a frame in the panel's own orientation of a turned output
    // has its sides swapped and cannot be mapped to logical points this way
    if (sx - sy).abs() > UPRIGHT_SLACK_PX / f64::from(ow) + UPRIGHT_SLACK_PX / f64::from(oh) {
        return Err(NOT_UPRIGHT.into());
    }
    let whole = Region {
        x: 0.0,
        y: 0.0,
        width: f64::from(ow),
        height: f64::from(oh),
    };
    let Some(region) = region else {
        return Ok(Plan {
            crop: Crop {
                x: 0,
                y: 0,
                width: fw,
                height: fh,
            },
            size: (fw.min(ow), fh.min(oh)),
            region: whole,
        });
    };
    region.check(output)?;
    let start = |v: f64, s: f64, limit: u32| {
        ((v * s + EDGE_EPSILON).floor().max(0.0) as u32).min(limit - 1)
    };
    let end =
        |v: f64, s: f64, limit: u32| ((v * s - EDGE_EPSILON).ceil().max(0.0) as u32).min(limit);
    let (x0, y0) = (start(region.x, sx, fw), start(region.y, sy, fh));
    let x1 = end(region.x + region.width, sx, fw).max(x0 + 1);
    let y1 = end(region.y + region.height, sy, fh).max(y0 + 1);
    let crop = Crop {
        x: x0,
        y: y0,
        width: x1 - x0,
        height: y1 - y0,
    };
    Ok(Plan {
        crop,
        size: (crop.width, crop.height),
        region: Region {
            x: f64::from(x0) / sx,
            y: f64::from(y0) / sy,
            width: f64::from(crop.width) / sx,
            height: f64::from(crop.height) / sy,
        },
    })
}

/// Why a frame that is not upright is refused.
pub const NOT_UPRIGHT: &str =
    "the captured frame is not upright for this output's transform, so its pixels cannot be mapped to logical points";

impl Drop for Capture {
    fn drop(&mut self) {
        if let Some(f) = &self.frame {
            f.destroy();
        }
        if let Some(b) = &self.buffer {
            b.destroy();
        }
        self.session.destroy();
        self.source.destroy();
    }
}
macro_rules! quiet {($($ty:ty),*)=>{$(impl Dispatch<$ty,Quiet> for State {
    fn event(_: &mut Self,_:&$ty,_:<$ty as wayland_client::Proxy>::Event,_:&Quiet,_:&WlConnection,_:&QueueHandle<Self>){}
})*};}
quiet!(
    SourceManager,
    CopyManager,
    Source,
    wl_shm::WlShm,
    wl_shm_pool::WlShmPool,
    wl_buffer::WlBuffer
);
impl Dispatch<Session, Shared> for State {
    fn event(
        _: &mut Self,
        _: &Session,
        event: session::Event,
        state: &Shared,
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {
        let mut p = state.0.lock().unwrap();
        match event {
            session::Event::BufferSize { width, height } => p.size = Some((width, height)),
            session::Event::Done => p.constraints = true,
            session::Event::Stopped => p.failed = true,
            _ => {}
        }
    }
}
impl Dispatch<Frame, Shared> for State {
    fn event(
        _: &mut Self,
        _: &Frame,
        event: frame::Event,
        state: &Shared,
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {
        let mut p = state.0.lock().unwrap();
        match event {
            frame::Event::Ready => p.ready = true,
            frame::Event::Transform { transform } => {
                p.transformed = transform != WEnum::Value(wl_output::Transform::Normal)
            }
            frame::Event::Failed { .. } => p.failed = true,
            _ => {}
        }
    }
}

#[cfg(test)]
#[path = "capture-tests.rs"]
mod tests;
