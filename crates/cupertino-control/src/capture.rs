use cupertino_wayland_client::{Connection, OutputId, State};
use std::io::{Cursor, Read, Seek, SeekFrom};
use std::os::fd::AsFd;
use std::sync::{Arc, Mutex};
use wayland_client::protocol::{wl_buffer, wl_shm, wl_shm_pool};
use wayland_client::{Connection as WlConnection, Dispatch, QueueHandle};
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
struct Quiet;
#[derive(Default)]
pub struct Progress {
    pub size: Option<(u32, u32)>,
    pub constraints: bool,
    pub ready: bool,
    pub failed: bool,
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
        drop(p);
        let (width, height) = self.buffer_size.ok_or("capture size is unavailable")?;
        let file = self.file.take().ok_or("capture buffer is unavailable")?;
        Ok(Pixels {
            width,
            height,
            file,
        })
    }
}
/// A copied frame: `width` × `height` pixels of 32-bit blue, green, red and alpha in `file`.
pub struct Pixels {
    pub width: u32,
    pub height: u32,
    pub file: std::fs::File,
}
impl Pixels {
    /// The frame upright as a PNG image, with its size. Encoding takes long for a large output: it runs off the
    /// controller's loop.
    pub fn png(mut self) -> Result<(u32, u32, Vec<u8>), String> {
        let (w, h) = (self.width, self.height);
        let size = (w as usize)
            .checked_mul(h as usize)
            .and_then(|p| p.checked_mul(4))
            .filter(|n| *n <= MAX_CAPTURE_BYTES)
            .ok_or("capture exceeds memory limit")?;
        let mut bytes = vec![0; size];
        let file = &mut self.file;
        file.seek(SeekFrom::Start(0))
            .and_then(|_| file.read_exact(&mut bytes))
            .map_err(|e| e.to_string())?;
        for pixel in bytes.as_chunks_mut::<4>().0 {
            pixel.swap(0, 2);
        }
        let image =
            image::RgbaImage::from_raw(w, h, bytes).ok_or("capture has an invalid layout")?;
        let mut png = Cursor::new(Vec::new());
        image
            .write_to(&mut png, image::ImageFormat::Png)
            .map_err(|e| e.to_string())?;
        Ok((w, h, png.into_inner()))
    }
}
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
            frame::Event::Failed { .. } => p.failed = true,
            _ => {}
        }
    }
}
