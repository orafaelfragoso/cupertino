//! EGL and GBM, loaded at run time: the display and context of a device, its extensions, the two dmabuf allocators
//! (GBM buffer objects, texture export) and native fences.

// entry points are looked up by name and cast to the type their specification gives
#![allow(non_snake_case, clippy::missing_transmute_annotations)]

use std::ffi::{c_char, c_int, c_uint, c_void, CStr, CString};
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd};
use std::ptr::{null, null_mut};

use libloading::Library;

use crate::types::{Error, Result, MODIFIER_INVALID};

pub type EGLDisplay = *mut c_void;
pub type EGLContext = *mut c_void;
pub type EGLImage = *mut c_void;
pub type EGLSync = *mut c_void;
pub type EGLint = i32;

pub const EGL_NONE: EGLint = 0x3038;
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
const EGL_SYNC_NATIVE_FENCE_FD_ANDROID: EGLint = 0x3145;
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
const GBM_BO_USE_RENDERING: u32 = 1 << 2;

macro_rules! entry_points {
    ($name:ident { $($f:ident : fn($($a:ty),*) $(-> $r:ty)?;)* }) => {
        pub struct $name { $(pub $f: unsafe extern "C" fn($($a),*) $(-> $r)?,)* }
        impl $name {
            /// Safety: `lib` must be the library whose entry points these are, with the signatures written here.
            unsafe fn load(lib: &Library) -> std::result::Result<$name, String> {
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
    eglGetCurrentDisplay: fn() -> EGLDisplay;
    eglGetCurrentContext: fn() -> EGLContext;
    eglGetError: fn() -> EGLint;
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
type QueryModifiers =
    unsafe extern "C" fn(EGLDisplay, EGLint, EGLint, *mut u64, *mut c_uint, *mut EGLint) -> c_uint;
type CreateSync = unsafe extern "C" fn(EGLDisplay, c_uint, *const EGLint) -> EGLSync;
type DestroySync = unsafe extern "C" fn(EGLDisplay, EGLSync) -> c_uint;
type DupNativeFence = unsafe extern "C" fn(EGLDisplay, EGLSync) -> EGLint;
type WaitSync = unsafe extern "C" fn(EGLDisplay, EGLSync, EGLint) -> EGLint;
type ExportQuery =
    unsafe extern "C" fn(EGLDisplay, EGLImage, *mut c_int, *mut c_int, *mut u64) -> c_uint;
type Export =
    unsafe extern "C" fn(EGLDisplay, EGLImage, *mut c_int, *mut EGLint, *mut EGLint) -> c_uint;

/// Native fences: create, destroy, export to a sync file, and wait on one on the GPU.
#[derive(Clone, Copy)]
struct Fences {
    create: CreateSync,
    destroy: DestroySync,
    dup: DupNativeFence,
    wait: Option<WaitSync>,
}

/// One plane of a buffer the allocators made, before it becomes a texture.
pub struct Plane {
    pub fd: OwnedFd,
    pub offset: u32,
    pub stride: u32,
}

/// An EGL display and context, owned (made here) or borrowed (someone else's, current when used).
pub struct Context {
    // the libraries stay loaded as long as their entry points can be called
    _libs: Vec<Library>,
    pub egl: Egl,
    gbm: Option<(Gbm, *mut c_void)>,
    // the render node the GBM device works on, which GBM does not keep open itself
    node: Option<OwnedFd>,
    pub display: EGLDisplay,
    pub context: EGLContext,
    owned: bool,
    extensions: String,
    create_image: Option<CreateImage>,
    destroy_image: Option<DestroyImage>,
    query_modifiers: Option<QueryModifiers>,
    fences: Option<Fences>,
    export_query: Option<ExportQuery>,
    export: Option<Export>,
    pub robust: bool,
    pub purge: bool,
}

fn text(p: *const c_char) -> String {
    if p.is_null() {
        String::new()
    } else {
        // Safety: EGL returns a nul-terminated string that lives as long as the display.
        unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
    }
}

fn open(name: &str) -> Result<Library> {
    // Safety: loading a system library runs its initialisers, which EGL, GLES and GBM keep free of side effects
    // beyond their own state.
    unsafe { Library::new(name) }.map_err(|e| Error::Unavailable(format!("{name}: {e}")))
}

impl Context {
    /// Load EGL and make a robust GLES 3.0 context, current on this thread, on the GBM device of the open render
    /// node `node`, or on the surfaceless platform without one.
    pub fn new(node: Option<OwnedFd>) -> Result<Context> {
        let egl_lib = open("libEGL.so.1")?;
        // Safety: the signatures are those of the EGL 1.5 specification.
        let egl = unsafe { Egl::load(&egl_lib) }.map_err(Error::Unavailable)?;
        let mut libs = vec![egl_lib];
        let proc = |name: &str| proc_address(&egl, name);
        let get_display: GetPlatformDisplay = match proc("eglGetPlatformDisplayEXT") {
            // Safety: the extension's entry point has this signature.
            Some(p) => unsafe { std::mem::transmute(p) },
            None => return Err(Error::Unavailable("no eglGetPlatformDisplayEXT".into())),
        };
        let mut gbm = None;
        let display = match &node {
            Some(fd) => {
                let lib = open("libgbm.so.1")?;
                // Safety: the signatures are those of gbm.h.
                let g = unsafe { Gbm::load(&lib) }.map_err(Error::Unavailable)?;
                libs.push(lib);
                // Safety: `fd` is an open render node; GBM keeps no ownership of it.
                let device = unsafe { (g.gbm_create_device)(fd.as_raw_fd()) };
                if device.is_null() {
                    return Err(Error::Unavailable("gbm_create_device failed".into()));
                }
                // Safety: a GBM device is the native display of the GBM platform.
                let display = unsafe { get_display(EGL_PLATFORM_GBM_KHR, device, null()) };
                gbm = Some((g, device));
                display
            }
            // Safety: the surfaceless platform takes no native display.
            None => unsafe { get_display(EGL_PLATFORM_SURFACELESS_MESA, null_mut(), null()) },
        };
        let fail = |egl: &Egl, what: &str| {
            // Safety: eglGetError has no preconditions.
            Error::Unavailable(format!("{what}: {:#x}", unsafe { (egl.eglGetError)() }))
        };
        if display.is_null() {
            let e = fail(&egl, "no EGL display");
            if let Some((g, device)) = gbm {
                // Safety: the device was made above and nothing else holds it.
                unsafe { (g.gbm_device_destroy)(device) };
            }
            return Err(e);
        }
        let (mut major, mut minor) = (0, 0);
        // Safety: `display` is a valid display of this EGL.
        if unsafe { (egl.eglInitialize)(display, &mut major, &mut minor) } == 0 {
            return Err(fail(&egl, "eglInitialize"));
        }
        // Safety: an initialised display.
        let extensions = text(unsafe { (egl.eglQueryString)(display, EGL_EXTENSIONS) });
        let has = |e: &str| extensions.split_whitespace().any(|x| x == e);
        let robust = has("EGL_EXT_create_context_robustness");
        let purge = robust && has("EGL_NV_robustness_video_memory_purge");
        let mut attribs = vec![EGL_CONTEXT_MAJOR_VERSION, 3, EGL_CONTEXT_MINOR_VERSION, 0];
        if robust {
            attribs.extend([
                EGL_CONTEXT_OPENGL_ROBUST_ACCESS_EXT,
                EGL_TRUE,
                EGL_CONTEXT_OPENGL_RESET_NOTIFICATION_STRATEGY_EXT,
                EGL_LOSE_CONTEXT_ON_RESET_EXT,
            ]);
            if purge {
                attribs.extend([EGL_GENERATE_RESET_ON_VIDEO_MEMORY_PURGE_NV, EGL_TRUE]);
            }
        }
        attribs.push(EGL_NONE);
        // Safety: an initialised display and a terminated attribute list.
        let context = unsafe {
            (egl.eglBindAPI)(EGL_OPENGL_ES_API);
            (egl.eglCreateContext)(display, null_mut(), null_mut(), attribs.as_ptr())
        };
        if context.is_null() {
            let e = fail(&egl, "eglCreateContext");
            // Safety: the display was initialised above.
            unsafe { (egl.eglTerminate)(display) };
            return Err(e);
        }
        // Safety: a context of this display, without surfaces (surfaceless contexts are core in EGL 1.5).
        if unsafe { (egl.eglMakeCurrent)(display, null_mut(), null_mut(), context) } == 0 {
            return Err(fail(&egl, "eglMakeCurrent"));
        }
        let mut ctx = Context {
            _libs: libs,
            egl,
            gbm,
            node,
            display,
            context,
            owned: true,
            extensions,
            create_image: None,
            destroy_image: None,
            query_modifiers: None,
            fences: None,
            export_query: None,
            export: None,
            robust,
            purge,
        };
        ctx.load_extensions();
        Ok(ctx)
    }

    /// The EGL display and context current on this thread, someone else's: nothing is made, and nothing is
    /// destroyed when the device goes. `robust` and `purge` say how the owner made its context.
    pub fn borrow_current(robust: bool, purge: bool) -> Result<Context> {
        let egl_lib = open("libEGL.so.1")?;
        // Safety: the signatures are those of the EGL 1.5 specification.
        let egl = unsafe { Egl::load(&egl_lib) }.map_err(Error::Unavailable)?;
        // Safety: these queries have no preconditions.
        let (display, context) =
            unsafe { ((egl.eglGetCurrentDisplay)(), (egl.eglGetCurrentContext)()) };
        if display.is_null() || context.is_null() {
            return Err(Error::Invalid("no EGL context is current to borrow".into()));
        }
        // Safety: the current display is initialised.
        let extensions = text(unsafe { (egl.eglQueryString)(display, EGL_EXTENSIONS) });
        let mut ctx = Context {
            _libs: vec![egl_lib],
            egl,
            gbm: None,
            node: None,
            display,
            context,
            owned: false,
            extensions,
            create_image: None,
            destroy_image: None,
            query_modifiers: None,
            fences: None,
            export_query: None,
            export: None,
            robust,
            purge,
        };
        ctx.load_extensions();
        Ok(ctx)
    }

    fn load_extensions(&mut self) {
        let proc = |name: &str| proc_address(&self.egl, name);
        let has = |e: &str| self.extensions.split_whitespace().any(|x| x == e);
        // Safety (every transmute here): each extension entry point has the signature its specification gives.
        unsafe {
            self.create_image = proc("eglCreateImageKHR").map(|p| std::mem::transmute(p));
            self.destroy_image = proc("eglDestroyImageKHR").map(|p| std::mem::transmute(p));
            self.query_modifiers =
                proc("eglQueryDmaBufModifiersEXT").map(|p| std::mem::transmute(p));
            self.fences = match (
                has("EGL_ANDROID_native_fence_sync"),
                proc("eglCreateSyncKHR"),
                proc("eglDestroySyncKHR"),
                proc("eglDupNativeFenceFDANDROID"),
            ) {
                (true, Some(c), Some(d), Some(e)) => Some(Fences {
                    create: std::mem::transmute(c),
                    destroy: std::mem::transmute(d),
                    dup: std::mem::transmute(e),
                    wait: has("EGL_KHR_wait_sync")
                        .then(|| proc("eglWaitSyncKHR"))
                        .flatten()
                        .map(|p| std::mem::transmute(p)),
                }),
                _ => None,
            };
            if has("EGL_MESA_image_dma_buf_export") {
                self.export_query =
                    proc("eglExportDMABUFImageQueryMESA").map(|p| std::mem::transmute(p));
                self.export = proc("eglExportDMABUFImageMESA").map(|p| std::mem::transmute(p));
            }
        }
    }

    /// The address of an EGL or GL entry point, or None.
    pub fn proc_address(&self, name: &str) -> Option<*mut c_void> {
        proc_address(&self.egl, name)
    }

    pub fn has_gbm(&self) -> bool {
        self.gbm.is_some()
    }

    pub fn can_export(&self) -> bool {
        self.export.is_some() && self.export_query.is_some() && self.create_image.is_some()
    }

    pub fn can_import(&self) -> bool {
        self.create_image.is_some() && has_ext(&self.extensions, "EGL_EXT_image_dma_buf_import")
    }

    pub fn native_fences(&self) -> bool {
        self.fences.is_some()
    }

    pub fn fence_import(&self) -> bool {
        self.fences.is_some_and(|f| f.wait.is_some())
    }

    pub fn error(&self, what: &str) -> Error {
        // Safety: eglGetError has no preconditions.
        Error::Driver(format!("{what}: {:#x}", unsafe {
            (self.egl.eglGetError)()
        }))
    }

    /// The modifiers of `fourcc` the driver renders to (not those it can only sample as external images).
    pub fn renderable_modifiers(&self, fourcc: u32) -> Vec<u64> {
        let Some(query) = self.query_modifiers else {
            return Vec::new();
        };
        let mut n = 0;
        // Safety: a size query first, then arrays of the size it gave.
        unsafe {
            if query(
                self.display,
                fourcc as EGLint,
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
                fourcc as EGLint,
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

    /// A GBM buffer object of `size` and `fourcc` with one of `modifiers`: its planes, its modifier, the object.
    pub fn gbm_allocate(
        &self,
        size: (u32, u32),
        fourcc: u32,
        modifiers: &[u64],
    ) -> Result<(Vec<Plane>, u64, *mut c_void)> {
        let (gbm, device) = self
            .gbm
            .as_ref()
            .ok_or_else(|| Error::Unsupported("no GBM device".into()))?;
        let explicit: Vec<u64> = modifiers
            .iter()
            .copied()
            .filter(|&m| m != MODIFIER_INVALID)
            .collect();
        // Safety: a live GBM device; the modifier list's pointer and length agree.
        unsafe {
            let bo = if !explicit.is_empty() {
                (gbm.gbm_bo_create_with_modifiers2)(
                    *device,
                    size.0,
                    size.1,
                    fourcc,
                    explicit.as_ptr(),
                    explicit.len() as c_uint,
                    GBM_BO_USE_RENDERING,
                )
            } else if modifiers.contains(&MODIFIER_INVALID) {
                (gbm.gbm_bo_create)(*device, size.0, size.1, fourcc, GBM_BO_USE_RENDERING)
            } else {
                return Err(Error::Unsupported(
                    "no modifier both offered and renderable".into(),
                ));
            };
            if bo.is_null() {
                return Err(Error::Driver("gbm_bo_create failed".into()));
            }
            let modifier = if explicit.is_empty() {
                MODIFIER_INVALID
            } else {
                (gbm.gbm_bo_get_modifier)(bo)
            };
            let mut planes = Vec::new();
            for i in 0..(gbm.gbm_bo_get_plane_count)(bo).max(1) {
                let fd = (gbm.gbm_bo_get_fd_for_plane)(bo, i);
                if fd < 0 {
                    (gbm.gbm_bo_destroy)(bo);
                    return Err(Error::Driver("gbm_bo_get_fd_for_plane failed".into()));
                }
                planes.push(Plane {
                    fd: OwnedFd::from_raw_fd(fd),
                    offset: (gbm.gbm_bo_get_offset)(bo, i),
                    stride: (gbm.gbm_bo_get_stride_for_plane)(bo, i),
                });
            }
            Ok((planes, modifier, bo))
        }
    }

    /// Destroy a buffer object [`Context::gbm_allocate`] made.
    ///
    /// Safety: `bo` came from this context's GBM device and is destroyed once.
    pub unsafe fn gbm_free(&self, bo: *mut c_void) {
        if let Some((gbm, _)) = &self.gbm {
            (gbm.gbm_bo_destroy)(bo);
        }
    }

    /// An EGL image of the dmabuf described.
    pub fn import(
        &self,
        size: (u32, u32),
        fourcc: u32,
        modifier: u64,
        planes: &[(i32, u32, u32)],
    ) -> Result<EGLImage> {
        let create = self
            .create_image
            .ok_or_else(|| Error::Unsupported("no eglCreateImage".into()))?;
        let mut attribs = vec![
            EGL_WIDTH,
            size.0 as EGLint,
            EGL_HEIGHT,
            size.1 as EGLint,
            EGL_LINUX_DRM_FOURCC_EXT,
            fourcc as EGLint,
        ];
        for (i, &(fd, offset, stride)) in planes.iter().enumerate() {
            let i = i as EGLint;
            attribs.extend([
                EGL_DMA_BUF_PLANE0_FD_EXT + i * EGL_PLANE_STEP,
                fd,
                EGL_DMA_BUF_PLANE0_OFFSET_EXT + i * EGL_PLANE_STEP,
                offset as EGLint,
                EGL_DMA_BUF_PLANE0_PITCH_EXT + i * EGL_PLANE_STEP,
                stride as EGLint,
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
        // Safety: a terminated attribute list naming open file descriptors; EGL duplicates what it keeps.
        let image = unsafe {
            create(
                self.display,
                null_mut(),
                EGL_LINUX_DMA_BUF_EXT,
                null_mut(),
                attribs.as_ptr(),
            )
        };
        if image.is_null() {
            return Err(self.error("eglCreateImage of a dmabuf"));
        }
        Ok(image)
    }

    /// The texture `texture` (a GL name of the current context) exported as a one-plane dmabuf: its image, plane,
    /// format and modifier.
    pub fn export(&self, texture: u32) -> Result<(EGLImage, Plane, u32, u64)> {
        let (Some(create), Some(query), Some(export)) =
            (self.create_image, self.export_query, self.export)
        else {
            return Err(Error::Unsupported("no texture export".into()));
        };
        // Safety: `texture` names a complete 2D texture of the current context, which is this one.
        unsafe {
            let image = create(
                self.display,
                self.context,
                EGL_GL_TEXTURE_2D_KHR,
                texture as usize as *mut c_void,
                [EGL_NONE].as_ptr(),
            );
            if image.is_null() {
                return Err(self.error("eglCreateImage of a texture"));
            }
            let (mut fourcc, mut count, mut modifier) = (0, 0, 0u64);
            if query(self.display, image, &mut fourcc, &mut count, &mut modifier) == 0 || count != 1
            {
                self.destroy_image(image);
                return Err(Error::Unsupported(format!(
                    "the texture cannot be exported ({count} planes)"
                )));
            }
            let (mut fd, mut stride, mut offset) = (-1, 0, 0);
            if export(self.display, image, &mut fd, &mut stride, &mut offset) == 0 || fd < 0 {
                self.destroy_image(image);
                return Err(self.error("eglExportDMABUFImage"));
            }
            let plane = Plane {
                fd: OwnedFd::from_raw_fd(fd),
                offset: offset as u32,
                stride: stride as u32,
            };
            Ok((image, plane, fourcc as u32, modifier))
        }
    }

    /// Destroy an image this context made.
    ///
    /// Safety: `image` came from this display and is destroyed once.
    pub unsafe fn destroy_image(&self, image: EGLImage) {
        if let Some(destroy) = self.destroy_image {
            destroy(self.display, image);
        }
    }

    /// A native fence after the work submitted so far, flushed by `flush`: a sync file that signals when the GPU
    /// has finished it.
    pub fn native_fence(&self, flush: impl FnOnce()) -> Result<OwnedFd> {
        let f = self
            .fences
            .ok_or_else(|| Error::Unsupported("the driver has no native fences".into()))?;
        // Safety: a terminated attribute list; the sync is destroyed once after its file is duplicated.
        unsafe {
            let sync = (f.create)(
                self.display,
                EGL_SYNC_NATIVE_FENCE_ANDROID,
                [EGL_NONE].as_ptr(),
            );
            if sync.is_null() {
                return Err(self.error("eglCreateSync"));
            }
            flush();
            let fd = (f.dup)(self.display, sync);
            (f.destroy)(self.display, sync);
            if fd == EGL_NO_NATIVE_FENCE_FD_ANDROID {
                return Err(self.error("eglDupNativeFenceFD"));
            }
            Ok(OwnedFd::from_raw_fd(fd))
        }
    }

    /// Make the GPU wait for the sync file `fence` before the work submitted after.
    pub fn wait_native_fence(&self, fence: OwnedFd) -> Result<()> {
        let f = self
            .fences
            .ok_or_else(|| Error::Unsupported("the driver has no native fences".into()))?;
        let wait = f
            .wait
            .ok_or_else(|| Error::Unsupported("the driver cannot wait on a fence".into()))?;
        let raw = fence.into_raw_fd();
        let attribs = [EGL_SYNC_NATIVE_FENCE_FD_ANDROID, raw, EGL_NONE];
        // Safety: a terminated attribute list; on success EGL owns the file descriptor, on failure it is closed here.
        unsafe {
            let sync = (f.create)(
                self.display,
                EGL_SYNC_NATIVE_FENCE_ANDROID,
                attribs.as_ptr(),
            );
            if sync.is_null() {
                drop(OwnedFd::from_raw_fd(raw));
                return Err(self.error("eglCreateSync of a sync file"));
            }
            let ok = wait(self.display, sync, 0);
            (f.destroy)(self.display, sync);
            if ok == 0 {
                return Err(self.error("eglWaitSync"));
            }
        }
        Ok(())
    }

    /// Make this context current again (after another device's context was).
    pub fn make_current(&self) -> Result<()> {
        if !self.owned {
            return Ok(());
        }
        // Safety: this context's own display and context.
        if unsafe { (self.egl.eglMakeCurrent)(self.display, null_mut(), null_mut(), self.context) }
            == 0
        {
            return Err(self.error("eglMakeCurrent"));
        }
        Ok(())
    }

    /// Whether this context is the current one.
    pub fn is_current(&self) -> bool {
        // Safety: no preconditions.
        unsafe { (self.egl.eglGetCurrentContext)() == self.context }
    }
}

fn has_ext(list: &str, name: &str) -> bool {
    list.split_whitespace().any(|x| x == name)
}

fn proc_address(egl: &Egl, name: &str) -> Option<*mut c_void> {
    let c = CString::new(name).ok()?;
    // Safety: a nul-terminated name; eglGetProcAddress has no other preconditions.
    let p = unsafe { (egl.eglGetProcAddress)(c.as_ptr()) };
    (!p.is_null()).then_some(p)
}

impl Drop for Context {
    fn drop(&mut self) {
        if !self.owned {
            return;
        }
        // Safety: this context's own objects, destroyed once, the context released first.
        unsafe {
            (self.egl.eglMakeCurrent)(self.display, null_mut(), null_mut(), null_mut());
            (self.egl.eglDestroyContext)(self.display, self.context);
            // a GBM device's display is this context's alone; the surfaceless display is one per process, shared
            // by every device on it, and terminating it would end the others' contexts
            if let Some((gbm, device)) = &self.gbm {
                (self.egl.eglTerminate)(self.display);
                (gbm.gbm_device_destroy)(*device);
            }
        }
        // the node closes after the GBM device that used it
        self.node.take();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_library_is_an_error_value_not_a_failed_start() {
        match open("libcupertino-no-such-library.so.1") {
            Err(Error::Unavailable(why)) => {
                assert!(why.contains("libcupertino-no-such-library"), "{why}")
            }
            other => panic!("{:?}", other.map(|_| ())),
        }
    }
}
