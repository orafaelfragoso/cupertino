//! The OpenGL ES 3.0 backend over EGL.
//!
//! Binding points: the uniform block at group `g`, binding `b` is bound to uniform buffer point `g * 8 + b`, the
//! texture there to texture unit `g * 8 + b`; groups 0 and 1, bindings 0 to 7. Blocks and samplers of a program are
//! found by the names its generated bindings list, since GLSL ES 3.00 has no binding qualifiers.

mod egl;
mod state;

use std::cell::Cell;
use std::ffi::c_void;
use std::os::fd::{AsRawFd, OwnedFd};
use std::sync::atomic::{AtomicU64, Ordering};

use glow::HasContext;

use crate::program_cache::{driver_key, program_key, ProgramCache, ProgramCacheStats};
use crate::shaders::{NamedBinding, ShaderSource};
use crate::types::*;

pub use state::BorrowedState;
use state::Touched;

/// Groups a pipeline may have.
pub const MAX_GROUPS: u32 = 2;
/// Bindings a group may have.
pub const MAX_BINDINGS: u32 = 8;

const GL_GUILTY_CONTEXT_RESET: u32 = 0x8253;
const GL_INNOCENT_CONTEXT_RESET: u32 = 0x8254;
const GL_UNKNOWN_CONTEXT_RESET: u32 = 0x8255;
/// `GL_PURGED_CONTEXT_RESET_NV`: the driver lost the video memory.
const GL_PURGED_CONTEXT_RESET_NV: u32 = 0x92BB;

type ImageTargetTexture = unsafe extern "C" fn(u32, *mut c_void);
type ResetStatus = unsafe extern "C" fn() -> u32;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

/// The reset status a driver reports, as a loss.
fn loss_of(status: u32) -> Option<Loss> {
    match status {
        0 => None,
        GL_GUILTY_CONTEXT_RESET => Some(Loss::Reset { guilty: Some(true) }),
        GL_INNOCENT_CONTEXT_RESET => Some(Loss::Reset {
            guilty: Some(false),
        }),
        GL_PURGED_CONTEXT_RESET_NV => Some(Loss::Purge),
        // GL_UNKNOWN_CONTEXT_RESET and anything else
        _ => Some(Loss::Reset { guilty: None }),
    }
}

fn status_of(loss: Loss) -> u32 {
    match loss {
        Loss::Reset { guilty: Some(true) } => GL_GUILTY_CONTEXT_RESET,
        Loss::Reset {
            guilty: Some(false),
        } => GL_INNOCENT_CONTEXT_RESET,
        Loss::Reset { guilty: None } => GL_UNKNOWN_CONTEXT_RESET,
        Loss::Purge => GL_PURGED_CONTEXT_RESET_NV,
    }
}

/// A GPU buffer.
#[derive(Debug)]
pub struct Buffer {
    raw: glow::Buffer,
    size: u64,
    usage: BufferUsage,
}

impl Buffer {
    pub fn size(&self) -> u64 {
        self.size
    }
}

/// A texture: sampled, a render target, or both; its memory the device's own or a dmabuf.
#[derive(Debug)]
pub struct Texture {
    raw: glow::Texture,
    desc: TextureDesc,
    framebuffer: Option<glow::Framebuffer>,
    image: Option<egl::EGLImage>,
    bo: Option<*mut c_void>,
    id: u64,
    /// Its storage is the device's owner's ([`Device::wrap_texture`]): destroying it keeps the texture.
    foreign: bool,
    /// The bytes of storage the device gave it when it was made, in [`Device::live_textures`].
    counted: u64,
}

impl Texture {
    pub fn desc(&self) -> TextureDesc {
        self.desc
    }

    pub fn size(&self) -> (u32, u32) {
        self.desc.size
    }

    /// An identity unique in the process, for caches and tests.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Whether its memory is a dmabuf.
    pub fn is_external(&self) -> bool {
        self.image.is_some()
    }
}

/// A texture whose memory is a dmabuf the device allocated, and that dmabuf.
#[derive(Debug)]
pub struct ExternalImage {
    pub texture: Texture,
    pub dmabuf: Dmabuf,
}

/// How textures are filtered and read past their edges.
#[derive(Debug)]
pub struct Sampler {
    raw: glow::Sampler,
}

/// The bindings a group of a pipeline has.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BindGroupLayout {
    entries: Vec<BindingLayout>,
}

#[derive(Clone, Copy, Debug)]
enum Bound {
    Buffer {
        raw: glow::Buffer,
        offset: u64,
        size: u64,
    },
    Texture(glow::Texture),
    Sampler(glow::Sampler),
}

/// The resources one group of bindings names.
#[derive(Debug)]
pub struct BindGroup {
    entries: Vec<(u32, Bound)>,
}

/// One resource of a bind group.
#[derive(Clone, Copy, Debug)]
pub enum BindingResource<'a> {
    Buffer {
        buffer: &'a Buffer,
        offset: u64,
        size: u64,
    },
    Texture(&'a Texture),
    Sampler(&'a Sampler),
}

/// One entry of a bind group: its binding and resource.
#[derive(Clone, Copy, Debug)]
pub struct BindGroupEntry<'a> {
    pub binding: u32,
    pub resource: BindingResource<'a>,
}

/// A pipeline to make: a program, the layouts of its groups, its vertex buffers, blending and topology.
#[derive(Clone, Debug)]
pub struct PipelineDesc<'a> {
    pub shader: &'a ShaderSource,
    pub layouts: &'a [&'a BindGroupLayout],
    pub vertex_buffers: &'a [VertexLayout],
    pub blend: Blend,
    pub topology: Topology,
}

/// A texture a program reads: its group and binding, and its sampler's (group, binding), if it has one.
type TextureBinding = (u32, u32, Option<(u32, u32)>);

/// An immutable pipeline: program, layouts, vertex input, blending, topology.
#[derive(Debug)]
pub struct Pipeline {
    program: glow::Program,
    layouts: Vec<BindGroupLayout>,
    vertex_buffers: Vec<VertexLayout>,
    blend: Blend,
    topology: Topology,
    /// Every texture the program reads, with its sampler.
    textures: Vec<TextureBinding>,
}

/// Work submitted so far, finished when the fence signals.
#[derive(Debug)]
pub struct Fence {
    raw: glow::Fence,
}

/// The device: a GLES 3.0 context (owned or borrowed) and what it can do.
pub struct Device {
    gl: glow::Context,
    // dropped after `gl`'s last use: the context's libraries
    ctx: egl::Context,
    image_target_texture: Option<ImageTargetTexture>,
    reset_status: Option<ResetStatus>,
    info: DeviceInfo,
    vao: glow::VertexArray,
    lost: Cell<Option<Loss>>,
    injected: Cell<Option<u32>>,
    borrowed: bool,
    entered: Cell<bool>,
    /// The groups of the owner's state the device changed since it was entered or resumed (borrowed devices).
    touched: Cell<Touched>,
    /// The owner's draw framebuffer and viewport, as entering read them (borrowed devices).
    owner_target: Cell<(Option<glow::Framebuffer>, [i32; 4])>,
    /// Saved program binaries, when the caller named a cache directory.
    program_cache: Option<ProgramCache>,
    /// The driver's vendor, renderer and version strings, which a saved binary is valid for.
    driver_key: String,
    /// The textures made and not yet destroyed.
    live: Cell<TextureTally>,
}

/// The textures a device made and has not destroyed, and the bytes of the storage it gave them: a texture's own
/// storage, or the buffer object of a dmabuf it allocated; a dmabuf imported from elsewhere and an owner's texture
/// wrapped are counted with no bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TextureTally {
    pub textures: u32,
    pub bytes: u64,
}

impl Texture {
    /// The bytes of storage the device gives it as it is made.
    fn own_bytes(&self) -> u64 {
        if self.foreign || (self.image.is_some() && self.bo.is_none()) {
            return 0;
        }
        let (w, h) = self.desc.size;
        u64::from(w) * u64::from(h) * u64::from(self.desc.format.bytes_per_pixel())
    }
}

impl std::fmt::Debug for Device {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Device").field("info", &self.info).finish()
    }
}

fn binding_index(group: u32, binding: u32) -> Result<u32> {
    if group >= MAX_GROUPS || binding >= MAX_BINDINGS {
        return Err(Error::Invalid(format!(
            "group {group} binding {binding}: groups 0 to {} and bindings 0 to {} only",
            MAX_GROUPS - 1,
            MAX_BINDINGS - 1
        )));
    }
    Ok(group * MAX_BINDINGS + binding)
}

/// Texture units and uniform buffer points the device may touch (and a borrowed device restores).
pub(crate) const UNITS: u32 = MAX_GROUPS * MAX_BINDINGS;

fn software_renderer(name: &str) -> bool {
    let r = name.to_ascii_lowercase();
    ["llvmpipe", "softpipe", "swrast", "lavapipe"]
        .iter()
        .any(|s| r.contains(s))
}

impl Device {
    /// A device with a context of its own, current on this thread.
    pub fn new(desc: &DeviceDesc) -> Result<Device> {
        let node = match &desc.platform {
            Platform::RenderNode(path) => Some(
                std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(path)
                    .map(OwnedFd::from)
                    .map_err(|e| Error::Unavailable(format!("{}: {e}", path.display())))?,
            ),
            Platform::Surfaceless => None,
        };
        let ctx = egl::Context::new(node)?;
        Self::with_context(ctx, false)
    }

    /// A device over the GLES context current on this thread, made by someone else (the compositor's), with the
    /// robustness and purge attributes `robust` and `purge` it was made with.
    ///
    /// Every call on it happens between [`Device::enter`] and [`Device::leave`] while that context is current:
    /// `enter` saves the state the device may change ([`BorrowedState`]) and `leave` puts it back, so the owner's
    /// own drawing sees its state as it left it.
    ///
    /// # Safety
    ///
    /// The context must stay alive as long as the device, and be current on this thread whenever the device is
    /// used or dropped.
    pub unsafe fn borrow(robust: bool, purge: bool) -> Result<Device> {
        let ctx = egl::Context::borrow_current(robust, purge)?;
        let device = Self::with_context(ctx, true)?;
        Ok(device)
    }

    fn with_context(ctx: egl::Context, borrowed: bool) -> Result<Device> {
        // Safety: the context is current on this thread; glow reads entry points through EGL.
        let gl = unsafe {
            glow::Context::from_loader_function_cstr(|name| {
                ctx.proc_address(&name.to_string_lossy())
                    .map_or(std::ptr::null(), |p| p as *const c_void)
            })
        };
        // Safety (transmutes): the entry points have the signatures their extensions give.
        let image_target_texture: Option<ImageTargetTexture> = ctx
            .proc_address("glEGLImageTargetTexture2DOES")
            .map(|p| unsafe { std::mem::transmute(p) });
        let has = |e: &str| gl.supported_extensions().contains(e);
        let robust_gl = has("GL_KHR_robustness") || has("GL_EXT_robustness");
        let reset_status: Option<ResetStatus> = if robust_gl {
            [
                "glGetGraphicsResetStatusKHR",
                "glGetGraphicsResetStatusEXT",
                "glGetGraphicsResetStatus",
            ]
            .iter()
            .find_map(|n| ctx.proc_address(n))
            .map(|p| unsafe { std::mem::transmute(p) })
        } else {
            None
        };
        // Safety: a current context; plain queries.
        let (renderer, max_texture_size, vendor, version) = unsafe {
            (
                gl.get_parameter_string(glow::RENDERER),
                gl.get_parameter_i32(glow::MAX_TEXTURE_SIZE) as u32,
                gl.get_parameter_string(glow::VENDOR),
                gl.get_parameter_string(glow::VERSION),
            )
        };
        let driver = driver_key(&vendor, &renderer, &version);
        let float_targets = has("GL_EXT_color_buffer_float");
        let allocator = if ctx.has_gbm() {
            Some(Allocator::Gbm)
        } else if ctx.can_export() {
            Some(Allocator::Export)
        } else {
            None
        };
        let info = DeviceInfo {
            software: software_renderer(&renderer),
            renderer,
            allocator,
            native_fences: ctx.native_fences(),
            fence_import: ctx.fence_import(),
            robustness: ctx.robust && reset_status.is_some(),
            purge_notification: ctx.purge && reset_status.is_some(),
            half_float_targets: float_targets || has("GL_EXT_color_buffer_half_float"),
            float_targets,
            max_texture_size,
        };
        // Safety: a current context.
        let vao = unsafe { gl.create_vertex_array() }.map_err(Error::Driver)?;
        let device = Device {
            gl,
            ctx,
            image_target_texture,
            reset_status,
            info,
            vao,
            lost: Cell::new(None),
            injected: Cell::new(None),
            borrowed,
            entered: Cell::new(false),
            touched: Cell::new(Touched::NONE),
            owner_target: Cell::new((None, [0; 4])),
            program_cache: None,
            driver_key: driver,
            live: Cell::new(TextureTally::default()),
        };
        if !borrowed {
            // Safety: our own context: its state is ours to set once.
            unsafe { device.gl.bind_vertex_array(Some(device.vao)) };
        }
        Ok(device)
    }

    pub fn info(&self) -> &DeviceInfo {
        &self.info
    }

    /// Whether the device draws inside someone else's context.
    pub fn is_borrowed(&self) -> bool {
        self.borrowed
    }

    /// Save linked programs under `base` (one directory per driver, [`crate::program_cache`]) and load them from
    /// there instead of compiling, for the pipelines made from now on; `None` compiles every program. A driver with
    /// no binary formats compiles every program whatever this says.
    pub fn set_program_cache(&mut self, base: Option<&std::path::Path>) {
        // Safety: a plain query on a current context (a lost device answers 0 and compiles).
        let formats = self.ready().map_or(0, |()| unsafe {
            self.gl.get_parameter_i32(glow::NUM_PROGRAM_BINARY_FORMATS)
        });
        self.program_cache = base
            .filter(|_| formats > 0)
            .map(|base| ProgramCache::new(base, &self.driver_key));
    }

    /// The directory programs are saved in, for this device's driver, while there is a cache.
    pub fn program_cache_dir(&self) -> Option<&std::path::Path> {
        self.program_cache.as_ref().map(ProgramCache::dir)
    }

    /// The textures made and not yet destroyed, and their storage's bytes.
    pub fn live_textures(&self) -> TextureTally {
        self.live.get()
    }

    fn count_made(&self, texture: &mut Texture) {
        texture.counted = texture.own_bytes();
        let t = self.live.get();
        self.live.set(TextureTally {
            textures: t.textures + 1,
            bytes: t.bytes + texture.counted,
        });
    }

    /// What the program cache did since it was set; all zero without one.
    pub fn program_cache_stats(&self) -> ProgramCacheStats {
        self.program_cache
            .as_ref()
            .map_or_else(ProgramCacheStats::default, |c| c.stats.get())
    }

    /// The device released from this thread, to be used on another ([`DetachedDevice::attach`]): a context of its
    /// own is made current on no thread, so another may make it current. A borrowed device cannot be detached.
    pub fn detach(self) -> Result<DetachedDevice> {
        if self.borrowed {
            return Err(Error::Invalid(
                "a borrowed device stays on its owner's thread".into(),
            ));
        }
        if self.ctx.is_current() {
            // Safety: a current context; everything submitted reaches the driver before it is released.
            unsafe { self.gl.flush() };
            self.ctx.release_current()?;
        }
        Ok(DetachedDevice(self))
    }

    /// Save the owner's state and make the device's own current (borrowed devices; owned ones need nothing).
    pub fn enter(&self) -> Result<BorrowedState> {
        if !self.borrowed {
            return Err(Error::Invalid("enter is for borrowed devices".into()));
        }
        if self.entered.get() {
            return Err(Error::Invalid("the device is entered already".into()));
        }
        let saved = self.capture_state();
        self.enter_with(&saved);
        Ok(saved)
    }

    /// Put back the state [`Device::enter`] saved: the groups of it the device changed.
    pub fn leave(&self, saved: BorrowedState) {
        self.pause(&saved);
    }

    /// Put back what the device changed since it was entered or resumed, as `saved` (what [`Device::enter`] read)
    /// holds it, and keep `saved` for [`Device::resume`]: how a device draws among many of its owner's draws with
    /// the owner's state read once. Nothing is read; only the groups of state the device's calls changed are
    /// written.
    pub fn pause(&self, saved: &BorrowedState) {
        if self.entered.replace(false) {
            state::restore(&self.gl, saved, self.touched.replace(Touched::NONE));
        }
    }

    /// Enter again after [`Device::pause`] without reading the owner's state: `saved` is what the first
    /// [`Device::enter`] read, and the next pause puts back what the device changes to it.
    ///
    /// The owner keeps to its side of this: between the pause and the resume it may change any state, but what it
    /// relies on keeping from one of its draws to the next (its framebuffer, viewport, scissor and blending, say) is
    /// as `saved` holds it, since that is what a pause puts back; state it sets before each of its draws (its
    /// program, textures and buffers) may differ.
    pub fn resume(&self, saved: &BorrowedState) -> Result<()> {
        if !self.borrowed {
            return Err(Error::Invalid("resume is for borrowed devices".into()));
        }
        if self.entered.get() {
            return Err(Error::Invalid("the device is entered already".into()));
        }
        self.enter_with(saved);
        Ok(())
    }

    fn enter_with(&self, saved: &BorrowedState) {
        // Safety: the owner's context is current (the borrow's contract); its state is in `saved`.
        unsafe { self.gl.bind_vertex_array(Some(self.vao)) };
        self.owner_target.set(saved.target());
        self.touched.set(Touched::NONE);
        self.entered.set(true);
    }

    /// Note that a call changes the groups `groups` of the owner's state, which leaving puts back.
    fn touch(&self, groups: Touched) {
        self.touched.set(self.touched.get().with(groups));
    }

    /// The state a borrowed device restores, as the context holds it now.
    pub fn capture_state(&self) -> BorrowedState {
        state::capture(&self.gl)
    }

    /// Before every call: not lost, current, entered when borrowed.
    fn ready(&self) -> Result<()> {
        if let Some(loss) = self.lost.get() {
            return Err(Error::Lost(loss));
        }
        if self.borrowed {
            if !self.entered.get() {
                return Err(Error::Invalid(
                    "a borrowed device is used between enter and leave only".into(),
                ));
            }
        } else if !self.ctx.is_current() {
            self.ctx.make_current()?;
            // Safety: our own context, current again.
            unsafe { self.gl.bind_vertex_array(Some(self.vao)) };
        }
        Ok(())
    }

    /// Whether the context is still usable: the driver's reset status, read now. After a reset or a purge every
    /// call fails with [`Error::Lost`] and the caller makes a new device.
    pub fn status(&self) -> Result<()> {
        if let Some(loss) = self.lost.get() {
            return Err(Error::Lost(loss));
        }
        let status = match (self.injected.take(), self.reset_status) {
            (Some(s), _) => s,
            // Safety: the extension's query, with the context current.
            (None, Some(query)) if self.borrowed || self.ctx.is_current() => unsafe { query() },
            _ => 0,
        };
        match loss_of(status) {
            None => Ok(()),
            Some(loss) => {
                self.lost.set(Some(loss));
                Err(Error::Lost(loss))
            }
        }
    }

    /// Test hook: the next [`Device::status`] reads the status the driver gives for `loss` instead of the driver's
    /// own, as if the context had been reset or purged.
    pub fn inject_loss(&self, loss: Loss) {
        self.injected.set(Some(status_of(loss)));
    }

    // ---- buffers ----

    pub fn create_buffer(&self, desc: &BufferDesc) -> Result<Buffer> {
        self.ready()?;
        let target = buffer_target(desc.usage);
        self.touch(Touched::BUFFERS);
        // Safety: a current context; the buffer is bound and sized.
        unsafe {
            let raw = self.gl.create_buffer().map_err(Error::Driver)?;
            self.gl.bind_buffer(target, Some(raw));
            self.gl
                .buffer_data_size(target, desc.size as i32, glow::DYNAMIC_DRAW);
            self.gl.bind_buffer(target, None);
            Ok(Buffer {
                raw,
                size: desc.size,
                usage: desc.usage,
            })
        }
    }

    pub fn write_buffer(&self, buffer: &Buffer, offset: u64, data: &[u8]) -> Result<()> {
        self.ready()?;
        if offset + data.len() as u64 > buffer.size {
            return Err(Error::Invalid(format!(
                "{} bytes at {offset} into a buffer of {}",
                data.len(),
                buffer.size
            )));
        }
        let target = buffer_target(buffer.usage);
        self.touch(Touched::BUFFERS);
        // Safety: a buffer of this device, written inside its size.
        unsafe {
            self.gl.bind_buffer(target, Some(buffer.raw));
            self.gl
                .buffer_sub_data_u8_slice(target, offset as i32, data);
            self.gl.bind_buffer(target, None);
        }
        Ok(())
    }

    pub fn destroy_buffer(&self, buffer: Buffer) {
        if self.ready().is_ok() {
            // Safety: a buffer of this device, destroyed once.
            unsafe { self.gl.delete_buffer(buffer.raw) };
        }
    }

    // ---- textures ----

    pub fn create_texture(&self, desc: &TextureDesc) -> Result<Texture> {
        self.ready()?;
        let (w, h) = desc.size;
        if w == 0 || h == 0 || w > self.info.max_texture_size || h > self.info.max_texture_size {
            return Err(Error::Invalid(format!("a texture of {w} x {h}")));
        }
        if desc.usage.render_target {
            let renders = match desc.format {
                TextureFormat::Rgba16Float => self.info.half_float_targets,
                TextureFormat::Rgba32Float => self.info.float_targets,
                _ => true,
            };
            if !renders {
                return Err(Error::Unsupported(format!(
                    "rendering to {:?}",
                    desc.format
                )));
            }
        }
        self.touch(Touched::TEXTURES.with(Touched::FRAMEBUFFERS));
        // Safety: a current context; the texture is made, bound and given storage.
        unsafe {
            let raw = self.gl.create_texture().map_err(Error::Driver)?;
            self.gl.bind_texture(glow::TEXTURE_2D, Some(raw));
            self.gl.tex_storage_2d(
                glow::TEXTURE_2D,
                1,
                internal_format(desc.format),
                w as i32,
                h as i32,
            );
            self.texture_defaults();
            let mut texture = Texture {
                raw,
                desc: *desc,
                framebuffer: None,
                image: None,
                bo: None,
                id: next_id(),
                foreign: false,
                counted: 0,
            };
            if desc.usage.render_target {
                match self.framebuffer_of(raw) {
                    Ok(fb) => texture.framebuffer = Some(fb),
                    Err(e) => {
                        self.gl.delete_texture(raw);
                        return Err(e);
                    }
                }
            }
            self.count_made(&mut texture);
            Ok(texture)
        }
    }

    /// A render target over `name`, a 2D texture of `size` texels in 8-bit RGBA the device's owner made in the
    /// context it lent: drawn into, sampled and read like the device's own textures (its filtering set to theirs,
    /// one level, nearest). Destroying the result
    /// ([`Device::destroy_texture`]) releases only the framebuffer the device made for it; the texture stays the
    /// owner's. Only a borrowed device wraps.
    ///
    /// # Safety
    ///
    /// `name` is a texture of the current context with storage of `size` in a colour-renderable 8-bit RGBA format,
    /// and it outlives the result.
    pub unsafe fn wrap_texture(&self, name: u32, size: (u32, u32)) -> Result<Texture> {
        self.ready()?;
        if !self.is_borrowed() {
            return Err(Error::Invalid(
                "only a borrowed device wraps its owner's textures".into(),
            ));
        }
        let raw = std::num::NonZeroU32::new(name)
            .map(glow::NativeTexture)
            .ok_or_else(|| Error::Invalid("texture name 0".into()))?;
        let (w, h) = size;
        if w == 0 || h == 0 {
            return Err(Error::Invalid(format!("a texture of {w} x {h}")));
        }
        self.touch(Touched::FRAMEBUFFERS.with(Touched::TEXTURES));
        // Safety: the caller's texture of the current context; it reads its one level, as the device's own do (its
        // owner sets its own filtering when it samples it)
        let framebuffer = unsafe {
            self.gl.bind_texture(glow::TEXTURE_2D, Some(raw));
            self.texture_defaults();
            self.framebuffer_of(raw)
        }?;
        let mut texture = Texture {
            raw,
            desc: TextureDesc {
                size,
                format: TextureFormat::Rgba8,
                usage: TextureUsage {
                    sampled: true,
                    render_target: true,
                },
            },
            framebuffer: Some(framebuffer),
            image: None,
            bo: None,
            id: next_id(),
            foreign: true,
            counted: 0,
        };
        self.count_made(&mut texture);
        Ok(texture)
    }

    /// Without a sampler bound, a texture reads its single level, nearest, clamped: complete without mipmaps.
    unsafe fn texture_defaults(&self) {
        let gl = &self.gl;
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_MIN_FILTER,
            glow::NEAREST as i32,
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_MAG_FILTER,
            glow::NEAREST as i32,
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_WRAP_S,
            glow::CLAMP_TO_EDGE as i32,
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_WRAP_T,
            glow::CLAMP_TO_EDGE as i32,
        );
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAX_LEVEL, 0);
    }

    unsafe fn framebuffer_of(&self, texture: glow::Texture) -> Result<glow::Framebuffer> {
        let gl = &self.gl;
        let fb = gl.create_framebuffer().map_err(Error::Driver)?;
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fb));
        gl.framebuffer_texture_2d(
            glow::FRAMEBUFFER,
            glow::COLOR_ATTACHMENT0,
            glow::TEXTURE_2D,
            Some(texture),
            0,
        );
        let status = gl.check_framebuffer_status(glow::FRAMEBUFFER);
        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        if status != glow::FRAMEBUFFER_COMPLETE {
            gl.delete_framebuffer(fb);
            return Err(Error::Unsupported(format!(
                "the texture cannot be rendered to ({status:#x})"
            )));
        }
        Ok(fb)
    }

    /// Write `data` (rows first to last, tightly packed, in the texture's format; 16-bit floats for
    /// [`TextureFormat::Rgba16Float`]) into `region`.
    pub fn write_texture(&self, texture: &Texture, region: Region, data: &[u8]) -> Result<()> {
        self.ready()?;
        let bpp = texture.desc.format.bytes_per_pixel() as usize;
        let need = region.width as usize * region.height as usize * bpp;
        let (w, h) = texture.desc.size;
        if data.len() != need || region.x + region.width > w || region.y + region.height > h {
            return Err(Error::Invalid(format!(
                "{} bytes for {region:?} of a {w} x {h} texture",
                data.len()
            )));
        }
        if texture.is_external() {
            return Err(Error::Invalid(
                "dmabuf textures are drawn into, not written".into(),
            ));
        }
        let (format, ty) = upload_format(texture.desc.format);
        self.touch(Touched::TEXTURES.with(Touched::PIXELS));
        // Safety: a texture of this device; the data is as large as the region.
        unsafe {
            self.gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            self.gl.pixel_store_i32(glow::UNPACK_ROW_LENGTH, 0);
            self.gl.bind_texture(glow::TEXTURE_2D, Some(texture.raw));
            self.gl.tex_sub_image_2d(
                glow::TEXTURE_2D,
                0,
                region.x as i32,
                region.y as i32,
                region.width as i32,
                region.height as i32,
                format,
                ty,
                glow::PixelUnpackData::Slice(Some(data)),
            );
        }
        Ok(())
    }

    fn read(
        &self,
        texture: &Texture,
        region: Region,
        format: u32,
        ty: u32,
        out: &mut [u8],
    ) -> Result<()> {
        self.ready()?;
        let fb = texture
            .framebuffer
            .ok_or_else(|| Error::Invalid("only render targets are read back".into()))?;
        let (w, h) = texture.desc.size;
        if region.x + region.width > w || region.y + region.height > h {
            return Err(Error::Invalid(format!("{region:?} of a {w} x {h} texture")));
        }
        self.touch(Touched::FRAMEBUFFERS.with(Touched::PIXELS));
        // Safety: a complete framebuffer of this device; `out` holds the region in the format asked for.
        unsafe {
            self.gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(fb));
            self.gl.pixel_store_i32(glow::PACK_ALIGNMENT, 1);
            self.gl.read_pixels(
                region.x as i32,
                region.y as i32,
                region.width as i32,
                region.height as i32,
                format,
                ty,
                glow::PixelPackData::Slice(Some(out)),
            );
            self.gl.bind_framebuffer(glow::READ_FRAMEBUFFER, None);
            match self.gl.get_error() {
                glow::NO_ERROR => Ok(()),
                e => Err(Error::Driver(format!("reading back: {e:#x}"))),
            }
        }
    }

    /// The pixels of `region` of an 8-bit render target, rows first to last, in its format's channels.
    pub fn read_bytes(&self, texture: &Texture, region: Region) -> Result<Vec<u8>> {
        let n = region.width as usize * region.height as usize;
        let mut rgba = vec![0u8; n * 4];
        match texture.desc.format {
            TextureFormat::Rgba8 => {
                self.read(texture, region, glow::RGBA, glow::UNSIGNED_BYTE, &mut rgba)?;
                Ok(rgba)
            }
            TextureFormat::R8 => {
                self.read(texture, region, glow::RGBA, glow::UNSIGNED_BYTE, &mut rgba)?;
                Ok(rgba.as_chunks::<4>().0.iter().map(|p| p[0]).collect())
            }
            f => Err(Error::Invalid(format!("{f:?} is read with read_floats"))),
        }
    }

    /// The pixels of `region` of a float render target, rows first to last, four `f32` a pixel.
    pub fn read_floats(&self, texture: &Texture, region: Region) -> Result<Vec<f32>> {
        match texture.desc.format {
            TextureFormat::Rgba16Float | TextureFormat::Rgba32Float => {
                let n = region.width as usize * region.height as usize * 4;
                let mut bytes = vec![0u8; n * 4];
                self.read(texture, region, glow::RGBA, glow::FLOAT, &mut bytes)?;
                Ok(bytes
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|b| f32::from_ne_bytes(*b))
                    .collect())
            }
            f => Err(Error::Invalid(format!("{f:?} is read with read_bytes"))),
        }
    }

    pub fn destroy_texture(&self, texture: Texture) {
        let t = self.live.get();
        self.live.set(TextureTally {
            textures: t.textures.saturating_sub(1),
            bytes: t.bytes.saturating_sub(texture.counted),
        });
        if self.ready().is_err() {
            return;
        }
        // Safety: a texture of this device and what it owns, destroyed once.
        unsafe {
            if let Some(fb) = texture.framebuffer {
                self.gl.delete_framebuffer(fb);
            }
            if !texture.foreign {
                self.gl.delete_texture(texture.raw);
            }
            if let Some(image) = texture.image {
                self.ctx.destroy_image(image);
            }
            if let Some(bo) = texture.bo {
                self.ctx.gbm_free(bo);
            }
        }
    }

    // ---- external memory ----

    /// The modifiers of `fourcc` the device renders to.
    pub fn renderable_modifiers(&self, fourcc: u32) -> Vec<u64> {
        self.ctx.renderable_modifiers(fourcc)
    }

    /// A render target whose memory is a dmabuf: a GBM buffer object with one of the modifiers offered, or,
    /// without GBM, a texture exported in a layout the offer must hold.
    pub fn allocate_dmabuf(&self, desc: &DmabufDesc) -> Result<ExternalImage> {
        self.ready()?;
        match self.info.allocator {
            Some(Allocator::Gbm) => {
                let (planes, modifier, bo) =
                    self.ctx
                        .gbm_allocate(desc.size, desc.fourcc, &desc.modifiers)?;
                let raw: Vec<(i32, u32, u32)> = planes
                    .iter()
                    .map(|p| (p.fd.as_raw_fd(), p.offset, p.stride))
                    .collect();
                let dmabuf = Dmabuf {
                    size: desc.size,
                    fourcc: desc.fourcc,
                    modifier,
                    planes: planes
                        .into_iter()
                        .map(|p| DmabufPlane {
                            fd: p.fd,
                            offset: p.offset,
                            stride: p.stride,
                        })
                        .collect(),
                };
                let texture = match self.ctx.import(desc.size, desc.fourcc, modifier, &raw) {
                    Ok(image) => self.texture_of_image(image, desc.size, Some(bo)),
                    Err(e) => {
                        // Safety: the object made above, not yet handed anywhere.
                        unsafe { self.ctx.gbm_free(bo) };
                        Err(e)
                    }
                }?;
                Ok(ExternalImage { texture, dmabuf })
            }
            Some(Allocator::Export) => {
                let texture = self.create_texture(&TextureDesc {
                    size: desc.size,
                    format: TextureFormat::Rgba8,
                    usage: TextureUsage::TARGET,
                })?;
                let (image, plane, fourcc, modifier) = match self.ctx.export(raw_name(texture.raw))
                {
                    Ok(x) => x,
                    Err(e) => {
                        self.destroy_texture(texture);
                        return Err(e);
                    }
                };
                if !desc.modifiers.contains(&modifier) {
                    // Safety: the image made by the export above.
                    unsafe { self.ctx.destroy_image(image) };
                    self.destroy_texture(texture);
                    return Err(Error::Unsupported(format!(
                        "the exported layout {modifier:#x} is not offered"
                    )));
                }
                let mut texture = texture;
                texture.image = Some(image);
                Ok(ExternalImage {
                    texture,
                    dmabuf: Dmabuf {
                        size: desc.size,
                        fourcc,
                        modifier,
                        planes: vec![DmabufPlane {
                            fd: plane.fd,
                            offset: plane.offset,
                            stride: plane.stride,
                        }],
                    },
                })
            }
            None => Err(Error::Unsupported("the device allocates no dmabufs".into())),
        }
    }

    /// A texture (sampled and drawn into) whose memory is the dmabuf `dmabuf`, made by another device or process.
    pub fn import_dmabuf(&self, dmabuf: &Dmabuf) -> Result<Texture> {
        self.ready()?;
        if !self.ctx.can_import() {
            return Err(Error::Unsupported("no dmabuf import".into()));
        }
        let raw: Vec<(i32, u32, u32)> = dmabuf
            .planes
            .iter()
            .map(|p| (p.fd.as_raw_fd(), p.offset, p.stride))
            .collect();
        let image = self
            .ctx
            .import(dmabuf.size, dmabuf.fourcc, dmabuf.modifier, &raw)?;
        self.texture_of_image(image, dmabuf.size, None)
    }

    fn texture_of_image(
        &self,
        image: egl::EGLImage,
        size: (u32, u32),
        bo: Option<*mut c_void>,
    ) -> Result<Texture> {
        let target = self
            .image_target_texture
            .ok_or_else(|| Error::Unsupported("no glEGLImageTargetTexture2DOES".into()))?;
        self.touch(Touched::TEXTURES.with(Touched::FRAMEBUFFERS));
        // Safety: a current context; the image is valid and becomes the texture's storage.
        unsafe {
            let raw = self.gl.create_texture().map_err(Error::Driver)?;
            self.gl.bind_texture(glow::TEXTURE_2D, Some(raw));
            target(glow::TEXTURE_2D, image);
            self.texture_defaults();
            let mut texture = Texture {
                raw,
                desc: TextureDesc {
                    size,
                    format: TextureFormat::Rgba8,
                    usage: TextureUsage::TARGET,
                },
                framebuffer: None,
                image: Some(image),
                bo,
                id: next_id(),
                foreign: false,
                counted: 0,
            };
            match self.framebuffer_of(raw) {
                Ok(fb) => texture.framebuffer = Some(fb),
                // an import-only layout is still sampled
                Err(_) => texture.desc.usage = TextureUsage::SAMPLED,
            }
            self.count_made(&mut texture);
            Ok(texture)
        }
    }

    // ---- samplers ----

    pub fn create_sampler(&self, desc: &SamplerDesc) -> Result<Sampler> {
        self.ready()?;
        let filter = match desc.filter {
            Filter::Nearest => glow::NEAREST,
            Filter::Linear => glow::LINEAR,
        } as i32;
        let wrap = match desc.address {
            AddressMode::ClampToEdge => glow::CLAMP_TO_EDGE,
            AddressMode::Repeat => glow::REPEAT,
        } as i32;
        // Safety: a current context; a new sampler object's parameters.
        unsafe {
            let raw = self.gl.create_sampler().map_err(Error::Driver)?;
            self.gl
                .sampler_parameter_i32(raw, glow::TEXTURE_MIN_FILTER, filter);
            self.gl
                .sampler_parameter_i32(raw, glow::TEXTURE_MAG_FILTER, filter);
            self.gl
                .sampler_parameter_i32(raw, glow::TEXTURE_WRAP_S, wrap);
            self.gl
                .sampler_parameter_i32(raw, glow::TEXTURE_WRAP_T, wrap);
            Ok(Sampler { raw })
        }
    }

    pub fn destroy_sampler(&self, sampler: Sampler) {
        if self.ready().is_ok() {
            // Safety: a sampler of this device, destroyed once.
            unsafe { self.gl.delete_sampler(sampler.raw) };
        }
    }

    // ---- bind groups and pipelines ----

    pub fn create_bind_group_layout(&self, entries: &[BindingLayout]) -> Result<BindGroupLayout> {
        for e in entries {
            binding_index(0, e.binding)?;
        }
        Ok(BindGroupLayout {
            entries: entries.to_vec(),
        })
    }

    pub fn create_bind_group(
        &self,
        layout: &BindGroupLayout,
        entries: &[BindGroupEntry<'_>],
    ) -> Result<BindGroup> {
        self.ready()?;
        let mut out = Vec::new();
        for l in &layout.entries {
            let e = entries
                .iter()
                .find(|e| e.binding == l.binding)
                .ok_or_else(|| Error::Invalid(format!("binding {} has no resource", l.binding)))?;
            let bound = match (l.kind, e.resource) {
                (
                    BindingKind::UniformBuffer,
                    BindingResource::Buffer {
                        buffer,
                        offset,
                        size,
                    },
                ) => {
                    if buffer.usage != BufferUsage::Uniform || offset + size > buffer.size {
                        return Err(Error::Invalid(format!(
                            "binding {}: not a uniform range",
                            l.binding
                        )));
                    }
                    Bound::Buffer {
                        raw: buffer.raw,
                        offset,
                        size,
                    }
                }
                (BindingKind::Texture, BindingResource::Texture(t)) => {
                    if !t.desc.usage.sampled {
                        return Err(Error::Invalid(format!(
                            "binding {}: the texture is not sampled",
                            l.binding
                        )));
                    }
                    Bound::Texture(t.raw)
                }
                (BindingKind::Sampler, BindingResource::Sampler(s)) => Bound::Sampler(s.raw),
                (kind, _) => {
                    return Err(Error::Invalid(format!(
                        "binding {} holds a {kind:?}",
                        l.binding
                    )));
                }
            };
            out.push((l.binding, bound));
        }
        Ok(BindGroup { entries: out })
    }

    pub fn create_pipeline(&self, desc: &PipelineDesc<'_>) -> Result<Pipeline> {
        self.ready()?;
        let gl = &self.gl;
        self.touch(Touched::PROGRAM);
        // Safety: a current context; shaders compiled and linked, names looked up in the linked program.
        unsafe {
            let program = self.link(desc.shader)?;
            gl.use_program(Some(program));
            let mut textures = Vec::new();
            let mut result = Ok(());
            for b in desc.shader.bindings() {
                match b {
                    NamedBinding::Uniform {
                        group,
                        binding,
                        block,
                    } => {
                        let point = binding_index(group, binding);
                        match (point, gl.get_uniform_block_index(program, &block)) {
                            (Ok(point), Some(index)) => {
                                gl.uniform_block_binding(program, index, point)
                            }
                            (Err(e), _) => result = Err(e),
                            (_, None) => {
                                result = Err(Error::Driver(format!(
                                    "{}: no block {block}",
                                    desc.shader.name
                                )))
                            }
                        }
                    }
                    NamedBinding::Texture {
                        group,
                        binding,
                        sampler,
                        name,
                    } => match binding_index(group, binding) {
                        Ok(unit) => {
                            if let Some(location) = gl.get_uniform_location(program, &name) {
                                gl.uniform_1_i32(Some(&location), unit as i32);
                            }
                            textures.push((group, binding, sampler));
                        }
                        Err(e) => result = Err(e),
                    },
                }
            }
            // the program stays current: a pass sets its own, and a borrowed device's leave restores the owner's
            // (an earlier program may be one deleted since, which cannot be made current again)
            if let Err(e) = result {
                gl.delete_program(program);
                return Err(e);
            }
            Ok(Pipeline {
                program,
                layouts: desc.layouts.iter().map(|l| (*l).clone()).collect(),
                vertex_buffers: desc.vertex_buffers.to_vec(),
                blend: desc.blend,
                topology: desc.topology,
                textures,
            })
        }
    }

    /// The linked program of `shader`: loaded from a saved binary where the cache has one the driver takes, else
    /// compiled and linked (and saved).
    fn link(&self, shader: &ShaderSource) -> Result<glow::Program> {
        let gl = &self.gl;
        let file = program_key(shader.name, shader.vertex_glsl, shader.fragment_glsl);
        let cache = self.program_cache.as_ref();
        if let Some((format, buffer)) = cache.and_then(|c| c.load(&file)) {
            // Safety: a current context; the program is deleted when the driver refuses the binary.
            unsafe {
                let program = gl.create_program().map_err(Error::Driver)?;
                gl.program_binary(program, &glow::ProgramBinary { buffer, format });
                if gl.get_program_link_status(program) {
                    if let Some(c) = cache {
                        c.note_loaded();
                    }
                    return Ok(program);
                }
                gl.delete_program(program);
                // a format the driver does not know leaves an error, which would fail the next query that checks
                while gl.get_error() != glow::NO_ERROR {}
            }
            if let Some(c) = cache {
                c.note_refused(&file);
            }
        }
        // Safety: a current context; shaders and the program are deleted on every failure.
        unsafe {
            let compile = |kind: u32, source: &str| -> Result<glow::Shader> {
                let s = gl.create_shader(kind).map_err(Error::Driver)?;
                gl.shader_source(s, source);
                gl.compile_shader(s);
                if !gl.get_shader_compile_status(s) {
                    let log = gl.get_shader_info_log(s);
                    gl.delete_shader(s);
                    return Err(Error::Driver(format!("{}: {log}", shader.name)));
                }
                Ok(s)
            };
            let vs = compile(glow::VERTEX_SHADER, shader.vertex_glsl)?;
            let fs = match compile(glow::FRAGMENT_SHADER, shader.fragment_glsl) {
                Ok(fs) => fs,
                Err(e) => {
                    gl.delete_shader(vs);
                    return Err(e);
                }
            };
            let program = gl.create_program().map_err(Error::Driver)?;
            if cache.is_some() {
                gl.program_binary_retrievable_hint(program, true);
            }
            gl.attach_shader(program, vs);
            gl.attach_shader(program, fs);
            gl.link_program(program);
            gl.detach_shader(program, vs);
            gl.detach_shader(program, fs);
            gl.delete_shader(vs);
            gl.delete_shader(fs);
            if !gl.get_program_link_status(program) {
                let log = gl.get_program_info_log(program);
                gl.delete_program(program);
                return Err(Error::Driver(format!("{}: {log}", shader.name)));
            }
            if let Some(c) = cache {
                c.note_compiled();
                if let Some(binary) = gl.get_program_binary(program) {
                    c.store(&file, binary.format, &binary.buffer);
                }
            }
            Ok(program)
        }
    }

    pub fn destroy_pipeline(&self, pipeline: Pipeline) {
        if self.ready().is_ok() {
            // Safety: a program of this device, destroyed once.
            unsafe { self.gl.delete_program(pipeline.program) };
        }
    }

    // ---- passes and submission ----

    /// Start a render pass into `target`, cleared or kept.
    pub fn begin_pass<'a>(&'a self, target: &Texture, load: LoadOp) -> Result<Pass<'a>> {
        self.ready()?;
        let fb = target
            .framebuffer
            .ok_or_else(|| Error::Invalid("the target is not a render target".into()))?;
        let (w, h) = target.desc.size;
        let gl = &self.gl;
        self.touch(
            Touched::FRAMEBUFFERS
                .with(Touched::VIEWPORT)
                .with(Touched::SCISSOR)
                .with(Touched::TESTS)
                .with(Touched::BLEND)
                .with(Touched::MASKS),
        );
        // Safety: a complete framebuffer of this device; the pass's fixed state set before any draw.
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fb));
            gl.viewport(0, 0, w as i32, h as i32);
            gl.disable(glow::SCISSOR_TEST);
            gl.disable(glow::DEPTH_TEST);
            gl.disable(glow::STENCIL_TEST);
            gl.disable(glow::CULL_FACE);
            gl.disable(glow::BLEND);
            gl.color_mask(true, true, true, true);
            if let LoadOp::Clear([r, g, b, a]) = load {
                gl.clear_color(r, g, b, a);
                gl.clear(glow::COLOR_BUFFER_BIT);
            }
        }
        Ok(Pass {
            device: self,
            size: (w, h),
            pipeline: None,
            attributes: Vec::new(),
            index: false,
            owners: false,
        })
    }

    /// Start a render pass into the framebuffer the owner of a borrowed device has bound, at the viewport the owner
    /// set, keeping what the framebuffer holds: how a borrowed device draws among its owner's own draws.
    ///
    /// The pass sets the fixed state of every pass (no scissor, depth, stencil, culling or blending, every channel
    /// written) and leaves the framebuffer and the viewport as the owner has them. The framebuffer's orientation is
    /// the owner's: a pipeline places its vertices through the projection the owner gives it, and
    /// [`Pass::set_scissor`] regions and fragment positions are the framebuffer's pixels as the owner's context
    /// counts them. [`Pass::size`] is the viewport's size. [`Device::leave`] puts back all the state the pass
    /// changed, the owner's framebuffer and viewport among it.
    pub fn begin_owner_pass(&self) -> Result<Pass<'_>> {
        if !self.borrowed {
            return Err(Error::Invalid(
                "only a borrowed device draws into its owner's framebuffer".into(),
            ));
        }
        self.ready()?;
        let gl = &self.gl;
        // the framebuffer and viewport entering read: the owner's, bound again if a pass of the device's own
        // changed them since
        let (framebuffer, viewport) = self.owner_target.get();
        let moved = self
            .touched
            .get()
            .has(Touched::FRAMEBUFFERS.with(Touched::VIEWPORT));
        self.touch(
            Touched::SCISSOR
                .with(Touched::TESTS)
                .with(Touched::BLEND)
                .with(Touched::MASKS),
        );
        // Safety: the owner's context is current and entered; its framebuffer stays bound, the pass's fixed state is
        // set before any draw and restored on leave.
        unsafe {
            if moved {
                gl.bind_framebuffer(glow::FRAMEBUFFER, framebuffer);
                let [x, y, w, h] = viewport;
                gl.viewport(x, y, w, h);
            }
            gl.disable(glow::SCISSOR_TEST);
            gl.disable(glow::DEPTH_TEST);
            gl.disable(glow::STENCIL_TEST);
            gl.disable(glow::CULL_FACE);
            gl.disable(glow::BLEND);
            gl.color_mask(true, true, true, true);
        }
        Ok(Pass {
            device: self,
            size: (viewport[2].max(0) as u32, viewport[3].max(0) as u32),
            pipeline: None,
            attributes: Vec::new(),
            index: false,
            owners: true,
        })
    }

    /// Submit the work recorded so far: a fence that signals when it is done.
    pub fn submit(&self) -> Result<Fence> {
        self.ready()?;
        // Safety: a current context.
        unsafe {
            let raw = self
                .gl
                .fence_sync(glow::SYNC_GPU_COMMANDS_COMPLETE, 0)
                .map_err(Error::Driver)?;
            self.gl.flush();
            Ok(Fence { raw })
        }
    }

    /// Submit the work recorded so far with a native fence too: the sync file to hand to the compositor or another
    /// device, which signals when the work is done.
    pub fn submit_exported(&self) -> Result<(Fence, OwnedFd)> {
        self.ready()?;
        // Safety: a current context.
        let raw = unsafe { self.gl.fence_sync(glow::SYNC_GPU_COMMANDS_COMPLETE, 0) }
            .map_err(Error::Driver)?;
        let fence = Fence { raw };
        // Safety: a current context; the native fence is made before the flush that submits it.
        match self.ctx.native_fence(|| unsafe { self.gl.flush() }) {
            Ok(fd) => Ok((fence, fd)),
            Err(e) => {
                self.destroy_fence(fence);
                Err(e)
            }
        }
    }

    /// Make the work submitted after this wait on the GPU for the sync file `fence` (another device's or process's
    /// work), without waiting on the CPU.
    pub fn wait_sync_file(&self, fence: OwnedFd) -> Result<()> {
        self.ready()?;
        self.ctx.wait_native_fence(fence)
    }

    /// Wait on the CPU up to `timeout` for `fence`; whether it signalled.
    pub fn wait(&self, fence: &Fence, timeout: std::time::Duration) -> Result<bool> {
        self.ready()?;
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            // the GL timeout is a signed 32-bit count of nanoseconds here
            let step = left.as_nanos().min(i32::MAX as u128) as i32;
            // Safety: a fence of this device.
            let r = unsafe {
                self.gl
                    .client_wait_sync(fence.raw, glow::SYNC_FLUSH_COMMANDS_BIT, step)
            };
            match r {
                glow::ALREADY_SIGNALED | glow::CONDITION_SATISFIED => return Ok(true),
                glow::TIMEOUT_EXPIRED if left.is_zero() => return Ok(false),
                glow::TIMEOUT_EXPIRED => {}
                _ => {
                    self.status()?;
                    return Err(Error::Driver("glClientWaitSync failed".into()));
                }
            }
        }
    }

    /// Whether `fence` has signalled, without waiting.
    pub fn is_signalled(&self, fence: &Fence) -> Result<bool> {
        self.wait(fence, std::time::Duration::ZERO)
    }

    pub fn destroy_fence(&self, fence: Fence) {
        if self.ready().is_ok() {
            // Safety: a fence of this device, destroyed once.
            unsafe { self.gl.delete_sync(fence.raw) };
        }
    }

    /// Wait on the CPU for all work submitted so far.
    pub fn finish(&self) -> Result<()> {
        self.ready()?;
        // Safety: a current context.
        unsafe { self.gl.finish() };
        Ok(())
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        if self.ready().is_ok() || self.lost.get().is_some() {
            // Safety: the device's own vertex array, deleted once while its context is current.
            unsafe { self.gl.delete_vertex_array(self.vao) };
        }
    }
}

/// A device current on no thread, to be sent to the thread that uses it ([`Device::detach`]).
#[derive(Debug)]
pub struct DetachedDevice(Device);

// Safety: an owned EGL context that is current on no thread may be made current on any one thread; the device's
// other state is plain data and GL names, used only once the context is current on the receiving thread again.
unsafe impl Send for DetachedDevice {}

impl DetachedDevice {
    /// The device again, its context made current on this thread.
    pub fn attach(self) -> Result<Device> {
        let device = self.0;
        device.ready()?;
        Ok(device)
    }
}

/// Wait on the CPU up to `timeout` for a sync file; whether it signalled.
pub fn wait_sync_file(fence: &OwnedFd, timeout: std::time::Duration) -> bool {
    let mut pfd = PollFd {
        fd: fence.as_raw_fd(),
        events: POLLIN,
        revents: 0,
    };
    let ms = timeout.as_millis().min(i32::MAX as u128) as i32;
    // Safety: one valid pollfd for an open file descriptor.
    unsafe { poll(&mut pfd, 1, ms) == 1 && pfd.revents & POLLIN != 0 }
}

#[repr(C)]
struct PollFd {
    fd: i32,
    events: i16,
    revents: i16,
}
const POLLIN: i16 = 1;
extern "C" {
    fn poll(fds: *mut PollFd, nfds: u64, timeout: i32) -> i32;
}

fn raw_name(texture: glow::Texture) -> u32 {
    texture.0.get()
}

fn buffer_target(usage: BufferUsage) -> u32 {
    match usage {
        BufferUsage::Uniform => glow::UNIFORM_BUFFER,
        BufferUsage::Vertex => glow::ARRAY_BUFFER,
        BufferUsage::Index => glow::ELEMENT_ARRAY_BUFFER,
    }
}

fn internal_format(format: TextureFormat) -> u32 {
    match format {
        TextureFormat::R8 => glow::R8,
        TextureFormat::Rgba8 => glow::RGBA8,
        TextureFormat::Rgba16Float => glow::RGBA16F,
        TextureFormat::Rgba32Float => glow::RGBA32F,
    }
}

fn upload_format(format: TextureFormat) -> (u32, u32) {
    match format {
        TextureFormat::R8 => (glow::RED, glow::UNSIGNED_BYTE),
        TextureFormat::Rgba8 => (glow::RGBA, glow::UNSIGNED_BYTE),
        TextureFormat::Rgba16Float => (glow::RGBA, glow::HALF_FLOAT),
        TextureFormat::Rgba32Float => (glow::RGBA, glow::FLOAT),
    }
}

/// A render pass being recorded: pipelines, bind groups, vertex buffers and draws into its target.
pub struct Pass<'a> {
    device: &'a Device,
    size: (u32, u32),
    pipeline: Option<&'a Pipeline>,
    attributes: Vec<u32>,
    index: bool,
    /// The target is the framebuffer the owner bound ([`Device::begin_owner_pass`]), which stays bound.
    owners: bool,
}

impl<'a> Pass<'a> {
    pub fn set_pipeline(&mut self, pipeline: &'a Pipeline) {
        let gl = &self.device.gl;
        self.device.touch(Touched::PROGRAM.with(Touched::BLEND));
        // Safety: a program of this device; blending as the pipeline holds it.
        unsafe {
            gl.use_program(Some(pipeline.program));
            match pipeline.blend {
                Blend::Replace => gl.disable(glow::BLEND),
                Blend::PremultipliedOver => {
                    gl.enable(glow::BLEND);
                    gl.blend_equation(glow::FUNC_ADD);
                    gl.blend_func(glow::ONE, glow::ONE_MINUS_SRC_ALPHA);
                }
                Blend::StraightOver => {
                    gl.enable(glow::BLEND);
                    gl.blend_equation(glow::FUNC_ADD);
                    gl.blend_func_separate(
                        glow::SRC_ALPHA,
                        glow::ONE_MINUS_SRC_ALPHA,
                        glow::ZERO,
                        glow::ONE,
                    );
                }
            }
        }
        self.pipeline = Some(pipeline);
    }

    /// Bind `group` at `index` of the current pipeline's layouts.
    pub fn set_bind_group(&mut self, index: u32, group: &BindGroup) -> Result<()> {
        let pipeline = self
            .pipeline
            .ok_or_else(|| Error::Invalid("no pipeline set".into()))?;
        if index as usize >= pipeline.layouts.len() {
            return Err(Error::Invalid(format!("the pipeline has no group {index}")));
        }
        let gl = &self.device.gl;
        self.device.touch(Touched::BUFFERS.with(Touched::TEXTURES));
        for &(binding, bound) in &group.entries {
            let slot = binding_index(index, binding)?;
            // Safety: resources of this device bound at the points the pipeline's program reads.
            unsafe {
                match bound {
                    Bound::Buffer { raw, offset, size } => gl.bind_buffer_range(
                        glow::UNIFORM_BUFFER,
                        slot,
                        Some(raw),
                        offset as i32,
                        size as i32,
                    ),
                    Bound::Texture(raw) => {
                        gl.active_texture(glow::TEXTURE0 + slot);
                        gl.bind_texture(glow::TEXTURE_2D, Some(raw));
                        // a texture read without a sampler uses its own parameters
                        if pipeline
                            .textures
                            .iter()
                            .any(|t| (t.0, t.1, t.2) == (index, binding, None))
                        {
                            gl.bind_sampler(slot, None);
                        }
                    }
                    Bound::Sampler(raw) => {
                        for t in pipeline
                            .textures
                            .iter()
                            .filter(|t| t.2 == Some((index, binding)))
                        {
                            gl.bind_sampler(binding_index(t.0, t.1)?, Some(raw));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Read vertex buffer slot `slot` of the pipeline from `buffer` at `offset`.
    pub fn set_vertex_buffer(&mut self, slot: u32, buffer: &Buffer, offset: u64) -> Result<()> {
        let pipeline = self
            .pipeline
            .ok_or_else(|| Error::Invalid("no pipeline set".into()))?;
        let layout = pipeline
            .vertex_buffers
            .get(slot as usize)
            .ok_or_else(|| Error::Invalid(format!("the pipeline has no vertex buffer {slot}")))?;
        let gl = &self.device.gl;
        self.device.touch(Touched::BUFFERS);
        // Safety: the device's own vertex array is bound; attributes point into a buffer of this device.
        unsafe {
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(buffer.raw));
            for a in &layout.attributes {
                gl.enable_vertex_attrib_array(a.location);
                let at = (offset + a.offset as u64) as i32;
                match a.format {
                    VertexFormat::Float32x2 => gl.vertex_attrib_pointer_f32(
                        a.location,
                        2,
                        glow::FLOAT,
                        false,
                        layout.stride as i32,
                        at,
                    ),
                    VertexFormat::Float32x4 => gl.vertex_attrib_pointer_f32(
                        a.location,
                        4,
                        glow::FLOAT,
                        false,
                        layout.stride as i32,
                        at,
                    ),
                    VertexFormat::Unorm8x4 => gl.vertex_attrib_pointer_f32(
                        a.location,
                        4,
                        glow::UNSIGNED_BYTE,
                        true,
                        layout.stride as i32,
                        at,
                    ),
                }
                gl.vertex_attrib_divisor(
                    a.location,
                    matches!(layout.step, StepMode::Instance) as u32,
                );
                self.attributes.push(a.location);
            }
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
        }
        Ok(())
    }

    /// Read 16-bit indices from `buffer`.
    pub fn set_index_buffer(&mut self, buffer: &Buffer) -> Result<()> {
        if buffer.usage != BufferUsage::Index {
            return Err(Error::Invalid("not an index buffer".into()));
        }
        // Safety: the device's own vertex array holds the binding.
        unsafe {
            self.device
                .gl
                .bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(buffer.raw))
        };
        self.index = true;
        Ok(())
    }

    /// Draw only inside `region` (pixels from the first row; in a pass into the owner's framebuffer, its pixels as
    /// the owner's context counts them), or everywhere.
    pub fn set_scissor(&mut self, region: Option<Region>) {
        let gl = &self.device.gl;
        self.device.touch(Touched::SCISSOR);
        // Safety: plain state of the pass; fragment row 0 is the target's first row, so no flip.
        unsafe {
            match region {
                Some(r) => {
                    gl.enable(glow::SCISSOR_TEST);
                    gl.scissor(r.x as i32, r.y as i32, r.width as i32, r.height as i32);
                }
                None => gl.disable(glow::SCISSOR_TEST),
            }
        }
    }

    fn mode(&self) -> Result<u32> {
        let p = self
            .pipeline
            .ok_or_else(|| Error::Invalid("no pipeline set".into()))?;
        Ok(match p.topology {
            Topology::TriangleList => glow::TRIANGLES,
            Topology::TriangleStrip => glow::TRIANGLE_STRIP,
        })
    }

    /// Draw `vertices` vertices of `instances` instances (instance buffers start at their bound offset).
    pub fn draw(&mut self, vertices: std::ops::Range<u32>, instances: u32) -> Result<()> {
        let mode = self.mode()?;
        // Safety: a pipeline is set and its inputs bound.
        unsafe {
            self.device.gl.draw_arrays_instanced(
                mode,
                vertices.start as i32,
                vertices.len() as i32,
                instances as i32,
            )
        };
        Ok(())
    }

    /// Draw `count` indices from the index buffer, `instances` times.
    pub fn draw_indexed(&mut self, count: u32, instances: u32) -> Result<()> {
        let mode = self.mode()?;
        if !self.index {
            return Err(Error::Invalid("no index buffer set".into()));
        }
        // Safety: a pipeline is set, its inputs and an index buffer bound.
        unsafe {
            self.device.gl.draw_elements_instanced(
                mode,
                count as i32,
                glow::UNSIGNED_SHORT,
                0,
                instances as i32,
            )
        };
        Ok(())
    }

    /// The target's size.
    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// End the pass.
    pub fn end(self) {}
}

#[cfg(test)]
mod tests;

impl Drop for Pass<'_> {
    fn drop(&mut self) {
        let gl = &self.device.gl;
        // Safety: the device's own vertex array: the attributes the pass enabled are disabled again.
        unsafe {
            for &a in &self.attributes {
                gl.vertex_attrib_divisor(a, 0);
                gl.disable_vertex_attrib_array(a);
            }
            if self.index {
                gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, None);
            }
            if !self.owners {
                gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            }
        }
    }
}
