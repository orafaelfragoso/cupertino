//! Frames of one surface: when to draw, and what of a frame reaches the buffer.

use crate::pixels::{convert_rects, rgba_to_argb8888};
use crate::Error;
use cupertino_geometry::Rect;
use cupertino_wayland_client::{BufferFormat, Connection, PixelRect, Scale, SurfaceId};
use tiny_skia::{Color, Pixmap};

/// What [`ShmPresenter::poll`] asks of the application.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ready {
    Idle,
    /// The surface's scale changed since the last report; a draw follows.
    ScaleChanged(Scale),
    /// Draw a frame now ([`ShmPresenter::draw`]).
    Draw,
}

/// Draws one surface on the CPU with tiny-skia into a kept pixmap, presents it in the transport's shm buffers and paces
/// its frames. A GPU path is a presenter of its own. It holds no protocol object: the
/// application owns the surface's role and reports its configures and frame callbacks.
pub struct ShmPresenter {
    surface: SurfaceId,
    /// Logical size from the last configure.
    size: Option<(u32, u32)>,
    /// Scale last reported by [`ShmPresenter::poll`].
    reported_scale: Scale,
    pixmap: Option<Pixmap>,
    /// The scale the last frame was drawn at: a frame at another one is drawn whole.
    drawn_scale: Option<Scale>,
    /// A configure came since the last frame: the window's states (activation, maximized) may have changed what the
    /// frame shows, so the next one is drawn whole whatever its damage says.
    reconfigured: bool,
    frame_pending: bool,
    wants_redraw: bool,
    zoom: f64,
}

impl ShmPresenter {
    pub fn new(surface: SurfaceId) -> ShmPresenter {
        ShmPresenter {
            surface,
            size: None,
            reported_scale: Scale::ONE,
            pixmap: None,
            drawn_scale: None,
            reconfigured: false,
            frame_pending: false,
            wants_redraw: false,
            zoom: 1.0,
        }
    }

    pub fn surface(&self) -> SurfaceId {
        self.surface
    }

    /// The logical size the last configure asked for; `None` before the first.
    pub fn size(&self) -> Option<(u32, u32)> {
        self.size
    }

    /// A configure asked for `size`; the next poll draws. The size is a hint for layout: the buffer [`draw`] fills
    /// has the size of the latest configure the transport received, which may be newer.
    ///
    /// [`draw`]: Self::draw
    pub fn configured(&mut self, size: (u32, u32)) {
        self.size = Some(size);
        self.reconfigured = true;
        self.wants_redraw = true;
    }

    /// Draw again as soon as the surface can take a frame.
    pub fn request_redraw(&mut self) {
        self.wants_redraw = true;
    }

    /// Show the next frames magnified `zoom` times (at least 1) about their centre, scaled by the compositor where it
    /// can ([`BufferLease::set_zoom`]); 1 shows them as drawn.
    ///
    /// [`BufferLease::set_zoom`]: cupertino_wayland_client::BufferLease::set_zoom
    pub fn set_zoom(&mut self, zoom: f64) {
        self.zoom = zoom;
    }

    /// The frame callback of this surface fired (`Event::Frame`).
    pub fn frame_done(&mut self) {
        self.frame_pending = false;
    }

    /// What to do now: report a new scale first, then a draw once the surface is configured, a frame is asked for,
    /// none is in flight and the transport has a free buffer. Poll until [`Ready::Idle`].
    pub fn poll(&mut self, conn: &Connection) -> Result<Ready, cupertino_wayland_client::Error> {
        if self.size.is_none() {
            return Ok(Ready::Idle);
        }
        let scale = conn.scale(self.surface)?;
        if self.reported_scale != scale {
            self.reported_scale = scale;
            self.wants_redraw = true;
            return Ok(Ready::ScaleChanged(scale));
        }
        if self.wants_redraw && !self.frame_pending && conn.buffer_free(self.surface)? {
            self.wants_redraw = false;
            return Ok(Ready::Draw);
        }
        Ok(Ready::Idle)
    }

    /// Paint a frame and present it. `paint` receives the pixmap and the scale to draw at (device pixels per point).
    /// Without `damage` the pixmap is cleared and the frame is whole. With `damage` (rectangles in points) only those
    /// rectangles (rounded out to device pixels) of the last frame are cleared, and `paint` must repaint them and
    /// leave every other pixel as it is; only they reach the server. Without a last frame of the same size and scale
    /// the frame is whole. Commits the surface with a frame callback.
    pub fn draw(
        &mut self,
        conn: &mut Connection,
        damage: Option<&[Rect]>,
        paint: impl FnOnce(&mut Pixmap, Scale),
    ) -> Result<(), Error> {
        self.draw_with(conn, damage, |pixmap, scale, _| paint(pixmap, scale))
    }

    /// Show the surface as `rgba` (straight sRGB components from 0 to 1) all over: one pixel the compositor stretches
    /// over it ([`Connection::present_solid`]), which costs nothing whatever the surface's size, or where the compositor
    /// cannot, a frame filled with the colour. Commits with a frame callback, like [`draw`]; the next frame drawn is whole.
    ///
    /// [`draw`]: Self::draw
    /// [`Connection::present_solid`]: cupertino_wayland_client::Connection::present_solid
    pub fn draw_solid(&mut self, conn: &mut Connection, rgba: [f32; 4]) -> Result<(), Error> {
        let size = self
            .size
            .ok_or(cupertino_wayland_client::Error::NotConfigured)?;
        match conn.present_solid(self.surface, size, rgba) {
            Ok(()) => {
                self.frame_pending = true;
                // the pixmap no longer holds what is shown
                self.drawn_scale = None;
                self.reconfigured = false;
                Ok(())
            }
            Err(cupertino_wayland_client::Error::Unsupported(_)) => {
                let [r, g, b, a] = rgba.map(|c| c.clamp(0.0, 1.0));
                let color = Color::from_rgba(r, g, b, a).unwrap_or(Color::TRANSPARENT);
                self.draw(conn, None, |pixmap, _| pixmap.fill(color))
            }
            Err(e) => Err(e.into()),
        }
    }

    /// [`draw`] with the device-pixel rectangles that were cleared given to `paint` too: `None` when the frame is whole
    /// (the pixmap was cleared all over), also when `damage` was given but could not be kept.
    ///
    /// [`draw`]: Self::draw
    pub fn draw_with(
        &mut self,
        conn: &mut Connection,
        damage: Option<&[Rect]>,
        paint: impl FnOnce(&mut Pixmap, Scale, Option<&[PixelRect]>),
    ) -> Result<(), Error> {
        let size = self
            .size
            .ok_or(cupertino_wayland_client::Error::NotConfigured)?;
        let mut lease = conn.lease(self.surface, size, BufferFormat::Argb8888)?;
        let layout = lease.layout();
        let (pw, ph) = (layout.width(), layout.height());
        let scale = lease.scale();
        let kept = self
            .pixmap
            .as_ref()
            .is_some_and(|p| p.width() == pw && p.height() == ph)
            && self.drawn_scale == Some(scale)
            && !self.reconfigured;
        let mut pixmap = match self.pixmap.take() {
            Some(p) if p.width() == pw && p.height() == ph => p,
            _ => Pixmap::new(pw, ph).ok_or(Error::BufferSize {
                width: pw,
                height: ph,
            })?,
        };
        let damage: Option<Vec<PixelRect>> = match damage {
            Some(rects) if kept => Some(
                rects
                    .iter()
                    .filter_map(|r| clamp(pixel_rect(r, scale.factor()), pw, ph))
                    .collect(),
            ),
            _ => None,
        };
        match &damage {
            None => pixmap.fill(Color::TRANSPARENT),
            Some(rects) => {
                for r in rects {
                    clear(&mut pixmap, r);
                }
            }
        }
        paint(&mut pixmap, scale, damage.as_deref());
        // the buffer needs what this frame changed and what it missed of the frames shown since it last held one
        let regions: Option<Vec<PixelRect>> = damage
            .as_ref()
            .zip(lease.missed())
            .map(|(damage, missed)| damage.iter().chain(missed).copied().collect())
            // rectangles that add up to the buffer cost more than converting it once
            .filter(|rects: &Vec<PixelRect>| {
                rects
                    .iter()
                    .map(|r| u64::from(r.width) * u64::from(r.height))
                    .sum::<u64>()
                    < u64::from(pw) * u64::from(ph)
            });
        match regions {
            Some(rects) => convert_rects(
                pixmap.data(),
                pixmap.width() as usize * 4,
                lease.pixels_mut(),
                layout.stride(),
                &rects,
            ),
            None => rgba_to_argb8888(pixmap.data(), lease.pixels_mut()),
        }
        lease.set_zoom(self.zoom);
        lease.present_damage(damage.as_deref())?;
        self.frame_pending = true;
        self.pixmap = Some(pixmap);
        self.drawn_scale = Some(scale);
        self.reconfigured = false;
        Ok(())
    }
}

/// The device pixels `r` (points) touches at `factor` pixels a point.
fn pixel_rect(r: &Rect, factor: f64) -> PixelRect {
    let (x0, y0) = (
        (r.x() * factor).floor().max(0.0),
        (r.y() * factor).floor().max(0.0),
    );
    let (x1, y1) = (
        ((r.x() + r.width()) * factor).ceil().max(0.0),
        ((r.y() + r.height()) * factor).ceil().max(0.0),
    );
    PixelRect {
        x: x0 as u32,
        y: y0 as u32,
        width: (x1 - x0) as u32,
        height: (y1 - y0) as u32,
    }
}

/// `r` cut to a `width` × `height` buffer; `None` when nothing of it is inside.
fn clamp(r: PixelRect, width: u32, height: u32) -> Option<PixelRect> {
    let (x1, y1) = (
        r.x.saturating_add(r.width).min(width),
        r.y.saturating_add(r.height).min(height),
    );
    (r.x < x1 && r.y < y1).then(|| PixelRect {
        x: r.x,
        y: r.y,
        width: x1 - r.x,
        height: y1 - r.y,
    })
}

/// Copy the pixels of `rects` (device pixels inside both) from `src` to `dst`, leaving the rest of `dst` as it is.
pub fn copy_rects(dst: &mut Pixmap, src: &Pixmap, rects: &[PixelRect]) {
    debug_assert_eq!(
        (dst.width(), dst.height()),
        (src.width(), src.height()),
        "copy_rects takes pixmaps of one size"
    );
    let stride = dst.width() as usize * 4;
    let (src_data, dst_data) = (src.data(), dst.data_mut());
    for r in rects {
        let (start, len) = (r.x as usize * 4, r.width as usize * 4);
        for row in r.y..r.y + r.height {
            let at = row as usize * stride + start;
            dst_data[at..at + len].copy_from_slice(&src_data[at..at + len]);
        }
    }
}

/// Make the pixels of `r` (inside `pixmap`) transparent.
fn clear(pixmap: &mut Pixmap, r: &PixelRect) {
    let stride = pixmap.width() as usize * 4;
    let (start, len) = (r.x as usize * 4, r.width as usize * 4);
    let data = pixmap.data_mut();
    for row in r.y..r.y + r.height {
        let at = row as usize * stride + start;
        data[at..at + len].fill(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn damage_is_cut_to_the_buffer() {
        let r = PixelRect {
            x: 90,
            y: 5,
            width: 20,
            height: 10,
        };
        assert_eq!(
            clamp(r, 100, 100),
            Some(PixelRect {
                x: 90,
                y: 5,
                width: 10,
                height: 10
            })
        );
        assert_eq!(clamp(r, 90, 100), None);
    }

    #[test]
    fn damage_rounds_outwards_to_whole_pixels_at_a_fractional_scale() {
        assert_eq!(
            pixel_rect(&Rect::new(1.0, 1.0, 2.0, 1.0), 1.25),
            PixelRect {
                x: 1,
                y: 1,
                width: 3,
                height: 2
            }
        );
    }

    #[test]
    fn copying_rects_changes_only_those_pixels() {
        let mut dst = Pixmap::new(4, 3).unwrap();
        dst.fill(Color::from_rgba8(1, 2, 3, 255));
        let mut src = Pixmap::new(4, 3).unwrap();
        src.fill(Color::from_rgba8(9, 9, 9, 255));
        copy_rects(
            &mut dst,
            &src,
            &[PixelRect {
                x: 1,
                y: 1,
                width: 2,
                height: 1,
            }],
        );
        let changed: Vec<(u32, u32)> = (0..3)
            .flat_map(|y| (0..4).map(move |x| (x, y)))
            .filter(|&(x, y)| dst.pixel(x, y).unwrap().red() == 9)
            .collect();
        assert_eq!(changed, vec![(1, 1), (2, 1)]);
    }
}
