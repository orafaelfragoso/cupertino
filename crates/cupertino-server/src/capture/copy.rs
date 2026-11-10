//! `ext-image-copy-capture-v1`: sessions on a capture source and the frames they copy into shm buffers.
//!
//! A session announces the buffer size of its source and the two shm formats a frame can be copied into, and again
//! whenever the size changes (a frame whose buffer no longer fits fails with `buffer_constraints`). Damage is the
//! bounding rectangle of the pixels that differ from the session's previous frame; the first frame is all damage.
//! Cursor sessions (`create_pointer_cursor_session`) are accepted but their capture session stops at once: no shell
//! process captures the cursor image separately.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::{ExportMem, Texture};
use smithay::reexports::wayland_protocols::ext::image_copy_capture::v1::server::ext_image_copy_capture_cursor_session_v1::{
    self as cursor_session, ExtImageCopyCaptureCursorSessionV1,
};
use smithay::reexports::wayland_protocols::ext::image_copy_capture::v1::server::ext_image_copy_capture_frame_v1::{
    self as frame, ExtImageCopyCaptureFrameV1, FailureReason,
};
use smithay::reexports::wayland_protocols::ext::image_copy_capture::v1::server::ext_image_copy_capture_manager_v1::{
    self as manager, ExtImageCopyCaptureManagerV1, Options,
};
use smithay::reexports::wayland_protocols::ext::image_copy_capture::v1::server::ext_image_copy_capture_session_v1::{
    self as session, ExtImageCopyCaptureSessionV1,
};
use smithay::reexports::wayland_server::backend::ClientId;
use smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer;
use smithay::reexports::wayland_server::protocol::{wl_output, wl_shm};
use smithay::reexports::wayland_server::{Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, WEnum};
use smithay::utils::Rectangle;
use smithay::wayland::shm::{with_buffer_contents_mut, BufferAccessError};

use super::{capture_client, Placement, Source};
use crate::render::scene::capture_window;
use crate::state::Cupertino;

/// The shm formats a frame is copied into. Both keep blue, green, red, alpha (or unused) in memory order.
const SHM_FORMATS: [wl_shm::Format; 2] = [wl_shm::Format::Argb8888, wl_shm::Format::Xrgb8888];

/// A rendered source: RGBA bytes, rows top to bottom without padding.
pub(super) struct Image {
    width: i32,
    height: i32,
    rgba: Vec<u8>,
}

impl Image {
    fn new(width: i32, height: i32, rgba: Vec<u8>) -> Result<Self> {
        let expected = usize::try_from(width)? * usize::try_from(height)? * 4;
        if rgba.len() != expected {
            return Err(anyhow!(
                "a {width} x {height} image read back {} bytes, not {expected}",
                rgba.len()
            ));
        }
        Ok(Self {
            width,
            height,
            rgba,
        })
    }

    /// An opaque black image of `size`.
    fn black((width, height): (i32, i32)) -> Result<Self> {
        let pixels = usize::try_from(width)? * usize::try_from(height)?;
        Self::new(width, height, [0, 0, 0, 255].repeat(pixels))
    }

    fn size(&self) -> (i32, i32) {
        (self.width, self.height)
    }
}

/// The bounding rectangle `(x, y, width, height)` of the pixels that differ between two images of one size; `None`
/// when they are equal.
fn changed_rect(old: &Image, new: &Image) -> Option<(i32, i32, i32, i32)> {
    let row_bytes = new.width as usize * 4;
    let (mut x0, mut x1, mut y0, mut y1) = (usize::MAX, 0, usize::MAX, 0);
    for (y, (a, b)) in old
        .rgba
        .chunks_exact(row_bytes)
        .zip(new.rgba.chunks_exact(row_bytes))
        .enumerate()
    {
        if a == b {
            continue;
        }
        let differs = |x: &usize| a[x * 4..x * 4 + 4] != b[x * 4..x * 4 + 4];
        let width = new.width as usize;
        let first = (0..width)
            .find(differs)
            .expect("a differing row has a differing pixel");
        let last = (0..width)
            .rev()
            .find(differs)
            .expect("a differing row has a differing pixel");
        x0 = x0.min(first);
        x1 = x1.max(last);
        y0 = y0.min(y);
        y1 = y1.max(y);
    }
    (y0 != usize::MAX).then(|| {
        (
            x0 as i32,
            y0 as i32,
            (x1 - x0 + 1) as i32,
            (y1 - y0 + 1) as i32,
        )
    })
}

/// Why a frame could not be copied into its buffer.
enum CopyError {
    /// Not an shm buffer of the session's size and formats.
    Constraints,
    /// The buffer's memory could not be accessed (the pool shrank under it); the client got a protocol error.
    Access,
}

/// Copy `image` into the shm `buffer`, swapping red and blue.
fn copy_into(buffer: &WlBuffer, image: &Image) -> std::result::Result<(), CopyError> {
    let copied = with_buffer_contents_mut(buffer, |ptr, len, data| {
        let row_bytes = image.width as usize * 4;
        if (data.width, data.height) != image.size()
            || !SHM_FORMATS.contains(&data.format)
            || (data.stride as usize) < row_bytes
        {
            return Err(CopyError::Constraints);
        }
        let (offset, stride) = (data.offset as usize, data.stride as usize);
        if offset + stride * (image.height as usize - 1) + row_bytes > len {
            return Err(CopyError::Constraints);
        }
        // Safety: smithay maps the pool for the duration of the call; the bounds were checked above.
        let memory = unsafe { std::slice::from_raw_parts_mut(ptr, len) };
        for (y, row) in image.rgba.chunks_exact(row_bytes).enumerate() {
            let start = offset + y * stride;
            for (dst, src) in memory[start..start + row_bytes]
                .as_chunks_mut::<4>()
                .0
                .iter_mut()
                .zip(row.as_chunks::<4>().0)
            {
                *dst = [src[2], src[1], src[0], src[3]];
            }
        }
        Ok(())
    });
    match copied {
        Ok(result) => result,
        Err(BufferAccessError::NotManaged) => Err(CopyError::Constraints),
        Err(_) => Err(CopyError::Access),
    }
}

/// User data of a session object and of the frames created from it: the session's id.
pub struct SessionId(u64);

/// User data of a cursor session: what it captures and whether its capture session was taken.
pub struct CursorSessionData {
    taken: AtomicBool,
}

struct Session {
    id: u64,
    resource: ExtImageCopyCaptureSessionV1,
    source: Source,
    paint_cursors: bool,
    /// The buffer size of the constraints last sent.
    size: Option<(i32, i32)>,
    /// The frame last made ready, for the damage of the next.
    last: Option<Arc<Image>>,
    stopped: bool,
}

impl Session {
    fn send_constraints(&mut self, size: (i32, i32)) {
        self.resource.buffer_size(size.0 as u32, size.1 as u32);
        for format in SHM_FORMATS {
            self.resource.shm_format(format);
        }
        self.resource.done();
        self.size = Some(size);
        self.last = None;
    }

    fn stop(&mut self) {
        if !self.stopped {
            self.stopped = true;
            self.resource.stopped();
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FrameState {
    /// Taking a buffer and damage.
    Open,
    /// `capture` was sent; waiting for the source to be drawn (and, after the first frame, to change).
    Waiting,
    /// `ready` or `failed` was sent.
    Done,
}

struct Frame {
    resource: ExtImageCopyCaptureFrameV1,
    session: u64,
    buffer: Option<WlBuffer>,
    state: FrameState,
}

impl Frame {
    fn fail(&mut self, reason: FailureReason) {
        self.resource.failed(reason);
        self.state = FrameState::Done;
    }
}

/// The copy sessions and their frames.
#[derive(Default)]
pub(super) struct Sessions {
    sessions: Vec<Session>,
    frames: Vec<Frame>,
    next_id: u64,
}

impl Sessions {
    fn allocate(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    fn frame_mut(&mut self, resource: &ExtImageCopyCaptureFrameV1) -> &mut Frame {
        self.frames
            .iter_mut()
            .find(|f| f.resource == *resource)
            .expect("a frame stays listed until its object is destroyed")
    }
}

/// What one offscreen rendering serves: the window (`None` for the whole output) and whether the cursor is painted.
type RenderKey = (Option<cupertino_wm::WindowId>, bool);

impl Cupertino {
    fn add_session(
        &mut self,
        id: u64,
        resource: ExtImageCopyCaptureSessionV1,
        source: Source,
        paint_cursors: bool,
    ) {
        let mut session = Session {
            id,
            resource,
            source,
            paint_cursors,
            size: None,
            last: None,
            stopped: false,
        };
        let allowed = session
            .resource
            .client()
            .is_some_and(|c| self.capture_source_allowed(&c, &session.source));
        match self.placement(&session.source).filter(|_| allowed) {
            Some(p) => session.send_constraints(p.size),
            None => session.stop(),
        }
        self.capture.copy.sessions.push(session);
        self.refresh_capture_indicator();
    }

    /// Stop the copy sessions whose connection no longer holds the Capture role (its process exited). Frames waiting
    /// on a stopped session fail.
    pub(crate) fn stop_captures(&mut self) {
        let allowed: Vec<_> = self
            .capture
            .copy
            .sessions
            .iter()
            .map(|s| {
                s.resource
                    .client()
                    .is_some_and(|c| self.capture_source_allowed(&c, &s.source))
            })
            .collect();
        let copy = &mut self.capture.copy;
        let mut ended = Vec::new();
        for (session, holds) in copy
            .sessions
            .iter_mut()
            .zip(allowed)
            .filter(|(s, _)| !s.stopped)
        {
            if !holds {
                session.stop();
                ended.push(session.id);
            }
        }
        if ended.is_empty() {
            return;
        }
        for f in copy
            .frames
            .iter_mut()
            .filter(|f| f.state == FrameState::Waiting && ended.contains(&f.session))
        {
            f.fail(FailureReason::Stopped);
        }
        self.refresh_capture_indicator();
    }

    /// Whether a copy session runs: what the menu bar's screen capture indicator shows.
    pub fn capture_running(&self) -> bool {
        self.capture.copy.sessions.iter().any(|s| !s.stopped)
    }

    /// Stop the sessions whose source went away, fail the frames waiting on them, then copy the waiting frames of the
    /// sources drawn on output `idx`. While the session is locked an output source is black and a window source keeps
    /// its frames waiting.
    pub(super) fn copy_frames(&mut self, renderer: &mut GlesRenderer, idx: usize) {
        let locked = self.session_lock.is_active();
        let placements: Vec<Option<Placement>> = self
            .capture
            .copy
            .sessions
            .iter()
            .map(|s| self.placement(&s.source))
            .collect();
        let copy = &mut self.capture.copy;
        let mut stopped = false;
        for (session, placement) in copy.sessions.iter_mut().zip(&placements) {
            if placement.is_none() && !session.stopped {
                session.stop();
                stopped = true;
            }
        }
        if stopped {
            self.refresh_capture_indicator();
        }
        let copy = &mut self.capture.copy;
        let mut due: Vec<(usize, usize, Placement)> = Vec::new();
        for (fi, f) in copy
            .frames
            .iter_mut()
            .enumerate()
            .filter(|(_, f)| f.state == FrameState::Waiting)
        {
            match copy
                .sessions
                .iter()
                .position(|s| s.id == f.session)
                .filter(|si| !copy.sessions[*si].stopped)
            {
                None => f.fail(FailureReason::Stopped),
                Some(si) => {
                    let placement =
                        placements[si].expect("a session that is not stopped has a placement");
                    if placement.output == idx && !(locked && placement.window.is_some()) {
                        due.push((fi, si, placement));
                    }
                }
            }
        }
        // sessions on one source with the same cursor option share one rendering
        let mut rendered: Vec<(RenderKey, Arc<Image>)> = Vec::new();
        for (fi, si, placement) in due {
            let key = (
                placement.window,
                self.capture.copy.sessions[si].paint_cursors,
            );
            let image = match rendered.iter().find(|(k, _)| *k == key) {
                Some((_, image)) => image.clone(),
                None => {
                    let image = match placement.window {
                        None if locked => Image::black(placement.size),
                        None => self.render_output_image(renderer, idx, key.1),
                        Some(window) => self.render_toplevel_image(renderer, idx, window),
                    };
                    match image {
                        Ok(image) => {
                            let image = Arc::new(image);
                            rendered.push((key, image.clone()));
                            image
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "rendering a capture source failed");
                            self.capture.copy.frames[fi].fail(FailureReason::Unknown);
                            continue;
                        }
                    }
                }
            };
            let time = Duration::from(self.clock.now());
            let copy = &mut self.capture.copy;
            let session = &mut copy.sessions[si];
            if session.size != Some(image.size()) {
                session.send_constraints(image.size());
            }
            let damage = match &session.last {
                None => Some((0, 0, image.width, image.height)),
                Some(last) => changed_rect(last, &image),
            };
            // an unchanged source keeps the frame waiting for the next change
            let Some((x, y, w, h)) = damage else { continue };
            let f = &mut copy.frames[fi];
            let buffer = f
                .buffer
                .as_ref()
                .expect("a frame waits only after a buffer was attached");
            match copy_into(buffer, &image) {
                Ok(()) => {
                    f.resource.transform(wl_output::Transform::Normal);
                    f.resource.damage(x, y, w, h);
                    f.resource.presentation_time(
                        (time.as_secs() >> 32) as u32,
                        time.as_secs() as u32,
                        time.subsec_nanos(),
                    );
                    f.resource.ready();
                    f.state = FrameState::Done;
                    session.last = Some(image);
                }
                Err(CopyError::Constraints) => f.fail(FailureReason::BufferConstraints),
                Err(CopyError::Access) => f.fail(FailureReason::Unknown),
            }
        }
    }

    /// Output `idx` as its next frame shows it, the cursor drawn only with `paint_cursors`.
    fn render_output_image(
        &mut self,
        renderer: &mut GlesRenderer,
        idx: usize,
        paint_cursors: bool,
    ) -> Result<Image> {
        // the output as it is shown, upright, so the frame's transform is normal
        let image = self.draw_output_image(renderer, idx, paint_cursors, false)?;
        let (w, h) = image.dimensions();
        Image::new(w as i32, h as i32, image.into_raw())
    }

    /// The window at its natural size with its decorations and without its shadow, as the window snapshots draw it.
    fn render_toplevel_image(
        &mut self,
        renderer: &mut GlesRenderer,
        idx: usize,
        window: cupertino_wm::WindowId,
    ) -> Result<Image> {
        let frame = self
            .wm
            .window(window)
            .context("the window left the model")?
            .geometry();
        let active = self.wm.key_window() == Some(window);
        let (texture, _) = self
            .with_scene(renderer, idx, |r, mut p| {
                let windows = p.windows;
                let entry = windows.iter().find(|w| w.id == window)?;
                let geo = entry.window.geometry();
                let natural = cupertino_wm::Rect::new(
                    frame.x,
                    frame.y,
                    f64::from(geo.size.w) + entry.insets().horizontal(),
                    f64::from(geo.size.h) + entry.insets().vertical(),
                );
                capture_window(r, &mut p, entry, natural, active, 0.0, false)
            })
            .flatten()
            .context("the window could not be rendered")?;
        let size = texture.size();
        let mapping = renderer
            .copy_texture(&texture, Rectangle::from_size(size), Fourcc::Abgr8888)
            .map_err(|e| anyhow!("reading the window back: {e}"))?;
        let bytes = renderer
            .map_texture(&mapping)
            .map_err(|e| anyhow!("mapping the window: {e}"))?;
        Image::new(size.w, size.h, bytes.to_vec())
    }
}

impl GlobalDispatch<ExtImageCopyCaptureManagerV1, ()> for Cupertino {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<ExtImageCopyCaptureManagerV1>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        init.init(resource, ());
    }

    fn can_view(client: Client, _: &()) -> bool {
        capture_client(&client)
    }
}

impl Dispatch<ExtImageCopyCaptureManagerV1, ()> for Cupertino {
    fn request(
        state: &mut Self,
        _: &Client,
        resource: &ExtImageCopyCaptureManagerV1,
        request: manager::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        match request {
            manager::Request::CreateSession {
                session,
                source,
                options,
            } => {
                let source = source
                    .data::<Source>()
                    .expect("every capture source is created by a source manager")
                    .clone();
                let id = state.capture.copy.allocate();
                let resource_session = init.init(session, SessionId(id));
                match options {
                    WEnum::Value(options) => state.add_session(
                        id,
                        resource_session,
                        source,
                        options.contains(Options::PaintCursors),
                    ),
                    WEnum::Unknown(bits) => resource.post_error(
                        manager::Error::InvalidOption,
                        format!("unknown option bits {bits:#x}"),
                    ),
                }
            }
            manager::Request::CreatePointerCursorSession { session, .. } => {
                init.init(
                    session,
                    CursorSessionData {
                        taken: AtomicBool::new(false),
                    },
                );
            }
            manager::Request::Destroy => {}
            _ => unreachable!("ext_image_copy_capture_manager_v1 version 1 has no other request"),
        }
    }
}

impl Dispatch<ExtImageCopyCaptureSessionV1, SessionId> for Cupertino {
    fn request(
        state: &mut Self,
        _: &Client,
        resource: &ExtImageCopyCaptureSessionV1,
        request: session::Request,
        data: &SessionId,
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        match request {
            session::Request::CreateFrame { frame } => {
                let resource_frame = init.init(frame, SessionId(data.0));
                let copy = &mut state.capture.copy;
                if copy.frames.iter().any(|f| f.session == data.0) {
                    resource.post_error(
                        session::Error::DuplicateFrame,
                        "the session already has a frame",
                    );
                    return;
                }
                copy.frames.push(Frame {
                    resource: resource_frame,
                    session: data.0,
                    buffer: None,
                    state: FrameState::Open,
                });
            }
            session::Request::Destroy => {}
            _ => unreachable!("ext_image_copy_capture_session_v1 version 1 has no other request"),
        }
    }

    /// Frames waiting on the session fail; the others fail when they ask for a capture.
    fn destroyed(
        state: &mut Self,
        _: ClientId,
        _: &ExtImageCopyCaptureSessionV1,
        data: &SessionId,
    ) {
        let copy = &mut state.capture.copy;
        copy.sessions.retain(|s| s.id != data.0);
        for f in copy
            .frames
            .iter_mut()
            .filter(|f| f.session == data.0 && f.state == FrameState::Waiting)
        {
            f.fail(FailureReason::Stopped);
        }
        state.refresh_capture_indicator();
    }
}

impl Dispatch<ExtImageCopyCaptureFrameV1, SessionId> for Cupertino {
    fn request(
        state: &mut Self,
        _client: &Client,
        resource: &ExtImageCopyCaptureFrameV1,
        request: frame::Request,
        _: &SessionId,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        if matches!(request, frame::Request::Destroy) {
            return;
        }
        state.refresh_control();
        state.stop_captures();
        let copy = &mut state.capture.copy;
        let f = copy.frame_mut(resource);
        if f.state != FrameState::Open {
            resource.post_error(
                frame::Error::AlreadyCaptured,
                "the frame was already captured",
            );
            return;
        }
        match request {
            frame::Request::AttachBuffer { buffer } => f.buffer = Some(buffer),
            // every capture copies the whole buffer, so the client's damage needs no bookkeeping
            frame::Request::DamageBuffer {
                x,
                y,
                width,
                height,
            } => {
                if x < 0 || y < 0 || width <= 0 || height <= 0 {
                    resource.post_error(
                        frame::Error::InvalidBufferDamage,
                        format!("damage {x},{y} {width}x{height}"),
                    );
                }
            }
            frame::Request::Capture => {
                if f.buffer.is_none() {
                    resource.post_error(frame::Error::NoBuffer, "capture without a buffer");
                    return;
                }
                f.state = FrameState::Waiting;
                let session = f.session;
                if copy
                    .sessions
                    .iter()
                    .find(|s| s.id == session)
                    .is_none_or(|s| s.stopped)
                {
                    copy.frame_mut(resource).fail(FailureReason::Stopped);
                    return;
                }
                state.request_redraw();
            }
            _ => unreachable!("ext_image_copy_capture_frame_v1 version 1 has no other request"),
        }
    }

    fn destroyed(
        state: &mut Self,
        _: ClientId,
        resource: &ExtImageCopyCaptureFrameV1,
        _: &SessionId,
    ) {
        state
            .capture
            .copy
            .frames
            .retain(|f| f.resource != *resource);
    }
}

impl Dispatch<ExtImageCopyCaptureCursorSessionV1, CursorSessionData> for Cupertino {
    fn request(
        state: &mut Self,
        _: &Client,
        resource: &ExtImageCopyCaptureCursorSessionV1,
        request: cursor_session::Request,
        data: &CursorSessionData,
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        match request {
            cursor_session::Request::GetCaptureSession { session } => {
                let id = state.capture.copy.allocate();
                let resource_session = init.init(session, SessionId(id));
                if data.taken.swap(true, Ordering::Relaxed) {
                    resource.post_error(
                        cursor_session::Error::DuplicateSession,
                        "the cursor session already has its capture session",
                    );
                    return;
                }
                let mut session = Session {
                    id,
                    resource: resource_session,
                    source: Source::Output(None),
                    paint_cursors: false,
                    size: None,
                    last: None,
                    stopped: false,
                };
                session.stop();
                state.capture.copy.sessions.push(session);
            }
            cursor_session::Request::Destroy => {}
            _ => unreachable!(
                "ext_image_copy_capture_cursor_session_v1 version 1 has no other request"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(width: i32, height: i32, set: &[(i32, i32)]) -> Image {
        let mut rgba = vec![0u8; (width * height * 4) as usize];
        for (x, y) in set {
            rgba[((y * width + x) * 4) as usize] = 255;
        }
        Image::new(width, height, rgba).unwrap()
    }

    #[test]
    fn equal_images_have_no_damage() {
        assert_eq!(
            changed_rect(&image(8, 6, &[(2, 2)]), &image(8, 6, &[(2, 2)])),
            None
        );
    }

    #[test]
    fn damage_is_the_bounding_rectangle_of_the_changed_pixels() {
        let old = image(8, 6, &[]);
        assert_eq!(
            changed_rect(&old, &image(8, 6, &[(3, 1)])),
            Some((3, 1, 1, 1))
        );
        assert_eq!(
            changed_rect(&old, &image(8, 6, &[(6, 1), (1, 4)])),
            Some((1, 1, 6, 4))
        );
    }

    #[test]
    fn an_image_needs_four_bytes_per_pixel() {
        assert!(Image::new(2, 2, vec![0; 15]).is_err());
    }
}
