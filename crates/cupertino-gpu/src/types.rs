//! The backend-neutral descriptions, errors and values of the API.

use std::fmt;
use std::os::fd::OwnedFd;
use std::path::PathBuf;

/// Why a call failed.
#[derive(Debug)]
pub enum Error {
    /// A library or platform is missing or cannot start (EGL, GLES, GBM, the device): draw on the CPU.
    Unavailable(String),
    /// The device lacks what the call needs (texture export, native fences, a format).
    Unsupported(String),
    /// The context was reset or its video memory purged; make a new device.
    Lost(Loss),
    /// A cap would be exceeded: `need` more bytes than the `cap` of `what` allows.
    OverBudget {
        what: &'static str,
        need: u64,
        cap: u64,
    },
    /// The call's arguments do not fit what they name.
    Invalid(String),
    /// The driver refused the call.
    Driver(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Unavailable(s) => write!(f, "unavailable: {s}"),
            Error::Unsupported(s) => write!(f, "unsupported: {s}"),
            Error::Lost(loss) => write!(f, "device lost: {loss:?}"),
            Error::OverBudget { what, need, cap } => {
                write!(f, "{what}: {need} bytes more than the cap of {cap} allows")
            }
            Error::Invalid(s) => write!(f, "invalid: {s}"),
            Error::Driver(s) => write!(f, "driver: {s}"),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

/// How a device was lost.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Loss {
    /// The context was reset: by its own work (`guilty`), by another context's, or for a reason unknown (`None`).
    Reset { guilty: Option<bool> },
    /// The driver lost the video memory (suspend, a VT switch, a mode set). Not a fault of the work: never counted
    /// toward falling back to the CPU.
    Purge,
}

impl Loss {
    /// Whether this loss counts toward falling back to the CPU (resets do, purges do not).
    pub fn counts_as_failure(self) -> bool {
        matches!(self, Loss::Reset { .. })
    }
}

/// Where a device is made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Platform {
    /// The GBM device of a render node, allocating GBM buffer objects.
    RenderNode(PathBuf),
    /// No window system and no GBM: the driver's default device, allocating by texture export.
    Surfaceless,
}

/// A device to make.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceDesc {
    pub platform: Platform,
}

/// How a device allocates dmabufs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Allocator {
    /// GBM buffer objects on the render node.
    Gbm,
    /// A texture exported as a dmabuf, where there is no GBM device.
    Export,
}

/// What a device is and can do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceInfo {
    /// The driver's name for the renderer.
    pub renderer: String,
    /// Whether it is a software rasterizer.
    pub software: bool,
    /// How it allocates dmabufs, if it can.
    pub allocator: Option<Allocator>,
    /// Whether it exports native fences (sync files).
    pub native_fences: bool,
    /// Whether it waits on imported native fences on the GPU.
    pub fence_import: bool,
    /// Whether its context was made with robustness (reset notification).
    pub robustness: bool,
    /// Whether its context reports video-memory purges.
    pub purge_notification: bool,
    /// Whether it renders to 16-bit float targets.
    pub half_float_targets: bool,
    /// Whether it renders to 32-bit float targets.
    pub float_targets: bool,
    /// The largest texture side.
    pub max_texture_size: u32,
}

/// The formats of textures.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TextureFormat {
    /// One 8-bit channel: coverage and masks.
    R8,
    /// Four 8-bit channels, premultiplied colour.
    Rgba8,
    /// Four 16-bit float channels: blur levels where the device renders to them.
    Rgba16Float,
    /// Four 32-bit float channels: tests and measurement.
    Rgba32Float,
}

impl TextureFormat {
    /// Bytes a pixel takes.
    pub fn bytes_per_pixel(self) -> u32 {
        match self {
            TextureFormat::R8 => 1,
            TextureFormat::Rgba8 => 4,
            TextureFormat::Rgba16Float => 8,
            TextureFormat::Rgba32Float => 16,
        }
    }
}

/// What a texture is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct TextureUsage {
    pub sampled: bool,
    pub render_target: bool,
}

impl TextureUsage {
    pub const SAMPLED: TextureUsage = TextureUsage {
        sampled: true,
        render_target: false,
    };
    pub const TARGET: TextureUsage = TextureUsage {
        sampled: true,
        render_target: true,
    };
}

/// A texture to make.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextureDesc {
    pub size: (u32, u32),
    pub format: TextureFormat,
    pub usage: TextureUsage,
}

/// A rectangle of pixels: x, y from the first row and column, width, height.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Region {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Region {
    pub fn new(x: u32, y: u32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
}

/// What a buffer holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BufferUsage {
    Uniform,
    Vertex,
    Index,
}

/// A buffer to make.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BufferDesc {
    pub size: u64,
    pub usage: BufferUsage,
}

/// How a sampler filters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Filter {
    Nearest,
    Linear,
}

/// What a sampler reads past a texture's edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddressMode {
    ClampToEdge,
    Repeat,
}

/// A sampler to make.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SamplerDesc {
    pub filter: Filter,
    pub address: AddressMode,
}

/// What a binding of a bind group holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BindingKind {
    UniformBuffer,
    Texture,
    Sampler,
}

/// One binding of a bind group layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BindingLayout {
    pub binding: u32,
    pub kind: BindingKind,
}

/// The format of a vertex attribute.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VertexFormat {
    Float32x2,
    Float32x4,
    Unorm8x4,
}

impl VertexFormat {
    pub fn size(self) -> u32 {
        match self {
            VertexFormat::Float32x2 => 8,
            VertexFormat::Float32x4 => 16,
            VertexFormat::Unorm8x4 => 4,
        }
    }
}

/// One attribute of a vertex buffer, at the shader's location.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VertexAttribute {
    pub location: u32,
    pub format: VertexFormat,
    pub offset: u32,
}

/// Whether a vertex buffer advances per vertex or per instance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepMode {
    Vertex,
    Instance,
}

/// The layout of one vertex buffer slot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VertexLayout {
    pub stride: u32,
    pub step: StepMode,
    pub attributes: Vec<VertexAttribute>,
}

/// How a pipeline blends into its target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Blend {
    /// The output replaces the target.
    Replace,
    /// Premultiplied source over: `src + dst · (1 − src.a)`.
    PremultipliedOver,
}

/// How vertices make primitives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Topology {
    TriangleList,
    TriangleStrip,
}

/// How a pass starts.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LoadOp {
    /// Every pixel set to this premultiplied colour.
    Clear([f32; 4]),
    /// The target's pixels kept.
    Load,
}

/// One plane of a dmabuf.
#[derive(Debug)]
pub struct DmabufPlane {
    pub fd: OwnedFd,
    pub offset: u32,
    pub stride: u32,
}

/// A dmabuf: its size, DRM format, modifier and planes.
#[derive(Debug)]
pub struct Dmabuf {
    pub size: (u32, u32),
    pub fourcc: u32,
    pub modifier: u64,
    pub planes: Vec<DmabufPlane>,
}

impl Dmabuf {
    /// The same buffer with duplicated file descriptors, to hand to another device or process.
    pub fn try_clone(&self) -> std::io::Result<Dmabuf> {
        Ok(Dmabuf {
            size: self.size,
            fourcc: self.fourcc,
            modifier: self.modifier,
            planes: self
                .planes
                .iter()
                .map(|p| {
                    Ok(DmabufPlane {
                        fd: p.fd.try_clone()?,
                        offset: p.offset,
                        stride: p.stride,
                    })
                })
                .collect::<std::io::Result<_>>()?,
        })
    }
}

/// A dmabuf to allocate: size, DRM format, and the modifiers acceptable (offered by the compositor and renderable
/// by the device; [`MODIFIER_INVALID`] in it accepts the driver's implicit layout).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DmabufDesc {
    pub size: (u32, u32),
    pub fourcc: u32,
    pub modifiers: Vec<u64>,
}

/// `ARGB8888`: blue, green, red, alpha in memory.
pub const FOURCC_ARGB8888: u32 = u32::from_le_bytes(*b"AR24");
/// `ABGR8888`: red, green, blue, alpha in memory.
pub const FOURCC_ABGR8888: u32 = u32::from_le_bytes(*b"AB24");
/// The linear layout, never added on a hardware driver unless it offers it itself.
pub const MODIFIER_LINEAR: u64 = 0;
/// No explicit modifier: the driver's implicit layout.
pub const MODIFIER_INVALID: u64 = 0x00ff_ffff_ffff_ffff;
