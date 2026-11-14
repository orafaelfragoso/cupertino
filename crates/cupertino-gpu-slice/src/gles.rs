//! The EGL, GLES and GBM entry points the slice uses, loaded at run time, and the two buffer allocators: GBM buffer
//! objects on a hardware device and dmabuf export of a texture where there is no GBM device.

// entry points are looked up by name and cast to the type their specification gives
#![allow(
    non_snake_case,
    clippy::missing_safety_doc,
    clippy::missing_transmute_annotations
)]

use std::ffi::{c_char, c_int, c_uint, c_void, CStr, CString};
use std::os::fd::{FromRawFd, OwnedFd, RawFd};
use std::ptr::{null, null_mut};

use libloading::Library;

pub type EGLDisplay = *mut c_void;
pub type EGLContext = *mut c_void;
pub type EGLImage = *mut c_void;
pub type EGLSync = *mut c_void;
pub type EGLint = i32;
pub type GLuint = c_uint;
pub type GLenum = c_uint;

const EGL_NONE: EGLint = 0x3038;
const EGL_EXTENSIONS: EGLint = 0x3055;
const EGL_OPENGL_ES_API: c_uint = 0x30A0;
const EGL_CONTEXT_MAJOR_VERSION: EGLint = 0x3098;
const EGL_CONTEXT_MINOR_VERSION: EGLint = 0x30FB;
const EGL_CONTEXT_OPENGL_ROBUST_ACCESS_EXT: EGLint = 0x30BF;
const EGL_CONTEXT_OPENGL_RESET_NOTIFICATION_STRATEGY_EXT: EGLint = 0x3138;
const EGL_LOSE_CONTEXT_ON_RESET_EXT: EGLint = 0x31BF;
const EGL_GENERATE_RESET_ON_VIDEO_MEMORY_PURGE_NV: EGLint = 0x334C;
const EGL_TRUE: EGLint = 1;
const EGL_PLATFORM_GBM_KHR: c_uint = 0x31D7;
const EGL_PLATFORM_SURFACELESS_MESA: c_uint = 0x31DD;
const EGL_SYNC_NATIVE_FENCE_ANDROID: c_uint = 0x3144;
const EGL_NO_NATIVE_FENCE_FD_ANDROID: EGLint = -1;
const EGL_LINUX_DMA_BUF_EXT: c_uint = 0x3270;
const EGL_GL_TEXTURE_2D_KHR: c_uint = 0x30B1;
const EGL_WIDTH: EGLint = 0x3057;
const EGL_HEIGHT: EGLint = 0x3056;
const EGL_LINUX_DRM_FOURCC_EXT: EGLint = 0x3271;
const EGL_DMA_BUF_PLANE0_FD_EXT: EGLint = 0x3272;
const EGL_DMA_BUF_PLANE0_OFFSET_EXT: EGLint = 0x3273;
const EGL_DMA_BUF_PLANE0_PITCH_EXT: EGLint = 0x3274;
const EGL_DMA_BUF_PLANE0_MODIFIER_LO_EXT: EGLint = 0x3443;
const EGL_DMA_BUF_PLANE0_MODIFIER_HI_EXT: EGLint = 0x3444;
const EGL_PLANE_STEP: EGLint = 3;
const EGL_MODIFIER_STEP: EGLint = 2;

const GL_TEXTURE_2D: GLenum = 0x0DE1;
const GL_RGBA8: GLenum = 0x8058;
const GL_FRAMEBUFFER: GLenum = 0x8D40;
const GL_COLOR_ATTACHMENT0: GLenum = 0x8CE0;
const GL_FRAMEBUFFER_COMPLETE: GLenum = 0x8CD5;
const GL_COLOR_BUFFER_BIT: c_uint = 0x4000;
const GL_SCISSOR_TEST: GLenum = 0x0C11;
const GL_RENDERER: GLenum = 0x1F01;

/// `ARGB8888`, the buffer format the slice draws: blue, green, red, alpha in memory.
pub const FORMAT_ARGB8888: u32 = u32::from_le_bytes(*b"AR24");
/// The linear layout, which a hardware driver never gets unless it offers it itself.
pub const MODIFIER_LINEAR: u64 = 0;
/// No explicit modifier: the driver's implicit layout.
pub const MODIFIER_INVALID: u64 = 0x00ff_ffff_ffff_ffff;

const GBM_BO_USE_RENDERING: u32 = 1 << 2;

macro_rules! entry_points {
    ($name:ident { $($f:ident : fn($($a:ty),*) $(-> $r:ty)?;)* }) => {
        struct $name { $($f: unsafe extern "C" fn($($a),*) $(-> $r)?,)* }
        impl $name {
            unsafe fn load(lib: &Library) -> Result<$name, String> {
                Ok($name { $($f: *lib.get(concat!(stringify!($f), "\0").as_bytes()).map_err(|e| format!("{}: {e}", stringify!($f)))?,)* })
            }
        }
    };
}

entry_points!(Egl {
    eglGetProcAddress: fn(*const c_char) -> *mut c_void;
    eglInitialize: fn(EGLDisplay, *mut EGLint, *mut EGLint) -> c_uint;
    eglTerminate: fn(EGLDisplay) -> c_uint;
    eglQueryString: fn(EGLDisplay, EGLint) -> *const c_char;
    eglBindAPI: fn(c_uint) -> c_uint;
    eglCreateContext: fn(EGLDisplay, *mut c_void, EGLContext, *const EGLint) -> EGLContext;
    eglDestroyContext: fn(EGLDisplay, EGLContext) -> c_uint;
    eglMakeCurrent: fn(EGLDisplay, *mut c_void, *mut c_void, EGLContext) -> c_uint;
    eglGetError: fn() -> EGLint;
});

entry_points!(Gl {
    glGetString: fn(GLenum) -> *const u8;
    glGenTextures: fn(i32, *mut GLuint);
    glDeleteTextures: fn(i32, *const GLuint);
    glBindTexture: fn(GLenum, GLuint);
    glTexStorage2D: fn(GLenum, i32, GLenum, i32, i32);
    glGenFramebuffers: fn(i32, *mut GLuint);
    glDeleteFramebuffers: fn(i32, *const GLuint);
    glBindFramebuffer: fn(GLenum, GLuint);
    glFramebufferTexture2D: fn(GLenum, GLenum, GLenum, GLuint, i32);
    glCheckFramebufferStatus: fn(GLenum) -> GLenum;
    glViewport: fn(i32, i32, i32, i32);
    glScissor: fn(i32, i32, i32, i32);
    glEnable: fn(GLenum);
    glDisable: fn(GLenum);
    glClearColor: fn(f32, f32, f32, f32);
    glClear: fn(c_uint);
    glFlush: fn();
    glFinish: fn();
});

entry_points!(Gbm {
    gbm_create_device: fn(c_int) -> *mut c_void;
    gbm_device_destroy: fn(*mut c_void);
    gbm_bo_create_with_modifiers2: fn(*mut c_void, u32, u32, u32, *const u64, c_uint, u32) -> *mut c_void;
    gbm_bo_create: fn(*mut c_void, u32, u32, u32, u32) -> *mut c_void;
    gbm_bo_get_plane_count: fn(*mut c_void) -> c_int;
    gbm_bo_get_fd_for_plane: fn(*mut c_void, c_int) -> c_int;
    gbm_bo_get_stride_for_plane: fn(*mut c_void, c_int) -> u32;
    gbm_bo_get_offset: fn(*mut c_void, c_int) -> u32;
    gbm_bo_get_modifier: fn(*mut c_void) -> u64;
    gbm_bo_destroy: fn(*mut c_void);
});

type GetPlatformDisplay = unsafe extern "C" fn(c_uint, *mut c_void, *const EGLint) -> EGLDisplay;
type CreateImage =
    unsafe extern "C" fn(EGLDisplay, EGLContext, c_uint, *mut c_void, *const EGLint) -> EGLImage;
type DestroyImage = unsafe extern "C" fn(EGLDisplay, EGLImage) -> c_uint;
type ImageTargetTexture = unsafe extern "C" fn(GLenum, EGLImage);
type QueryModifiers =
    unsafe extern "C" fn(EGLDisplay, EGLint, EGLint, *mut u64, *mut c_uint, *mut EGLint) -> c_uint;
type CreateSync = unsafe extern "C" fn(EGLDisplay, c_uint, *const EGLint) -> EGLSync;
type DestroySync = unsafe extern "C" fn(EGLDisplay, EGLSync) -> c_uint;
type DupNativeFence = unsafe extern "C" fn(EGLDisplay, EGLSync) -> EGLint;
type ExportQuery =
    unsafe extern "C" fn(EGLDisplay, EGLImage, *mut c_int, *mut c_int, *mut u64) -> c_uint;
type Export =
    unsafe extern "C" fn(EGLDisplay, EGLImage, *mut c_int, *mut EGLint, *mut EGLint) -> c_uint;

struct Extensions {
    create_image: CreateImage,
    destroy_image: DestroyImage,
    image_target_texture: ImageTargetTexture,
    query_modifiers: Option<QueryModifiers>,
    create_sync: CreateSync,
    destroy_sync: DestroySync,
    dup_native_fence: DupNativeFence,
    export_query: Option<ExportQuery>,
    export: Option<Export>,
}

/// How the slice's buffers are allocated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Allocator {
    /// GBM buffer objects on the render node.
    Gbm,
    /// A texture exported as a dmabuf (`EGL_MESA_image_dma_buf_export`), where there is no GBM device.
    Export,
}

impl Allocator {
    pub fn name(self) -> &'static str {
        match self {
            Allocator::Gbm => "gbm",
            Allocator::Export => "export",
        }
    }
}

/// One plane of a dmabuf, as `zwp_linux_buffer_params_v1.add` takes it.
pub struct Plane {
    pub fd: OwnedFd,
    pub offset: u32,
    pub stride: u32,
}

/// A buffer the slice draws into: its dmabuf planes and the framebuffer bound to it.
pub struct Image {
    pub planes: Vec<Plane>,
    pub modifier: u64,
    pub size: (i32, i32),
    texture: GLuint,
    framebuffer: GLuint,
    image: EGLImage,
    bo: *mut c_void,
}

/// A GLES 3 context, current on this thread, on the GBM device of a render node or on the surfaceless platform.
pub struct Gpu {
    _libs: Vec<Library>,
    egl: Egl,
    gl: Gl,
    gbm: Option<(Gbm, *mut c_void)>,
    ext: Extensions,
    display: EGLDisplay,
    context: EGLContext,
    pub renderer: String,
    pub allocator: Allocator,
}

fn text(p: *const c_char) -> String {
    if p.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
    }
}

impl Gpu {
    /// A context on the GBM device of `node` (an open render node), or, without one or with `export` asked for, on
    /// the surfaceless platform with texture export.
    pub fn new(node: Option<RawFd>, prefer_export: bool) -> Result<Gpu, String> {
        unsafe {
            let egl_lib = Library::new("libEGL.so.1").map_err(|e| format!("libEGL: {e}"))?;
            let gl_lib = Library::new("libGLESv2.so.2").map_err(|e| format!("libGLESv2: {e}"))?;
            let egl = Egl::load(&egl_lib)?;
            let gl = Gl::load(&gl_lib)?;
            let proc = |name: &str| {
                let c = CString::new(name).unwrap();
                let p = (egl.eglGetProcAddress)(c.as_ptr());
                (!p.is_null()).then_some(p)
            };
            let get_display: GetPlatformDisplay = std::mem::transmute(
                proc("eglGetPlatformDisplayEXT").ok_or("no eglGetPlatformDisplayEXT")?,
            );
            let mut libs = vec![egl_lib, gl_lib];
            let mut gbm = None;
            let mut display = null_mut();
            if let (Some(fd), false) = (node, prefer_export) {
                let lib = Library::new("libgbm.so.1").map_err(|e| format!("libgbm: {e}"))?;
                let g = Gbm::load(&lib)?;
                let device = (g.gbm_create_device)(fd);
                if !device.is_null() {
                    display = get_display(EGL_PLATFORM_GBM_KHR, device, null());
                    gbm = Some((g, device));
                }
                libs.push(lib);
            }
            if display.is_null() {
                gbm = None;
                display = get_display(EGL_PLATFORM_SURFACELESS_MESA, null_mut(), null());
            }
            if display.is_null() {
                return Err(format!("no EGL display: {:#x}", (egl.eglGetError)()));
            }
            let (mut major, mut minor) = (0, 0);
            if (egl.eglInitialize)(display, &mut major, &mut minor) == 0 {
                return Err(format!("eglInitialize: {:#x}", (egl.eglGetError)()));
            }
            let extensions = text((egl.eglQueryString)(display, EGL_EXTENSIONS));
            let has = |e: &str| extensions.split_whitespace().any(|x| x == e);
            (egl.eglBindAPI)(EGL_OPENGL_ES_API);
            let mut attribs = vec![EGL_CONTEXT_MAJOR_VERSION, 3, EGL_CONTEXT_MINOR_VERSION, 0];
            if has("EGL_EXT_create_context_robustness") {
                attribs.extend([
                    EGL_CONTEXT_OPENGL_ROBUST_ACCESS_EXT,
                    EGL_TRUE,
                    EGL_CONTEXT_OPENGL_RESET_NOTIFICATION_STRATEGY_EXT,
                    EGL_LOSE_CONTEXT_ON_RESET_EXT,
                ]);
                if has("EGL_NV_robustness_video_memory_purge") {
                    attribs.extend([EGL_GENERATE_RESET_ON_VIDEO_MEMORY_PURGE_NV, EGL_TRUE]);
                }
            }
            attribs.push(EGL_NONE);
            let context = (egl.eglCreateContext)(display, null_mut(), null_mut(), attribs.as_ptr());
            if context.is_null() {
                return Err(format!("eglCreateContext: {:#x}", (egl.eglGetError)()));
            }
            if (egl.eglMakeCurrent)(display, null_mut(), null_mut(), context) == 0 {
                return Err(format!("eglMakeCurrent: {:#x}", (egl.eglGetError)()));
            }
            let need = |name: &str| proc(name).ok_or_else(|| format!("no {name}"));
            let ext = Extensions {
                create_image: std::mem::transmute(need("eglCreateImageKHR")?),
                destroy_image: std::mem::transmute(need("eglDestroyImageKHR")?),
                image_target_texture: std::mem::transmute(need("glEGLImageTargetTexture2DOES")?),
                query_modifiers: proc("eglQueryDmaBufModifiersEXT").map(|p| std::mem::transmute(p)),
                create_sync: std::mem::transmute(need("eglCreateSyncKHR")?),
                destroy_sync: std::mem::transmute(need("eglDestroySyncKHR")?),
                dup_native_fence: std::mem::transmute(need("eglDupNativeFenceFDANDROID")?),
                export_query: proc("eglExportDMABUFImageQueryMESA").map(|p| std::mem::transmute(p)),
                export: proc("eglExportDMABUFImageMESA").map(|p| std::mem::transmute(p)),
            };
            if !has("EGL_ANDROID_native_fence_sync") {
                return Err("the driver has no native fences".into());
            }
            let renderer = text((gl.glGetString)(GL_RENDERER).cast());
            let allocator = if gbm.is_some() {
                Allocator::Gbm
            } else {
                Allocator::Export
            };
            if allocator == Allocator::Export && !has("EGL_MESA_image_dma_buf_export") {
                return Err("no GBM device and no texture export".into());
            }
            Ok(Gpu {
                _libs: libs,
                egl,
                gl,
                gbm,
                ext,
                display,
                context,
                renderer,
                allocator,
            })
        }
    }

    /// Whether the renderer is a software rasterizer.
    pub fn software(&self) -> bool {
        let r = self.renderer.to_ascii_lowercase();
        ["llvmpipe", "softpipe", "swrast", "lavapipe"]
            .iter()
            .any(|s| r.contains(s))
    }

    /// The modifiers of `format` the driver can render to (not those it can only sample through an external image).
    pub fn renderable_modifiers(&self, format: u32) -> Vec<u64> {
        let Some(query) = self.ext.query_modifiers else {
            return Vec::new();
        };
        unsafe {
            let mut n = 0;
            if query(
                self.display,
                format as EGLint,
                0,
                null_mut(),
                null_mut(),
                &mut n,
            ) == 0
                || n <= 0
            {
                return Vec::new();
            }
            let mut modifiers = vec![0u64; n as usize];
            let mut external = vec![0u32; n as usize];
            query(
                self.display,
                format as EGLint,
                n,
                modifiers.as_mut_ptr(),
                external.as_mut_ptr(),
                &mut n,
            );
            modifiers
                .into_iter()
                .zip(external)
                .take(n.max(0) as usize)
                .filter(|&(_, external)| external == 0)
                .map(|(m, _)| m)
                .collect()
        }
    }

    /// A `size` buffer of `format` with one of `modifiers` (GBM picks the driver's preferred among them), or, through
    /// texture export, the layout the driver chooses, which must be among `modifiers` (`MODIFIER_INVALID` in it
    /// accepting the implicit layout).
    pub fn allocate(
        &self,
        size: (i32, i32),
        format: u32,
        modifiers: &[u64],
    ) -> Result<Image, String> {
        unsafe {
            let (w, h) = size;
            let gl = &self.gl;
            match &self.gbm {
                Some((gbm, device)) => {
                    let explicit: Vec<u64> = modifiers
                        .iter()
                        .copied()
                        .filter(|&m| m != MODIFIER_INVALID)
                        .collect();
                    let bo = if !explicit.is_empty() {
                        (gbm.gbm_bo_create_with_modifiers2)(
                            *device,
                            w as u32,
                            h as u32,
                            format,
                            explicit.as_ptr(),
                            explicit.len() as c_uint,
                            GBM_BO_USE_RENDERING,
                        )
                    } else if modifiers.contains(&MODIFIER_INVALID) {
                        (gbm.gbm_bo_create)(
                            *device,
                            w as u32,
                            h as u32,
                            format,
                            GBM_BO_USE_RENDERING,
                        )
                    } else {
                        return Err("no modifier both offered and renderable".into());
                    };
                    if bo.is_null() {
                        return Err("gbm_bo_create failed".into());
                    }
                    let modifier = if explicit.is_empty() {
                        MODIFIER_INVALID
                    } else {
                        (gbm.gbm_bo_get_modifier)(bo)
                    };
                    let count = (gbm.gbm_bo_get_plane_count)(bo).max(1);
                    let mut planes = Vec::new();
                    for i in 0..count {
                        let fd = (gbm.gbm_bo_get_fd_for_plane)(bo, i);
                        if fd < 0 {
                            (gbm.gbm_bo_destroy)(bo);
                            return Err("gbm_bo_get_fd_for_plane failed".into());
                        }
                        planes.push(Plane {
                            fd: OwnedFd::from_raw_fd(fd),
                            offset: (gbm.gbm_bo_get_offset)(bo, i),
                            stride: (gbm.gbm_bo_get_stride_for_plane)(bo, i),
                        });
                    }
                    let image = self.import(size, format, modifier, &planes)?;
                    let texture = self.texture_of(image);
                    let framebuffer = self.framebuffer(texture)?;
                    Ok(Image {
                        planes,
                        modifier,
                        size,
                        texture,
                        framebuffer,
                        image,
                        bo,
                    })
                }
                None => {
                    let mut texture = 0;
                    (gl.glGenTextures)(1, &mut texture);
                    (gl.glBindTexture)(GL_TEXTURE_2D, texture);
                    (gl.glTexStorage2D)(GL_TEXTURE_2D, 1, GL_RGBA8, w, h);
                    let image = (self.ext.create_image)(
                        self.display,
                        self.context,
                        EGL_GL_TEXTURE_2D_KHR,
                        texture as usize as *mut c_void,
                        [EGL_NONE].as_ptr(),
                    );
                    if image.is_null() {
                        return Err(format!(
                            "eglCreateImage of a texture: {:#x}",
                            (self.egl.eglGetError)()
                        ));
                    }
                    let (query, export) =
                        (self.ext.export_query.unwrap(), self.ext.export.unwrap());
                    let (mut fourcc, mut count, mut modifier) = (0, 0, 0u64);
                    if query(self.display, image, &mut fourcc, &mut count, &mut modifier) == 0
                        || count != 1
                    {
                        return Err(format!("the texture cannot be exported ({count} planes)"));
                    }
                    if !modifiers.contains(&modifier) {
                        return Err(format!("the exported layout {modifier:#x} is not offered"));
                    }
                    let (mut fd, mut stride, mut offset) = (-1, 0, 0);
                    if export(self.display, image, &mut fd, &mut stride, &mut offset) == 0 || fd < 0
                    {
                        return Err(format!(
                            "eglExportDMABUFImage: {:#x}",
                            (self.egl.eglGetError)()
                        ));
                    }
                    let planes = vec![Plane {
                        fd: OwnedFd::from_raw_fd(fd),
                        offset: offset as u32,
                        stride: stride as u32,
                    }];
                    let framebuffer = self.framebuffer(texture)?;
                    Ok(Image {
                        planes,
                        modifier,
                        size,
                        texture,
                        framebuffer,
                        image,
                        bo: null_mut(),
                    })
                }
            }
        }
    }

    /// The format the export allocator's textures come out in (the driver's choice, asked of one small texture).
    pub fn export_format(&self) -> Option<(u32, u64)> {
        let (query, _) = (self.ext.export_query?, self.ext.export?);
        unsafe {
            let gl = &self.gl;
            let mut texture = 0;
            (gl.glGenTextures)(1, &mut texture);
            (gl.glBindTexture)(GL_TEXTURE_2D, texture);
            (gl.glTexStorage2D)(GL_TEXTURE_2D, 1, GL_RGBA8, 16, 16);
            let image = (self.ext.create_image)(
                self.display,
                self.context,
                EGL_GL_TEXTURE_2D_KHR,
                texture as usize as *mut c_void,
                [EGL_NONE].as_ptr(),
            );
            let mut out = None;
            if !image.is_null() {
                let (mut fourcc, mut count, mut modifier) = (0, 0, 0u64);
                if query(self.display, image, &mut fourcc, &mut count, &mut modifier) != 0 {
                    out = Some((fourcc as u32, modifier));
                }
                (self.ext.destroy_image)(self.display, image);
            }
            (gl.glDeleteTextures)(1, &texture);
            out
        }
    }

    unsafe fn import(
        &self,
        size: (i32, i32),
        format: u32,
        modifier: u64,
        planes: &[Plane],
    ) -> Result<EGLImage, String> {
        use std::os::fd::AsRawFd;
        let mut attribs = vec![
            EGL_WIDTH,
            size.0,
            EGL_HEIGHT,
            size.1,
            EGL_LINUX_DRM_FOURCC_EXT,
            format as EGLint,
        ];
        for (i, plane) in planes.iter().enumerate() {
            let i = i as EGLint;
            attribs.extend([
                EGL_DMA_BUF_PLANE0_FD_EXT + i * EGL_PLANE_STEP,
                plane.fd.as_raw_fd(),
                EGL_DMA_BUF_PLANE0_OFFSET_EXT + i * EGL_PLANE_STEP,
                plane.offset as EGLint,
                EGL_DMA_BUF_PLANE0_PITCH_EXT + i * EGL_PLANE_STEP,
                plane.stride as EGLint,
            ]);
            if modifier != MODIFIER_INVALID {
                attribs.extend([
                    EGL_DMA_BUF_PLANE0_MODIFIER_LO_EXT + i * EGL_MODIFIER_STEP,
                    (modifier & 0xffff_ffff) as EGLint,
                    EGL_DMA_BUF_PLANE0_MODIFIER_HI_EXT + i * EGL_MODIFIER_STEP,
                    (modifier >> 32) as EGLint,
                ]);
            }
        }
        attribs.push(EGL_NONE);
        let image = (self.ext.create_image)(
            self.display,
            null_mut(),
            EGL_LINUX_DMA_BUF_EXT,
            null_mut(),
            attribs.as_ptr(),
        );
        if image.is_null() {
            return Err(format!(
                "eglCreateImage of the dmabuf: {:#x}",
                (self.egl.eglGetError)()
            ));
        }
        Ok(image)
    }

    unsafe fn texture_of(&self, image: EGLImage) -> GLuint {
        let mut texture = 0;
        (self.gl.glGenTextures)(1, &mut texture);
        (self.gl.glBindTexture)(GL_TEXTURE_2D, texture);
        (self.ext.image_target_texture)(GL_TEXTURE_2D, image);
        texture
    }

    unsafe fn framebuffer(&self, texture: GLuint) -> Result<GLuint, String> {
        let gl = &self.gl;
        let mut framebuffer = 0;
        (gl.glGenFramebuffers)(1, &mut framebuffer);
        (gl.glBindFramebuffer)(GL_FRAMEBUFFER, framebuffer);
        (gl.glFramebufferTexture2D)(
            GL_FRAMEBUFFER,
            GL_COLOR_ATTACHMENT0,
            GL_TEXTURE_2D,
            texture,
            0,
        );
        if (gl.glCheckFramebufferStatus)(GL_FRAMEBUFFER) != GL_FRAMEBUFFER_COMPLETE {
            return Err("the dmabuf cannot be rendered to".into());
        }
        Ok(framebuffer)
    }

    /// Draw `rects` (x, y, w, h in buffer pixels from the top left, and a premultiplied colour) in order into `image`.
    pub fn draw(&self, image: &Image, rects: &[([i32; 4], [f32; 4])]) {
        unsafe {
            let gl = &self.gl;
            (gl.glBindFramebuffer)(GL_FRAMEBUFFER, image.framebuffer);
            (gl.glViewport)(0, 0, image.size.0, image.size.1);
            (gl.glEnable)(GL_SCISSOR_TEST);
            for &([x, y, w, h], [r, g, b, a]) in rects {
                // GL's rows count from the bottom; the buffer's first row is the top of the window
                (gl.glScissor)(x, image.size.1 - y - h, w, h);
                (gl.glClearColor)(r, g, b, a);
                (gl.glClear)(GL_COLOR_BUFFER_BIT);
            }
            (gl.glDisable)(GL_SCISSOR_TEST);
        }
    }

    /// A native fence after the work so far, flushed: a sync file that signals when the GPU has finished it.
    pub fn fence(&self) -> Result<OwnedFd, String> {
        unsafe {
            let attribs = [EGL_NONE];
            let sync = (self.ext.create_sync)(
                self.display,
                EGL_SYNC_NATIVE_FENCE_ANDROID,
                attribs.as_ptr(),
            );
            if sync.is_null() {
                return Err(format!("eglCreateSync: {:#x}", (self.egl.eglGetError)()));
            }
            (self.gl.glFlush)();
            let fd = (self.ext.dup_native_fence)(self.display, sync);
            (self.ext.destroy_sync)(self.display, sync);
            if fd == EGL_NO_NATIVE_FENCE_FD_ANDROID {
                return Err(format!(
                    "eglDupNativeFenceFD: {:#x}",
                    (self.egl.eglGetError)()
                ));
            }
            Ok(OwnedFd::from_raw_fd(fd))
        }
    }

    /// Wait for all work so far on the CPU, the way a renderer without fences would.
    pub fn finish(&self) {
        unsafe { (self.gl.glFinish)() }
    }

    pub fn free(&self, image: Image) {
        unsafe {
            (self.gl.glDeleteFramebuffers)(1, &image.framebuffer);
            (self.gl.glDeleteTextures)(1, &image.texture);
            if !image.image.is_null() {
                (self.ext.destroy_image)(self.display, image.image);
            }
            if let (Some((gbm, _)), false) = (&self.gbm, image.bo.is_null()) {
                (gbm.gbm_bo_destroy)(image.bo);
            }
        }
    }
}

impl Drop for Gpu {
    fn drop(&mut self) {
        unsafe {
            (self.egl.eglMakeCurrent)(self.display, null_mut(), null_mut(), null_mut());
            (self.egl.eglDestroyContext)(self.display, self.context);
            (self.egl.eglTerminate)(self.display);
            if let Some((gbm, device)) = &self.gbm {
                (gbm.gbm_device_destroy)(*device);
            }
        }
    }
}
