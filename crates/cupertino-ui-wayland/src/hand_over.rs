//! A submitted frame drawn and committed away from the UI thread ([`WindowSystem::submit`]). Each surface it draws (its
//! surface layers, its raised layer, then the window's own) has a buffer prepared on the UI thread, a GPU buffer to draw
//! into ([`Draw`]) or a shared-memory buffer detached with the commit that shows it, and is drawn by its renderer on
//! the thread the frame is handed to. Once every surface is drawn the frame goes back to the UI thread and the surfaces
//! commit in that order, the layers before the window: a synchronized subsurface's commit waits for the window's, and
//! the window's commit applies the layers' values, so the compositor shows them all together. When one surface cannot
//! be drawn, none commits.
//!
//! [`WindowSystem::submit`]: cupertino_ui_core::platform::WindowSystem::submit

use std::any::Any;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::PoisonError;

use cupertino_ui_core::platform::{LayerList, Submission};
use cupertino_ui_render::{CpuFrame, MemoryTarget, RenderError, RenderFrameId};
use cupertino_wayland_client::{
    BufferFormat, BufferLayout, Commit, DetachedBuffer, Error as WlError, PixelRect, SurfaceId,
};

use crate::gpu_present::{Draw, Drawn, PresentError};
use crate::pixels::{convert_rects, copy_rects, disjoint};

/// Which of a window's surfaces a part of its frame is drawn into.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Of {
    Window,
    Raised,
    /// The surface layer with this key.
    Layer(u64),
}

impl Of {
    /// The name of its raster in the profile: `content` for the window's own.
    fn name(self) -> &'static str {
        match self {
            Of::Window => "content",
            Of::Raised => "raised",
            Of::Layer(_) => "surface",
        }
    }
}

/// The buffer a surface's frame is drawn into, prepared on the UI thread.
pub(crate) enum Target {
    Gpu(Draw),
    /// A shared-memory buffer and the commit that shows it.
    Shm(DetachedBuffer, Commit),
}

/// One surface of a submitted frame, ready to be drawn.
pub(crate) struct Part {
    pub(crate) of: Of,
    pub(crate) surface: SurfaceId,
    pub(crate) list: LayerList,
    /// The damage of the surface's frames that were not shown, drawn and shown with this one.
    pub(crate) carried: Vec<PixelRect>,
    /// The frame is of another size than the one the surface shows: the compositor takes all of it as changed.
    pub(crate) resized: bool,
    pub(crate) target: Target,
}

/// Why a surface's frame was not shown.
pub(crate) enum Failure {
    Render(RenderError),
    Present(PresentError),
}

/// A surface whose frame was ready but not shown, because another surface of the frame was not drawn.
fn left_unshown() -> Failure {
    Failure::Present(PresentError::Resized)
}

/// How a surface of a frame was drawn, sent back to the UI thread before any commit of the frame goes out.
pub(crate) struct Back {
    pub(crate) of: Of,
    pub(crate) surface: SurfaceId,
    /// The frame's size in pixels.
    pub(crate) size: (u32, u32),
    pub(crate) drew: Drew,
}

pub(crate) enum Drew {
    /// Into a GPU buffer: the frame's id and whether it was copied from its renderer's texture.
    Gpu(Result<(RenderFrameId, bool), Failure>),
    /// Into a shared-memory buffer, given back: the frame's id and the damage committed (`None` for all of it).
    Shm(
        DetachedBuffer,
        Result<(RenderFrameId, Option<Vec<PixelRect>>), Failure>,
    ),
}

/// The commit of a drawn surface.
enum Send {
    Gpu(Drawn),
    Shm(Commit, Option<Vec<PixelRect>>),
}

/// The commits of a frame whose surfaces were all drawn, in the order they go out.
pub(crate) struct Commits(Vec<Send>);

impl Commits {
    /// Sends them, the layers' before the window's.
    pub(crate) fn send(self) -> Result<(), WlError> {
        for send in self.0 {
            match send {
                Send::Gpu(drawn) => drawn.commit()?,
                Send::Shm(commit, damage) => tracing::trace_span!("wl.commit")
                    .in_scope(|| commit.commit(damage.as_deref()))?,
            }
        }
        Ok(())
    }
}

/// A frame's surfaces drawn: how each went, the panic of a renderer that panicked, and the commits to send.
pub(crate) struct Drawing {
    pub(crate) backs: Vec<Back>,
    pub(crate) panic: Option<Box<dyn Any + std::marker::Send>>,
    pub(crate) commits: Commits,
}

/// Draws `parts` in order with `submission`'s resources and scale. A part that cannot be drawn (its renderer failed or
/// panicked) leaves the parts after it undrawn and the frame without commits.
pub(crate) fn draw(parts: Vec<Part>, submission: &Submission) -> Drawing {
    let mut backs = Vec::with_capacity(parts.len());
    let mut sends = Vec::with_capacity(parts.len());
    let mut panic = None;
    let mut stopped = false;
    for part in parts {
        let Part {
            of,
            surface,
            list,
            carried,
            resized,
            target,
        } = part;
        let size = (list.width, list.height);
        let drew = match target {
            Target::Gpu(_) if stopped => Drew::Gpu(Err(left_unshown())),
            Target::Shm(buffer, _) if stopped => Drew::Shm(buffer, Err(left_unshown())),
            Target::Gpu(draw) => {
                let drawn = catch_unwind(AssertUnwindSafe(|| {
                    draw_gpu(draw, of, &list, submission, &carried)
                }));
                match drawn {
                    Ok(Ok((id, drawn))) => {
                        let from_texture = drawn.from_texture;
                        sends.push(Send::Gpu(drawn));
                        Drew::Gpu(Ok((id, from_texture)))
                    }
                    Ok(Err(e)) => Drew::Gpu(Err(e)),
                    Err(p) => {
                        panic = Some(p);
                        Drew::Gpu(Err(left_unshown()))
                    }
                }
            }
            Target::Shm(mut buffer, commit) => {
                let drawn = catch_unwind(AssertUnwindSafe(|| {
                    draw_shm(&mut buffer, of, &list, submission, &carried, resized)
                }));
                match drawn {
                    Ok(Ok((id, damage))) => {
                        sends.push(Send::Shm(commit, damage.clone()));
                        Drew::Shm(buffer, Ok((id, damage)))
                    }
                    Ok(Err(e)) => Drew::Shm(buffer, Err(e)),
                    Err(p) => {
                        panic = Some(p);
                        Drew::Shm(buffer, Err(left_unshown()))
                    }
                }
            }
        };
        stopped |= matches!(&drew, Drew::Gpu(Err(_)) | Drew::Shm(_, Err(_)));
        backs.push(Back {
            of,
            surface,
            size,
            drew,
        });
    }
    if stopped {
        // none of the frame commits: what was drawn goes back unshown
        sends.clear();
        for back in &mut backs {
            match &mut back.drew {
                Drew::Gpu(drawn @ Ok(_)) => *drawn = Err(left_unshown()),
                Drew::Shm(_, drawn @ Ok(_)) => *drawn = Err(left_unshown()),
                _ => {}
            }
        }
    }
    Drawing {
        backs,
        panic,
        commits: Commits(sends),
    }
}

/// Draws `list` with its renderer and then into the GPU buffer of `draw`, with the damage `carried`.
fn draw_gpu(
    draw: Draw,
    of: Of,
    list: &LayerList,
    submission: &Submission,
    carried: &[PixelRect],
) -> Result<(RenderFrameId, Drawn), Failure> {
    let mut renderer = list.renderer.lock().unwrap_or_else(PoisonError::into_inner);
    let id = tracing::trace_span!("ui.raster", layer = of.name())
        .in_scope(|| renderer.render(&submission.layer_request(list)))
        .map_err(Failure::Render)?;
    let frame = renderer.frame(id).map_err(Failure::Render)?;
    let damage: Vec<PixelRect> = carried
        .iter()
        .copied()
        .chain(frame.damage.iter().map(wire_rect))
        .collect();
    let drawn = draw.draw(frame, &damage).map_err(Failure::Present)?;
    Ok((id, drawn))
}

/// Draws `list` into the shared-memory `buffer`, with what the buffer missed and the damage `carried`: straight into
/// it where the renderer draws into memory in the buffer's byte order, else into the renderer's own image and copied.
/// The frame's id and the damage to commit (`None` for all of it, a `resized` frame).
fn draw_shm(
    buffer: &mut DetachedBuffer,
    of: Of,
    list: &LayerList,
    submission: &Submission,
    carried: &[PixelRect],
    resized: bool,
) -> Result<(RenderFrameId, Option<Vec<PixelRect>>), Failure> {
    let layout = buffer.layout();
    let (width, height) = (layout.width(), layout.height());
    let inside = |r: &PixelRect| clip(*r, width, height);
    let mut renderer = list.renderer.lock().unwrap_or_else(PoisonError::into_inner);
    let request = submission.layer_request(list);
    let raster = tracing::trace_span!("ui.raster", layer = of.name());
    let into_memory = layout.format() == BufferFormat::Abgr8888
        && layout.stride() == width as usize * 4
        && renderer.draws_into_memory();
    if into_memory {
        // what the buffer misses: the frames shown in the others since it held one and those not shown
        let missed: Option<Vec<_>> = buffer.missed().map(|missed| {
            missed
                .iter()
                .chain(carried)
                .filter_map(inside)
                .map(render_rect)
                .collect()
        });
        let stride = layout.stride();
        let (id, drawn) = raster
            .in_scope(|| {
                renderer.render_into(
                    &request,
                    MemoryTarget {
                        pixels: buffer.pixels_mut(),
                        stride,
                        missed: missed.as_deref(),
                    },
                )
            })
            .map_err(Failure::Render)?;
        let changed = carried
            .iter()
            .copied()
            .chain(drawn.iter().map(wire_rect))
            .filter_map(|r| inside(&r))
            .collect();
        return Ok((id, (!resized).then_some(changed)));
    }
    let id = raster
        .in_scope(|| renderer.render(&request))
        .map_err(Failure::Render)?;
    let frame = renderer.frame(id).map_err(Failure::Render)?;
    // a frame drawn on a GPU is read back; one that cannot be (its device lost) is not shown, and the frames after the
    // switch back to the CPU replace it
    let frame = frame.in_memory().map_err(|_| left_unshown())?;
    if (frame.width, frame.height) != (width, height) {
        return Err(left_unshown());
    }
    let damage: Vec<PixelRect> = carried
        .iter()
        .copied()
        .chain(frame.damage.iter().map(wire_rect))
        .collect();
    let missed = buffer.missed().map(<[PixelRect]>::to_vec);
    let changed = copy_frame(
        &frame,
        buffer.pixels_mut(),
        layout,
        missed.as_deref(),
        &damage,
    );
    Ok((id, (!resized).then_some(changed)))
}

/// Copies `damage` of `frame`, and what the buffer `missed` (all of it when that is unknown), into the buffer's
/// `pixels` of `layout`, converted to its byte order; the damage inside the frame, as the compositor is told it.
pub(crate) fn copy_frame(
    frame: &CpuFrame,
    pixels: &mut [u8],
    layout: BufferLayout,
    missed: Option<&[PixelRect]>,
    damage: &[PixelRect],
) -> Vec<PixelRect> {
    let inside = |rects: &mut dyn Iterator<Item = &PixelRect>| -> Vec<PixelRect> {
        rects
            .filter_map(|r| clip(*r, frame.width, frame.height))
            .collect()
    };
    let changed = inside(&mut damage.iter());
    let copy = missed.map(|missed| disjoint(&inside(&mut changed.iter().chain(missed))));
    let whole = [PixelRect {
        x: 0,
        y: 0,
        width: frame.width,
        height: frame.height,
    }];
    let into = if layout.format() == BufferFormat::Abgr8888 {
        copy_rects
    } else {
        convert_rects
    };
    let rects = copy.as_deref().unwrap_or(&whole);
    tracing::trace_span!("wl.copy", from = "memory", rects = rects.len()).in_scope(|| {
        into(
            &frame.pixels,
            frame.stride as usize,
            pixels,
            layout.stride(),
            rects,
        )
    });
    changed
}

/// A renderer's rectangle of pixels as the transport's.
pub(crate) fn wire_rect(r: &cupertino_ui_render::PixelRect) -> PixelRect {
    PixelRect {
        x: r.x,
        y: r.y,
        width: r.width,
        height: r.height,
    }
}

/// The transport's rectangle of pixels as a renderer's.
pub(crate) fn render_rect(r: PixelRect) -> cupertino_ui_render::PixelRect {
    cupertino_ui_render::PixelRect {
        x: r.x,
        y: r.y,
        width: r.width,
        height: r.height,
    }
}

/// `r` cut to a `width` × `height` buffer; `None` when nothing is left.
pub(crate) fn clip(r: PixelRect, width: u32, height: u32) -> Option<PixelRect> {
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
