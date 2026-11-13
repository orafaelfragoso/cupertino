//! The few EGL, GLES 3 and GBM entry points the probes use, linked directly, and a context made the way a toolkit
//! process would make one: surfaceless, GLES 3, with robustness and purge notification where the driver offers them.

#![allow(non_upper_case_globals, clippy::missing_safety_doc)]

use std::ffi::{c_char, c_int, c_uint, c_void, CStr, CString};
use std::ptr::{null, null_mut};

pub type EGLDisplay = *mut c_void;
pub type EGLConfig = *mut c_void;
pub type EGLContext = *mut c_void;
pub type EGLSurface = *mut c_void;
pub type EGLDeviceEXT = *mut c_void;
pub type EGLSync = *mut c_void;
pub type EGLint = i32;
pub type GLuint = c_uint;
pub type GLint = c_int;
pub type GLenum = c_uint;
pub type GLsizei = c_int;

pub const EGL_NONE: EGLint = 0x3038;
pub const EGL_EXTENSIONS: EGLint = 0x3055;
pub const EGL_RENDERABLE_TYPE: EGLint = 0x3040;
pub const EGL_OPENGL_ES3_BIT: EGLint = 0x40;
pub const EGL_SURFACE_TYPE: EGLint = 0x3033;
pub const EGL_OPENGL_ES_API: c_uint = 0x30A0;
pub const EGL_CONTEXT_MAJOR_VERSION: EGLint = 0x3098;
pub const EGL_CONTEXT_MINOR_VERSION: EGLint = 0x30FB;
pub const EGL_CONTEXT_OPENGL_ROBUST_ACCESS_EXT: EGLint = 0x30BF;
pub const EGL_CONTEXT_OPENGL_RESET_NOTIFICATION_STRATEGY_EXT: EGLint = 0x3138;
pub const EGL_LOSE_CONTEXT_ON_RESET_EXT: EGLint = 0x31BF;
pub const EGL_GENERATE_RESET_ON_VIDEO_MEMORY_PURGE_NV: EGLint = 0x334C;
pub const EGL_TRUE: EGLint = 1;
pub const EGL_PLATFORM_GBM_KHR: c_uint = 0x31D7;
pub const EGL_PLATFORM_DEVICE_EXT: c_uint = 0x313F;
pub const EGL_DRM_RENDER_NODE_FILE_EXT: EGLint = 0x3377;
pub const EGL_SYNC_NATIVE_FENCE_ANDROID: c_uint = 0x3144;

pub const GL_VERTEX_SHADER: GLenum = 0x8B31;
pub const GL_FRAGMENT_SHADER: GLenum = 0x8B30;
pub const GL_COMPILE_STATUS: GLenum = 0x8B81;
pub const GL_LINK_STATUS: GLenum = 0x8B82;
pub const GL_PROGRAM_BINARY_LENGTH: GLenum = 0x8741;
pub const GL_PROGRAM_BINARY_RETRIEVABLE_HINT: GLenum = 0x8257;
pub const GL_TEXTURE_2D: GLenum = 0x0DE1;
pub const GL_RGBA8: GLenum = 0x8058;
pub const GL_FRAMEBUFFER: GLenum = 0x8D40;
pub const GL_COLOR_ATTACHMENT0: GLenum = 0x8CE0;
pub const GL_FRAMEBUFFER_COMPLETE: GLenum = 0x8CD5;
pub const GL_COLOR_BUFFER_BIT: c_uint = 0x4000;
pub const GL_TRIANGLES: GLenum = 0x0004;
pub const GL_RENDERER: GLenum = 0x1F01;
pub const GL_TEXTURE_MIN_FILTER: GLenum = 0x2801;
pub const GL_LINEAR: GLint = 0x2601;

#[repr(C)]
pub struct GbmDevice {
    _private: [u8; 0],
}

#[link(name = "EGL")]
extern "C" {
    pub fn eglGetProcAddress(name: *const c_char) -> *mut c_void;
    pub fn eglInitialize(dpy: EGLDisplay, major: *mut EGLint, minor: *mut EGLint) -> c_uint;
    pub fn eglTerminate(dpy: EGLDisplay) -> c_uint;
    pub fn eglQueryString(dpy: EGLDisplay, name: EGLint) -> *const c_char;
    pub fn eglBindAPI(api: c_uint) -> c_uint;
    pub fn eglChooseConfig(
        dpy: EGLDisplay,
        attribs: *const EGLint,
        configs: *mut EGLConfig,
        size: EGLint,
        num: *mut EGLint,
    ) -> c_uint;
    pub fn eglCreateContext(
        dpy: EGLDisplay,
        config: EGLConfig,
        share: EGLContext,
        attribs: *const EGLint,
    ) -> EGLContext;
    pub fn eglDestroyContext(dpy: EGLDisplay, ctx: EGLContext) -> c_uint;
    pub fn eglMakeCurrent(
        dpy: EGLDisplay,
        draw: EGLSurface,
        read: EGLSurface,
        ctx: EGLContext,
    ) -> c_uint;
    pub fn eglReleaseThread() -> c_uint;
    pub fn eglGetError() -> EGLint;
}

#[link(name = "GLESv2")]
extern "C" {
    pub fn glGetString(name: GLenum) -> *const u8;
    pub fn glCreateShader(kind: GLenum) -> GLuint;
    pub fn glShaderSource(sh: GLuint, count: GLsizei, src: *const *const c_char, len: *const GLint);
    pub fn glCompileShader(sh: GLuint);
    pub fn glGetShaderiv(sh: GLuint, name: GLenum, out: *mut GLint);
    pub fn glGetShaderInfoLog(sh: GLuint, size: GLsizei, len: *mut GLsizei, log: *mut c_char);
    pub fn glDeleteShader(sh: GLuint);
    pub fn glCreateProgram() -> GLuint;
    pub fn glAttachShader(prog: GLuint, sh: GLuint);
    pub fn glLinkProgram(prog: GLuint);
    pub fn glGetProgramiv(prog: GLuint, name: GLenum, out: *mut GLint);
    pub fn glGetProgramInfoLog(prog: GLuint, size: GLsizei, len: *mut GLsizei, log: *mut c_char);
    pub fn glProgramParameteri(prog: GLuint, name: GLenum, value: GLint);
    pub fn glGetProgramBinary(
        prog: GLuint,
        size: GLsizei,
        len: *mut GLsizei,
        format: *mut GLenum,
        binary: *mut c_void,
    );
    pub fn glProgramBinary(prog: GLuint, format: GLenum, binary: *const c_void, len: GLsizei);
    pub fn glUseProgram(prog: GLuint);
    pub fn glGetUniformLocation(prog: GLuint, name: *const c_char) -> GLint;
    pub fn glUniform4f(loc: GLint, x: f32, y: f32, z: f32, w: f32);
    pub fn glGenTextures(n: GLsizei, out: *mut GLuint);
    pub fn glDeleteTextures(n: GLsizei, tex: *const GLuint);
    pub fn glBindTexture(target: GLenum, tex: GLuint);
    pub fn glTexStorage2D(target: GLenum, levels: GLsizei, format: GLenum, w: GLsizei, h: GLsizei);
    pub fn glTexParameteri(target: GLenum, name: GLenum, value: GLint);
    pub fn glGenFramebuffers(n: GLsizei, out: *mut GLuint);
    pub fn glDeleteFramebuffers(n: GLsizei, fb: *const GLuint);
    pub fn glBindFramebuffer(target: GLenum, fb: GLuint);
    pub fn glFramebufferTexture2D(
        target: GLenum,
        att: GLenum,
        textarget: GLenum,
        tex: GLuint,
        level: GLint,
    );
    pub fn glCheckFramebufferStatus(target: GLenum) -> GLenum;
    pub fn glGenVertexArrays(n: GLsizei, out: *mut GLuint);
    pub fn glBindVertexArray(vao: GLuint);
    pub fn glViewport(x: GLint, y: GLint, w: GLsizei, h: GLsizei);
    pub fn glClearColor(r: f32, g: f32, b: f32, a: f32);
    pub fn glClear(mask: c_uint);
    pub fn glDrawArrays(mode: GLenum, first: GLint, count: GLsizei);
    pub fn glFlush();
    pub fn glFinish();
}

#[link(name = "gbm")]
extern "C" {
    pub fn gbm_create_device(fd: c_int) -> *mut GbmDevice;
    pub fn gbm_device_destroy(dev: *mut GbmDevice);
}

type GetPlatformDisplay = unsafe extern "C" fn(c_uint, *mut c_void, *const EGLint) -> EGLDisplay;
type QueryDevices = unsafe extern "C" fn(EGLint, *mut EGLDeviceEXT, *mut EGLint) -> c_uint;
type QueryDeviceString = unsafe extern "C" fn(EGLDeviceEXT, EGLint) -> *const c_char;
pub type CreateSync = unsafe extern "C" fn(EGLDisplay, c_uint, *const EGLint) -> EGLSync;
pub type DestroySync = unsafe extern "C" fn(EGLDisplay, EGLSync) -> c_uint;
pub type DupNativeFence = unsafe extern "C" fn(EGLDisplay, EGLSync) -> EGLint;

/// An extension entry point by name.
pub unsafe fn proc<T: Copy>(name: &str) -> Option<T> {
    let c = CString::new(name).ok()?;
    let p = eglGetProcAddress(c.as_ptr());
    if p.is_null() {
        None
    } else {
        Some(std::mem::transmute_copy(&p))
    }
}

fn text(p: *const c_char) -> String {
    if p.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
    }
}

pub fn gl_string(name: GLenum) -> String {
    text(unsafe { glGetString(name) }.cast())
}

/// Where the display comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    /// A GBM device on the render node.
    Gbm,
    /// The EGL device of that render node.
    Device,
}

impl Platform {
    pub fn parse(s: &str) -> Option<Platform> {
        match s {
            "gbm" => Some(Platform::Gbm),
            "device" => Some(Platform::Device),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Platform::Gbm => "gbm",
            Platform::Device => "device",
        }
    }
}

/// An initialised display and a current surfaceless GLES 3 context.
pub struct Gpu {
    pub display: EGLDisplay,
    pub context: EGLContext,
    pub extensions: String,
    gbm: *mut GbmDevice,
    fd: c_int,
}

/// The render node the probes use: `GPU_PROBES_NODE`, else the first `renderD*` node.
pub fn render_node() -> String {
    if let Ok(node) = std::env::var("GPU_PROBES_NODE") {
        return node;
    }
    let mut nodes: Vec<String> = std::fs::read_dir("/dev/dri")
        .map(|d| {
            d.filter_map(|e| e.ok())
                .map(|e| e.path().to_string_lossy().into_owned())
                .filter(|p| p.contains("renderD"))
                .collect()
        })
        .unwrap_or_default();
    nodes.sort();
    nodes
        .into_iter()
        .next()
        .unwrap_or_else(|| "/dev/dri/renderD128".into())
}

/// The display of `platform` on the render node, initialised.
pub unsafe fn display(platform: Platform) -> Result<(EGLDisplay, *mut GbmDevice, c_int), String> {
    let get: GetPlatformDisplay =
        proc("eglGetPlatformDisplayEXT").ok_or("no eglGetPlatformDisplayEXT")?;
    let node = render_node();
    let (display, gbm, fd) = match platform {
        Platform::Gbm => {
            let path = CString::new(node.clone()).unwrap();
            let fd = libc::open(path.as_ptr(), libc::O_RDWR | libc::O_CLOEXEC);
            if fd < 0 {
                return Err(format!("cannot open {node}"));
            }
            let gbm = gbm_create_device(fd);
            if gbm.is_null() {
                return Err("gbm_create_device failed".into());
            }
            (get(EGL_PLATFORM_GBM_KHR, gbm.cast(), null()), gbm, fd)
        }
        Platform::Device => {
            let query: QueryDevices = proc("eglQueryDevicesEXT").ok_or("no eglQueryDevicesEXT")?;
            let string: QueryDeviceString =
                proc("eglQueryDeviceStringEXT").ok_or("no eglQueryDeviceStringEXT")?;
            let mut devices = [null_mut(); 16];
            let mut n = 0;
            query(16, devices.as_mut_ptr(), &mut n);
            let chosen = devices[..n.max(0) as usize]
                .iter()
                .copied()
                .find(|&d| text(string(d, EGL_DRM_RENDER_NODE_FILE_EXT)) == node)
                .ok_or_else(|| format!("no EGL device for {node}"))?;
            (get(EGL_PLATFORM_DEVICE_EXT, chosen, null()), null_mut(), -1)
        }
    };
    if display.is_null() {
        return Err(format!("no display: EGL error {:#x}", eglGetError()));
    }
    let (mut major, mut minor) = (0, 0);
    if eglInitialize(display, &mut major, &mut minor) == 0 {
        return Err(format!("eglInitialize: EGL error {:#x}", eglGetError()));
    }
    Ok((display, gbm, fd))
}

impl Gpu {
    /// Display, context and make-current, with the context attributes of a toolkit process.
    pub unsafe fn new(platform: Platform) -> Result<Gpu, String> {
        let (display, gbm, fd) = display(platform)?;
        let extensions = text(eglQueryString(display, EGL_EXTENSIONS));
        eglBindAPI(EGL_OPENGL_ES_API);
        let config_attribs = [
            EGL_RENDERABLE_TYPE,
            EGL_OPENGL_ES3_BIT,
            EGL_SURFACE_TYPE,
            0,
            EGL_NONE,
        ];
        let mut config = null_mut();
        let mut n = 0;
        if eglChooseConfig(display, config_attribs.as_ptr(), &mut config, 1, &mut n) == 0 || n == 0
        {
            // a context without a config, where the driver allows one
            config = null_mut();
        }
        let mut attribs = vec![EGL_CONTEXT_MAJOR_VERSION, 3, EGL_CONTEXT_MINOR_VERSION, 0];
        if extensions
            .split_whitespace()
            .any(|e| e == "EGL_EXT_create_context_robustness")
        {
            attribs.extend([
                EGL_CONTEXT_OPENGL_ROBUST_ACCESS_EXT,
                EGL_TRUE,
                EGL_CONTEXT_OPENGL_RESET_NOTIFICATION_STRATEGY_EXT,
                EGL_LOSE_CONTEXT_ON_RESET_EXT,
            ]);
            if extensions
                .split_whitespace()
                .any(|e| e == "EGL_NV_robustness_video_memory_purge")
            {
                attribs.extend([EGL_GENERATE_RESET_ON_VIDEO_MEMORY_PURGE_NV, EGL_TRUE]);
            }
        }
        attribs.push(EGL_NONE);
        let context = eglCreateContext(display, config, null_mut(), attribs.as_ptr());
        if context.is_null() {
            return Err(format!("eglCreateContext: EGL error {:#x}", eglGetError()));
        }
        if eglMakeCurrent(display, null_mut(), null_mut(), context) == 0 {
            return Err(format!("eglMakeCurrent: EGL error {:#x}", eglGetError()));
        }
        Ok(Gpu {
            display,
            context,
            extensions,
            gbm,
            fd,
        })
    }

    pub fn has(&self, extension: &str) -> bool {
        self.extensions.split_whitespace().any(|e| e == extension)
    }

    /// Everything given back: context, display, GBM device and the node.
    pub unsafe fn teardown(self) {
        eglMakeCurrent(self.display, null_mut(), null_mut(), null_mut());
        eglDestroyContext(self.display, self.context);
        eglTerminate(self.display);
        eglReleaseThread();
        if !self.gbm.is_null() {
            gbm_device_destroy(self.gbm);
        }
        if self.fd >= 0 {
            libc::close(self.fd);
        }
    }
}

pub const VERTEX: &str = "#version 300 es
out vec2 uv;
void main() {
    vec2 p = vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2));
    uv = p;
    gl_Position = vec4(p * 2.0 - 1.0, 0.0, 1.0);
}
";

/// A fragment program of the kind a GPU toolkit renderer runs (rounded-rectangle coverage, a gradient, a short
/// Gaussian), made distinct by `variant` so that the driver cannot reuse an earlier compilation.
pub fn fragment(variant: u32) -> String {
    format!(
        "#version 300 es
precision highp float;
in vec2 uv;
out vec4 color;
uniform vec4 rect;
uniform vec4 tint;
const float VARIANT = {variant}.0;
float rounded(vec2 p, vec2 half_size, float r) {{
    vec2 q = abs(p) - half_size + r;
    return length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - r;
}}
float gauss(float x, float s) {{ return exp(-0.5 * x * x / (s * s)); }}
void main() {{
    vec2 p = uv * 512.0 - rect.xy;
    float d = rounded(p, rect.zw * 0.5, 12.0 + VARIANT * 0.001);
    float cover = clamp(0.5 - d, 0.0, 1.0);
    float shadow = 0.0;
    float total = 0.0;
    for (int i = -6; i <= 6; i++) {{
        float w = gauss(float(i), 3.0);
        shadow += w * clamp(0.5 - rounded(p + vec2(0.0, float(i) + 4.0), rect.zw * 0.5, 12.0), 0.0, 1.0);
        total += w;
    }}
    shadow /= total;
    vec4 top = vec4(0.95, 0.95, 0.97, 1.0);
    vec4 bottom = vec4(0.85, 0.86, 0.9, 1.0);
    vec4 fill = mix(top, bottom, uv.y) * tint;
    color = mix(vec4(0.0, 0.0, 0.0, 0.3 * shadow), fill, cover);
}}
"
    )
}

pub unsafe fn compile(kind: GLenum, src: &str) -> Result<GLuint, String> {
    let sh = glCreateShader(kind);
    let c = CString::new(src).unwrap();
    let p = c.as_ptr();
    glShaderSource(sh, 1, &p, null());
    glCompileShader(sh);
    let mut ok = 0;
    glGetShaderiv(sh, GL_COMPILE_STATUS, &mut ok);
    if ok == 0 {
        let mut log = vec![0 as c_char; 4096];
        let mut len = 0;
        glGetShaderInfoLog(sh, log.len() as GLsizei, &mut len, log.as_mut_ptr());
        return Err(text(log.as_ptr()));
    }
    Ok(sh)
}

/// A linked program of the vertex shader and fragment `variant`; `retrievable` asks the driver to keep its binary.
pub unsafe fn program(variant: u32, retrievable: bool) -> Result<GLuint, String> {
    let v = compile(GL_VERTEX_SHADER, VERTEX)?;
    let f = compile(GL_FRAGMENT_SHADER, &fragment(variant))?;
    let prog = glCreateProgram();
    if retrievable {
        glProgramParameteri(prog, GL_PROGRAM_BINARY_RETRIEVABLE_HINT, 1);
    }
    glAttachShader(prog, v);
    glAttachShader(prog, f);
    glLinkProgram(prog);
    glDeleteShader(v);
    glDeleteShader(f);
    linked(prog)?;
    Ok(prog)
}

pub unsafe fn linked(prog: GLuint) -> Result<(), String> {
    let mut ok = 0;
    glGetProgramiv(prog, GL_LINK_STATUS, &mut ok);
    if ok == 0 {
        let mut log = vec![0 as c_char; 4096];
        let mut len = 0;
        glGetProgramInfoLog(prog, log.len() as GLsizei, &mut len, log.as_mut_ptr());
        return Err(format!("link: {}", text(log.as_ptr())));
    }
    Ok(())
}

/// A render target: texture and framebuffer of `w` x `h` RGBA8.
pub struct Target {
    pub tex: GLuint,
    pub fbo: GLuint,
    pub size: (i32, i32),
}

impl Target {
    pub unsafe fn new(w: i32, h: i32) -> Result<Target, String> {
        let (mut tex, mut fbo) = (0, 0);
        glGenTextures(1, &mut tex);
        glBindTexture(GL_TEXTURE_2D, tex);
        glTexStorage2D(GL_TEXTURE_2D, 1, GL_RGBA8, w, h);
        glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR);
        glGenFramebuffers(1, &mut fbo);
        glBindFramebuffer(GL_FRAMEBUFFER, fbo);
        glFramebufferTexture2D(GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_TEXTURE_2D, tex, 0);
        if glCheckFramebufferStatus(GL_FRAMEBUFFER) != GL_FRAMEBUFFER_COMPLETE {
            return Err("framebuffer incomplete".into());
        }
        Ok(Target {
            tex,
            fbo,
            size: (w, h),
        })
    }

    pub unsafe fn delete(self) {
        glDeleteFramebuffers(1, &self.fbo);
        glDeleteTextures(1, &self.tex);
    }
}

/// Draws one full-target triangle of `prog` into `target` (bound), so the driver commits its memory.
pub unsafe fn draw(prog: GLuint, target: &Target) {
    static mut VAO: GLuint = 0;
    if VAO == 0 {
        let mut vao = 0;
        glGenVertexArrays(1, &mut vao);
        VAO = vao;
    }
    glBindVertexArray(VAO);
    glBindFramebuffer(GL_FRAMEBUFFER, target.fbo);
    glViewport(0, 0, target.size.0, target.size.1);
    glClearColor(0.0, 0.0, 0.0, 0.0);
    glClear(GL_COLOR_BUFFER_BIT);
    glUseProgram(prog);
    let name = CString::new("rect").unwrap();
    glUniform4f(
        glGetUniformLocation(prog, name.as_ptr()),
        256.0,
        256.0,
        300.0,
        200.0,
    );
    let name = CString::new("tint").unwrap();
    glUniform4f(
        glGetUniformLocation(prog, name.as_ptr()),
        1.0,
        1.0,
        1.0,
        1.0,
    );
    glDrawArrays(GL_TRIANGLES, 0, 3);
}
