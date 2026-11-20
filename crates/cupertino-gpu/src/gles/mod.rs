//! The OpenGL ES 3.0 backend over EGL.
//!
//! Binding points: the uniform block at group `g`, binding `b` is bound to uniform buffer point `g * 8 + b`, the
//! texture there to texture unit `g * 8 + b`; groups 0 and 1, bindings 0 to 7. Blocks and samplers of a program are
//! found by the names its generated bindings list, since GLSL ES 3.00 has no binding qualifiers.

mod egl;
mod state;

pub use egl::ExternalTally;

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
    /// Whether the driver times the GPU's own work (`GL_EXT_disjoint_timer_query`).
    timer_query: bool,
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
        let timer_query = has("GL_EXT_disjoint_timer_query");
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
            timer_query,
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
}

mod pass;
mod resources;

pub use pass::{wait_sync_file, Pass};

impl Drop for Device {
    fn drop(&mut self) {
        // a device not lost makes its context current first (`ready`); a lost one deletes nothing unless its own
        // context is current already, as the same GL name may be another device's in the context current here (its
        // own goes with the context)
        let ours = match self.lost.get() {
            None => self.ready().is_ok(),
            Some(_) => self.is_current(),
        };
        if ours {
            // Safety: the device's own vertex array, deleted once, its own context current (checked just above, or
            // a borrowed device between enter and leave, which `ready` checks).
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
