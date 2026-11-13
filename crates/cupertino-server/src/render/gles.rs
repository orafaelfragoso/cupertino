//! The GLES implementation of the renderer seam: a [`SceneFrame`] is lowered to the elements of Smithay's damage
//! tracker and drawn to one of the targets the compositor has, with the same cache slots, commit counters and draw
//! order the renderer has always used. The caller keeps what presentation needs: the winit buffer swap, the DRM queue
//! and page flip, frame callbacks and presentation feedback.
//!
//! A target carries the device it draws on, so binding a window's framebuffer or a texture happens inside the render
//! call and no borrowed framebuffer leaks into the trait.

use cupertino_render_api::{Completion, Failure, RenderFailure, RenderStatus, Renderer};
use cupertino_scene::SceneFrame;
use image::RgbaImage;
use smithay::backend::allocator::dmabuf::Dmabuf;
#[cfg(feature = "udev")]
use smithay::backend::drm::compositor::RenderFrameError;
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::gles::{GlesError, GlesRenderer, GlesTexture};
use smithay::backend::renderer::sync::SyncPoint;
use smithay::backend::renderer::{Bind, Color32F};
use smithay::backend::SwapBuffersError;
use smithay::utils::{Physical, Rectangle};

use super::compose::FrameResources;
use super::lower::{lower, Lowering};
use super::material::SharedMaterial;
use super::mesh::{capture, SharedMesh};
use super::programs::Programs;
use super::scene::{finalize_materials, RenderResources};
use super::screenshot::render_shown_image;
use super::shader_rect::ElementCache;
use super::snapshots::Snapshots;
use super::CupertinoElement;

/// The GLES renderer. It holds nothing: the device is in the target, the programs and caches in the resources.
#[derive(Default)]
pub struct Gles;

impl Gles {
    /// The elements of `scene` front to back, as the damage tracker takes them: the lowering alone, without drawing.
    pub fn lower(
        renderer: &mut GlesRenderer,
        scene: &SceneFrame,
        resources: &mut GlesResources<'_>,
    ) -> Result<Vec<CupertinoElement>, RenderError> {
        if resources
            .frame
            .as_ref()
            .is_some_and(|frame| frame.frame != scene.id)
        {
            return Err(RenderError::Resources("resources belong to another frame"));
        }
        let frame = resources.frame.take().ok_or(RenderError::Resources(
            "the frame's resources were used by an earlier render",
        ))?;
        let mut ctx = Lowering {
            renderer,
            programs: resources.programs,
            material: resources.material.clone(),
            mesh: resources.mesh.clone(),
            snapshots: resources.snapshots.clone(),
            cache: resources.cache,
            scale: scene.output.scale,
        };
        let mut elements = lower(scene, frame, &mut ctx);
        finalize_materials(&mut elements, scene.output.scale, resources.cache);
        resources.cache.end_frame();
        Ok(elements)
    }
}

/// What a frame's handles resolve against, and the caches of the output: the shader programs, the material and mesh
/// passes, the element cache that keeps ids and commit counters across frames, and the frame's own resources (taken
/// by the first render).
pub struct GlesResources<'a> {
    programs: &'a Programs,
    material: Option<SharedMaterial>,
    mesh: Option<SharedMesh>,
    snapshots: Snapshots,
    cache: &'a mut ElementCache,
    frame: Option<FrameResources>,
}

impl<'a> GlesResources<'a> {
    pub fn new(
        res: &'a RenderResources,
        cache: &'a mut ElementCache,
        frame: FrameResources,
    ) -> Self {
        Self {
            programs: &res.programs,
            material: res.material.clone(),
            mesh: res.mesh.clone(),
            snapshots: res.snapshots.clone(),
            cache,
            frame: Some(frame),
        }
    }
}

/// Where a frame is drawn, with the device it is drawn on.
pub enum GlesTarget<'a> {
    /// An offscreen texture (of the headless backend, or that another card's display copies), drawn through the
    /// damage tracker at the texture's `age`.
    Texture {
        renderer: &'a mut GlesRenderer,
        texture: &'a mut GlesTexture,
        tracker: &'a mut OutputDamageTracker,
        age: usize,
        clear: Color32F,
    },
    /// A buffer of the render GPU that another card copies onto its display, drawn through the copy's damage tracker at
    /// the buffer's `age`.
    Dmabuf {
        renderer: &'a mut GlesRenderer,
        dmabuf: &'a mut Dmabuf,
        tracker: &'a mut OutputDamageTracker,
        age: usize,
        clear: Color32F,
    },
    /// The window of the nested backend, drawn through the output's damage tracker at the buffer's `age`.
    #[cfg(feature = "winit")]
    Window {
        graphics: &'a mut smithay::backend::winit::WinitGraphicsBackend<GlesRenderer>,
        tracker: &'a mut OutputDamageTracker,
        age: usize,
        clear: Color32F,
    },
    /// A display output of the DRM compositor, which draws, tracks damage and chooses planes itself.
    #[cfg(feature = "udev")]
    Drm {
        renderer: &'a mut GlesRenderer,
        output: &'a mut crate::backend::udev::DrmSurfaceOutput,
        clear: Color32F,
    },
    /// A fresh offscreen image of the output as it is shown, upright at the output's device size (a quarter-turned
    /// output's mode on its side), read back as RGBA: what screenshots and screen captures hand out.
    Image {
        renderer: &'a mut GlesRenderer,
        clear: Color32F,
    },
    /// A fresh offscreen texture of the frame's output size, cleared transparent: a window snapshot.
    Snapshot { renderer: &'a mut GlesRenderer },
}

/// What a render produced.
pub enum GlesCompletion {
    /// Drawn through a damage tracker: the damage to present, `None` when nothing differs from the buffer.
    Drawn {
        damage: Option<Vec<Rectangle<i32, Physical>>>,
    },
    /// Drawn into a buffer another device reads once `sync` is signalled: the damage, `None` when nothing differs.
    Offscreen {
        damage: Option<Vec<Rectangle<i32, Physical>>>,
        sync: SyncPoint,
    },
    /// Drawn by the DRM compositor, which keeps the frame for the queue: `empty` when nothing differs; `planes`, the
    /// planes it put elements on.
    Direct {
        empty: bool,
        planes: crate::drm_trace::Planes,
    },
    Image(RgbaImage),
    Snapshot(GlesTexture),
}

impl GlesCompletion {
    /// The damage a presenter submits, when a tracker drew something.
    pub fn damage(&self) -> Option<&[Rectangle<i32, Physical>]> {
        match self {
            Self::Drawn { damage } | Self::Offscreen { damage, .. } => damage.as_deref(),
            _ => None,
        }
    }
}

impl Completion for GlesCompletion {
    fn status(&self) -> RenderStatus {
        match self {
            Self::Drawn { damage: None }
            | Self::Offscreen { damage: None, .. }
            | Self::Direct { empty: true, .. } => RenderStatus::Empty,
            Self::Drawn { damage: Some(_) }
            | Self::Offscreen {
                damage: Some(_), ..
            }
            | Self::Direct { empty: false, .. }
            | Self::Image(_)
            | Self::Snapshot(_) => RenderStatus::Rendered,
        }
    }
}

#[derive(Debug)]
pub enum RenderError {
    /// Drawing or preparing the frame on the device failed, as Smithay classifies it: lost context or temporary.
    Rendering(SwapBuffersError),
    /// Binding, reading back or another step of the target failed.
    Target(String),
    Resources(&'static str),
}

impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rendering(e) => write!(f, "rendering failed: {e}"),
            Self::Target(e) => write!(f, "the target failed: {e}"),
            Self::Resources(e) => f.write_str(e),
        }
    }
}

impl std::error::Error for RenderError {}

impl RenderFailure for RenderError {
    fn failure(&self) -> Failure {
        match self {
            Self::Rendering(SwapBuffersError::ContextLost(_)) => Failure::ContextLost,
            _ => Failure::Transient,
        }
    }
}

fn tracked(e: smithay::backend::renderer::damage::Error<GlesError>) -> RenderError {
    match e {
        smithay::backend::renderer::damage::Error::Rendering(e) => RenderError::Rendering(e.into()),
        other => RenderError::Target(format!("{other:?}")),
    }
}

/// The output transform as Smithay draws it.
fn output_transform(t: cupertino_scene::Transform) -> smithay::utils::Transform {
    use cupertino_scene::Transform as T;
    use smithay::utils::Transform as S;
    match t {
        T::Normal => S::Normal,
        T::Rotate90 => S::_90,
        T::Rotate180 => S::_180,
        T::Rotate270 => S::_270,
        T::Flipped => S::Flipped,
        T::Flipped90 => S::Flipped90,
        T::Flipped180 => S::Flipped180,
        T::Flipped270 => S::Flipped270,
    }
}

impl Renderer for Gles {
    type Resources<'a> = GlesResources<'a>;
    type Target<'a> = GlesTarget<'a>;
    type Completion = GlesCompletion;
    type Error = RenderError;

    fn render(
        &mut self,
        scene: &SceneFrame,
        resources: &mut GlesResources<'_>,
        target: GlesTarget<'_>,
    ) -> Result<GlesCompletion, RenderError> {
        let size = (
            scene.output.physical_size.width,
            scene.output.physical_size.height,
        )
            .into();
        let scale = scene.output.scale;
        match target {
            GlesTarget::Texture {
                renderer,
                texture,
                tracker,
                age,
                clear,
            } => {
                let elements = Gles::lower(renderer, scene, resources)?;
                let mut framebuffer = renderer
                    .bind(texture)
                    .map_err(|e| RenderError::Target(e.to_string()))?;
                tracker
                    .render_output(renderer, &mut framebuffer, age, &elements, clear)
                    .map(|r| GlesCompletion::Drawn {
                        damage: r.damage.cloned(),
                    })
                    .map_err(tracked)
            }
            GlesTarget::Dmabuf {
                renderer,
                dmabuf,
                tracker,
                age,
                clear,
            } => {
                let elements = Gles::lower(renderer, scene, resources)?;
                let mut framebuffer = renderer
                    .bind(dmabuf)
                    .map_err(|e| RenderError::Target(e.to_string()))?;
                tracker
                    .render_output(renderer, &mut framebuffer, age, &elements, clear)
                    .map(|r| GlesCompletion::Offscreen {
                        damage: r.damage.cloned(),
                        sync: r.sync,
                    })
                    .map_err(tracked)
            }
            #[cfg(feature = "winit")]
            GlesTarget::Window {
                graphics,
                tracker,
                age,
                clear,
            } => {
                let (renderer, mut framebuffer) = graphics
                    .bind()
                    .map_err(|e| RenderError::Target(e.to_string()))?;
                let elements = Gles::lower(renderer, scene, resources)?;
                tracker
                    .render_output(renderer, &mut framebuffer, age, &elements, clear)
                    .map(|r| GlesCompletion::Drawn {
                        damage: r.damage.cloned(),
                    })
                    .map_err(tracked)
            }
            #[cfg(feature = "udev")]
            GlesTarget::Drm {
                renderer,
                output,
                clear,
            } => {
                let elements = Gles::lower(renderer, scene, resources)?;
                output
                    .render_frame(
                        renderer,
                        &elements,
                        clear,
                        smithay::backend::drm::compositor::FrameFlags::DEFAULT,
                    )
                    .map(|r| GlesCompletion::Direct {
                        empty: r.is_empty,
                        planes: crate::drm_trace::Planes {
                            direct_scanout: matches!(
                                r.primary_element,
                                smithay::backend::drm::compositor::PrimaryPlaneElement::Element(_)
                            ),
                            cursor_plane: r.cursor_element.is_some(),
                            overlays: r.overlay_elements.len(),
                        },
                    })
                    .map_err(|e| match e {
                        RenderFrameError::PrepareFrame(e) => RenderError::Rendering(e.into()),
                        RenderFrameError::RenderFrame(e) => tracked(e),
                    })
            }
            GlesTarget::Image { renderer, clear } => {
                let elements = Gles::lower(renderer, scene, resources)?;
                render_shown_image(
                    renderer,
                    &elements,
                    size,
                    scale,
                    output_transform(scene.output.transform),
                    clear,
                )
                .map(GlesCompletion::Image)
                .map_err(|e| RenderError::Target(format!("{e:#}")))
            }
            GlesTarget::Snapshot { renderer } => {
                let elements = Gles::lower(renderer, scene, resources)?;
                capture(renderer, &elements, size, scale)
                    .map(GlesCompletion::Snapshot)
                    .map_err(|e| RenderError::Target(format!("{e:#}")))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_completion_says_whether_there_is_anything_to_present() {
        let drawn = |damage| GlesCompletion::Drawn { damage };
        assert_eq!(drawn(None).status(), RenderStatus::Empty);
        assert_eq!(
            drawn(Some(vec![Rectangle::from_size((4, 4).into())])).status(),
            RenderStatus::Rendered
        );
        assert_eq!(
            GlesCompletion::Direct {
                empty: true,
                planes: Default::default()
            }
            .status(),
            RenderStatus::Empty
        );
        assert_eq!(
            GlesCompletion::Direct {
                empty: false,
                planes: Default::default()
            }
            .status(),
            RenderStatus::Rendered
        );
        assert_eq!(
            GlesCompletion::Image(RgbaImage::new(1, 1)).status(),
            RenderStatus::Rendered
        );
    }

    #[test]
    fn only_a_tracker_that_drew_hands_out_damage_to_submit() {
        let damage = vec![Rectangle::from_size((4, 4).into())];
        assert_eq!(
            GlesCompletion::Drawn {
                damage: Some(damage.clone())
            }
            .damage(),
            Some(damage.as_slice())
        );
        assert_eq!(GlesCompletion::Drawn { damage: None }.damage(), None);
        let direct = GlesCompletion::Direct {
            empty: false,
            planes: Default::default(),
        };
        assert_eq!(direct.damage(), None);
    }

    #[test]
    fn a_failed_render_says_why_and_is_not_an_empty_success() {
        let failed: Result<GlesCompletion, RenderError> =
            Err(RenderError::Target("no framebuffer".into()));
        assert!(failed.is_err());
        let text = failed.err().unwrap().to_string();
        assert!(text.contains("no framebuffer"), "{text}");
        assert_eq!(
            RenderError::Resources("used twice").to_string(),
            "used twice"
        );
    }

    #[test]
    fn only_a_lost_context_stops_rendering() {
        let rendering = |e: GlesError| RenderError::Rendering(e.into()).failure();
        let lost = SwapBuffersError::ContextLost(Box::new(std::io::Error::other("reset")));
        assert_eq!(RenderError::Rendering(lost).failure(), Failure::ContextLost);
        assert_eq!(
            rendering(GlesError::ShaderCompileError),
            Failure::ContextLost
        );
        assert_eq!(rendering(GlesError::BlitError), Failure::Transient);
        assert_eq!(
            RenderError::Target("no framebuffer".into()).failure(),
            Failure::Transient
        );
        assert_eq!(
            RenderError::Resources("used twice").failure(),
            Failure::Transient
        );
    }
}
