//! Drawing an output at a fractional scale at the next whole one: the scene is composed at the render scale into an
//! offscreen frame of the output's own, drawn through a damage tracker of its own so only what changed is drawn again,
//! and one element ([`ResampledElement`]) shows that frame shrunk to the display's pixels with the area filter
//! (`cupertino_raster_math::resample`), dithered at the display's depth. What changed in the frame is that element's
//! damage, mapped to the display's pixels and widened by the filter's footprint.

use cupertino_defaults::compositor::FractionalScaleMode;
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::element::{Element, Id, Kind, RenderElement, UnderlyingStorage};
use smithay::backend::renderer::gles::{
    GlesError, GlesFrame, GlesRenderer, GlesTexProgram, GlesTexture, Uniform, UniformName,
    UniformType,
};
use smithay::backend::renderer::utils::{CommitCounter, DamageBag, DamageSet, OpaqueRegions};
use smithay::backend::renderer::{Bind, Color32F, Offscreen};
use smithay::utils::{Buffer, Physical, Point, Rectangle, Scale, Size, Transform};

use super::programs::u2;

const RESAMPLE: &str = include_str!("../../shaders/compositor/resample.frag");

/// How many frames of damage the final element keeps: a buffer older than that is drawn whole.
const DAMAGE_KEPT: usize = 8;

/// Display pixels the final element's damage is widened by on each side, beyond what the frame's damage covers of the
/// display: the filter reads the texels a pixel covers, and the linear lookups may touch their neighbours' edge.
pub const FOOTPRINT_PAD: i32 = 1;

/// Whether `scale` is no whole number, in the 120ths the fractional scale protocol speaks.
pub fn is_fractional(scale: f64) -> bool {
    let n = (scale * 120.0).round();
    n.is_finite() && n > 0.0 && n % 120.0 != 0.0
}

/// The scale an output at `scale` is drawn at in `mode`: the next whole number above a fractional scale when
/// supersampled, else the output's own.
pub fn render_scale(scale: f64, mode: FractionalScaleMode) -> f64 {
    match mode {
        FractionalScaleMode::Supersample if is_fractional(scale) => {
            ((scale * 120.0).round() / 120.0).ceil().max(1.0)
        }
        _ => scale,
    }
}

/// The size in pixels of the frame an output of `mode_px` pixels at `scale` is drawn into at `render`: its points
/// times the render scale, rounded.
pub fn render_size(mode_px: (i32, i32), scale: f64, render: f64) -> (i32, i32) {
    let k = render / scale;
    (
        ((f64::from(mode_px.0) * k).round() as i32).max(1),
        ((f64::from(mode_px.1) * k).round() as i32).max(1),
    )
}

/// The display pixels (of `panel`) that `damage` (pixels of a frame of `frame`) reaches through the filter: each
/// rectangle scaled, rounded outwards, widened by [`FOOTPRINT_PAD`] and held to the display.
pub fn panel_damage(
    damage: &[Rectangle<i32, Physical>],
    frame: (i32, i32),
    panel: (i32, i32),
) -> Vec<Rectangle<i32, Physical>> {
    let kx = f64::from(panel.0) / f64::from(frame.0.max(1));
    let ky = f64::from(panel.1) / f64::from(frame.1.max(1));
    let bounds = Rectangle::<i32, Physical>::from_size(Size::from(panel));
    damage
        .iter()
        .filter_map(|r| {
            let x0 = (f64::from(r.loc.x) * kx).floor() as i32 - FOOTPRINT_PAD;
            let y0 = (f64::from(r.loc.y) * ky).floor() as i32 - FOOTPRINT_PAD;
            let x1 = (f64::from(r.loc.x + r.size.w) * kx).ceil() as i32 + FOOTPRINT_PAD;
            let y1 = (f64::from(r.loc.y + r.size.h) * ky).ceil() as i32 + FOOTPRINT_PAD;
            Rectangle::new(Point::from((x0, y0)), Size::from((x1 - x0, y1 - y0)))
                .intersection(bounds)
        })
        .filter(|r| !r.is_empty())
        .collect()
}

/// The program of the final pass, compiled through the renderer's texture shader interface.
pub fn compile(renderer: &mut GlesRenderer) -> Result<GlesTexProgram, GlesError> {
    let src = format!(
        "#version 100\n{}",
        RESAMPLE
            .replace("//_TAPS_", cupertino_raster_math::resample::GLSL_TAPS)
            .replace("//_DITHER_", cupertino_raster_math::dither::GLSL)
    );
    renderer.compile_custom_texture_shader(
        src,
        &[
            UniformName::new("texels", UniformType::_2f),
            UniformName::new("footprint", UniformType::_2f),
            UniformName::new("step", UniformType::_1f),
        ],
    )
}

/// The offscreen frame of an output and what it was made for.
struct Frame {
    texture: GlesTexture,
    tracker: OutputDamageTracker,
    size: (i32, i32),
    scale: f64,
    /// Frames drawn into it: its content is that of the last after the first.
    drawn: bool,
}

/// What an output drawn at its render scale keeps from one frame to the next: the offscreen frame, and the identity
/// and damage of the element that shows it.
pub struct Supersample {
    frame: Option<Frame>,
    id: Id,
    /// The final element's damage, in display pixels.
    damage: DamageBag<i32, Physical>,
    /// Bits a channel of the display's frame, the final pass's dither step.
    pub depth: u8,
    /// Dither the final pass ([`super::programs::dithering`]).
    pub dither: bool,
}

impl std::fmt::Debug for Supersample {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Supersample")
            .field("frame", &self.frame.as_ref().map(|f| (f.size, f.scale)))
            .field("depth", &self.depth)
            .finish()
    }
}

impl Default for Supersample {
    fn default() -> Self {
        Self {
            frame: None,
            id: Id::new(),
            damage: DamageBag::new(DAMAGE_KEPT),
            depth: 8,
            dither: super::programs::dithering(),
        }
    }
}

/// The formats the offscreen frame is made in, the first the device can draw into: 10 bits a channel, at least what
/// any display gets, else 8.
const FRAME_FORMATS: [Fourcc; 2] = [Fourcc::Abgr2101010, Fourcc::Abgr8888];

impl Supersample {
    /// The offscreen frame's size and format, once one is made.
    pub fn frame_size(&self) -> Option<(i32, i32)> {
        self.frame.as_ref().map(|f| f.size)
    }

    /// A new state for an image of the output (a screenshot, a capture), which is 8 bits a channel, dithered as the
    /// output's frames are.
    pub fn for_image(&self) -> Supersample {
        Supersample {
            depth: 8,
            dither: self.dither,
            ..Supersample::default()
        }
    }

    /// Forget the offscreen frame (a lost context): the next frame makes a new one and is drawn whole.
    pub fn reset(&mut self) {
        self.frame = None;
        self.damage.reset();
    }

    /// Draw `elements` (front to back, at `scale`) into the offscreen frame of `size` pixels, only what changed since
    /// the last frame, and return the element that shows it at `panel` pixels (the display's, upright) through
    /// `program`. The element's damage is what the frame's reaches of the display.
    #[allow(clippy::too_many_arguments)]
    pub fn draw<E: RenderElement<GlesRenderer>>(
        &mut self,
        renderer: &mut GlesRenderer,
        program: &GlesTexProgram,
        elements: &[E],
        size: (i32, i32),
        scale: f64,
        panel: (i32, i32),
        clear: Color32F,
    ) -> Result<ResampledElement, String> {
        let _supersample = tracing::trace_span!(
            "server.supersample",
            width = size.0,
            height = size.1,
            elements = elements.len()
        )
        .entered();
        let fits = self
            .frame
            .as_ref()
            .is_some_and(|f| f.size == size && f.scale == scale);
        if !fits {
            let buffer: Size<i32, Buffer> = Size::from(size);
            let texture = FRAME_FORMATS
                .iter()
                .find_map(|format| renderer.create_buffer(*format, buffer).ok())
                .ok_or_else(|| format!("no offscreen frame of {size:?} pixels"))?;
            self.frame = Some(Frame {
                texture,
                tracker: OutputDamageTracker::new(size, scale, Transform::Normal),
                size,
                scale,
                drawn: false,
            });
            self.damage.reset();
        }
        let frame = self.frame.as_mut().expect("made above");
        let age = usize::from(frame.drawn);
        let damage = {
            let mut target = renderer
                .bind(&mut frame.texture)
                .map_err(|e| format!("binding the offscreen frame: {e}"))?;
            frame
                .tracker
                .render_output(renderer, &mut target, age, elements, clear)
                .map_err(|e| format!("drawing the offscreen frame: {e:?}"))?
                .damage
                .cloned()
        };
        frame.drawn = true;
        if let Some(damage) = damage {
            self.damage.add(panel_damage(&damage, size, panel));
        }
        let step = match (self.dither, self.depth) {
            (false, _) => 0.0,
            (true, 10) => 1.0 / 1023.0,
            (true, _) => 1.0 / 255.0,
        };
        Ok(ResampledElement {
            id: self.id.clone(),
            commit: self.damage.current_commit(),
            damage: self.damage.snapshot(),
            texture: frame.texture.clone(),
            frame: size,
            panel,
            program: program.clone(),
            step,
        })
    }
}

/// The offscreen frame of an output shown over the whole output at the display's pixels, shrunk with the area filter.
pub struct ResampledElement {
    id: Id,
    commit: CommitCounter,
    damage: smithay::backend::renderer::utils::DamageSnapshot<i32, Physical>,
    texture: GlesTexture,
    /// The frame's size, texels.
    frame: (i32, i32),
    /// The display's size, upright, pixels.
    panel: (i32, i32),
    program: GlesTexProgram,
    step: f32,
}

impl std::fmt::Debug for ResampledElement {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResampledElement")
            .field("frame", &self.frame)
            .field("panel", &self.panel)
            .finish()
    }
}

impl ResampledElement {
    /// The uniforms of the final pass.
    pub fn uniforms(&self) -> [Uniform<'static>; 3] {
        [
            u2("texels", self.frame.0 as f32, self.frame.1 as f32),
            u2(
                "footprint",
                self.frame.0 as f32 / self.panel.0.max(1) as f32,
                self.frame.1 as f32 / self.panel.1.max(1) as f32,
            ),
            Uniform::new("step", self.step),
        ]
    }
}

impl Element for ResampledElement {
    fn id(&self) -> &Id {
        &self.id
    }

    fn current_commit(&self) -> CommitCounter {
        self.commit
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        Rectangle::from_size((f64::from(self.frame.0), f64::from(self.frame.1)).into())
    }

    fn geometry(&self, _scale: Scale<f64>) -> Rectangle<i32, Physical> {
        Rectangle::from_size(Size::from(self.panel))
    }

    fn damage_since(
        &self,
        scale: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> DamageSet<i32, Physical> {
        self.damage
            .damage_since(commit)
            .unwrap_or_else(|| DamageSet::from_slice(&[self.geometry(scale)]))
    }

    fn opaque_regions(&self, scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        OpaqueRegions::from_slice(&[self.geometry(scale)])
    }

    fn kind(&self) -> Kind {
        Kind::Unspecified
    }
}

impl RenderElement<GlesRenderer> for ResampledElement {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), GlesError> {
        let _shrink = tracing::trace_span!("server.shrink").entered();
        frame.render_texture_from_to(
            &self.texture,
            src,
            dst,
            damage,
            opaque_regions,
            Transform::Normal,
            1.0,
            Some(&self.program),
            &self.uniforms(),
        )
    }

    fn underlying_storage(&self, _renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        None
    }
}
