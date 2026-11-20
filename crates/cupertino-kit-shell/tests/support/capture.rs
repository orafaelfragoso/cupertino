//! The capture side of the stand-in, offered when [`Offer::capture`](super::display::Offer::capture) names a transform: output capture sources
//! (`ext_output_image_capture_source_manager_v1`) and copy-capture sessions (`ext_image_copy_capture_manager_v1`) in
//! `argb8888` shared memory. Every frame holds the output's upright picture of [`OUTPUT_SIZE`] pixels turned by the
//! offered transform, as a server sends the frames of a rotated or flipped output, and says so (`transform`) before it
//! is `ready`. Each pixel of the picture tells where it is in the upright picture ([`pixel_of`], [`place_of`]).

use std::os::unix::fs::FileExt;
use std::sync::Mutex;

use cupertino_kit::image::OutputTransform;
use wayland_protocols::ext::image_capture_source::v1::server::ext_image_capture_source_v1::{
    self as source, ExtImageCaptureSourceV1,
};
use wayland_protocols::ext::image_capture_source::v1::server::ext_output_image_capture_source_manager_v1::{
    self as output_sources, ExtOutputImageCaptureSourceManagerV1,
};
use wayland_protocols::ext::image_copy_capture::v1::server::ext_image_copy_capture_frame_v1::{
    self as frame, ExtImageCopyCaptureFrameV1,
};
use wayland_protocols::ext::image_copy_capture::v1::server::ext_image_copy_capture_manager_v1::{
    self as manager, ExtImageCopyCaptureManagerV1,
};
use wayland_protocols::ext::image_copy_capture::v1::server::ext_image_copy_capture_session_v1::{
    self as session, ExtImageCopyCaptureSessionV1,
};
use wayland_server::protocol::wl_buffer::WlBuffer;
use wayland_server::protocol::wl_output;
use wayland_server::protocol::wl_shm;
use wayland_server::{Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource};

use super::display::{ShmBuffer, OUTPUT_SIZE};
use super::State;

/// The colour (R, G, B, A, as a program reads a captured image) of the pixel at `(x, y)` of the output's upright
/// picture: blue and green are the low bytes of x and y, red holds their high bits.
pub fn pixel_of((x, y): (u32, u32)) -> [u8; 4] {
    [
        (((x >> 8) << 4) | (y >> 8)) as u8,
        (y & 0xff) as u8,
        (x & 0xff) as u8,
        u8::MAX,
    ]
}

/// Where in the output's upright picture the pixel of colour `rgba` is; the inverse of [`pixel_of`].
pub fn place_of(rgba: [u8; 4]) -> (u32, u32) {
    let [r, g, b, _] = rgba.map(u32::from);
    (b | ((r >> 4) << 8), g | ((r & 0xf) << 8))
}

/// The transform `wire` names.
fn transform_of(wire: wl_output::Transform) -> OutputTransform {
    OutputTransform::from_wayland(u32::from(wire)).expect("a transform of wl_output")
}

/// Creates the capture globals, whose frames hold the picture turned by `transform`.
pub(super) fn create_globals(handle: &mut DisplayHandle, transform: wl_output::Transform) {
    handle.create_global::<State, ExtOutputImageCaptureSourceManagerV1, ()>(1, ());
    handle.create_global::<State, ExtImageCopyCaptureManagerV1, wl_output::Transform>(1, transform);
}

impl GlobalDispatch<ExtOutputImageCaptureSourceManagerV1, ()> for State {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<ExtOutputImageCaptureSourceManagerV1>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        init.init(resource, ());
    }
}

impl GlobalDispatch<ExtImageCopyCaptureManagerV1, wl_output::Transform> for State {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<ExtImageCopyCaptureManagerV1>,
        transform: &wl_output::Transform,
        init: &mut DataInit<'_, Self>,
    ) {
        init.init(resource, *transform);
    }
}

impl Dispatch<ExtOutputImageCaptureSourceManagerV1, ()> for State {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &ExtOutputImageCaptureSourceManagerV1,
        request: output_sources::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let output_sources::Request::CreateSource { source, .. } = request {
            init.init(source, ());
        }
    }
}

impl Dispatch<ExtImageCaptureSourceV1, ()> for State {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &ExtImageCaptureSourceV1,
        _: source::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
    }
}

impl Dispatch<ExtImageCopyCaptureManagerV1, wl_output::Transform> for State {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &ExtImageCopyCaptureManagerV1,
        request: manager::Request,
        transform: &wl_output::Transform,
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let manager::Request::CreateSession { session, .. } = request {
            let session = init.init(session, *transform);
            let (width, height) = transform_of(*transform).turn_pixels(OUTPUT_SIZE);
            session.buffer_size(width, height);
            session.shm_format(wl_shm::Format::Argb8888);
            session.done();
        }
    }
}

/// A frame of a session: the transform its picture is turned by and the buffer it is copied into.
pub(super) struct Captured {
    transform: wl_output::Transform,
    buffer: Mutex<Option<WlBuffer>>,
}

impl Dispatch<ExtImageCopyCaptureSessionV1, wl_output::Transform> for State {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &ExtImageCopyCaptureSessionV1,
        request: session::Request,
        transform: &wl_output::Transform,
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let session::Request::CreateFrame { frame } = request {
            init.init(
                frame,
                Captured {
                    transform: *transform,
                    buffer: Mutex::new(None),
                },
            );
        }
    }
}

impl Dispatch<ExtImageCopyCaptureFrameV1, Captured> for State {
    fn request(
        _: &mut Self,
        _: &Client,
        resource: &ExtImageCopyCaptureFrameV1,
        request: frame::Request,
        captured: &Captured,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        match request {
            frame::Request::AttachBuffer { buffer } => {
                *captured.buffer.lock().unwrap() = Some(buffer);
            }
            frame::Request::Capture => {
                let buffer = captured.buffer.lock().unwrap().clone();
                match buffer.as_ref().and_then(|b| b.data::<ShmBuffer>()) {
                    Some(shm) => {
                        draw(shm, transform_of(captured.transform));
                        resource.transform(captured.transform);
                        resource.ready();
                    }
                    None => resource.failed(frame::FailureReason::BufferConstraints),
                }
            }
            _ => {}
        }
    }
}

/// Writes the output's picture turned by `transform` into `shm`, in `argb8888`'s byte order (B, G, R, A).
fn draw(shm: &ShmBuffer, transform: OutputTransform) {
    let (width, height) = (shm.width as u32, shm.height as u32);
    let stride = shm.stride as usize;
    let mut bytes = vec![0u8; stride * height as usize];
    for y in 0..height {
        for x in 0..width {
            let [r, g, b, a] = pixel_of(transform.upright_pixel((x, y), (width, height)));
            let at = y as usize * stride + x as usize * 4;
            bytes[at..at + 4].copy_from_slice(&[b, g, r, a]);
        }
    }
    shm.file
        .write_all_at(&bytes, shm.offset as u64)
        .expect("the frame is written into the client's buffer");
}
