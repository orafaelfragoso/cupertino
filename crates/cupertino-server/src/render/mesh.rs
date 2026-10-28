//! Textured-mesh element: draws a window snapshot through a vertex grid produced by
//! `cupertino-effects` (warp, funnel and scale minimize, closing and opening quads).
//!
//! The mesh lives in output-local logical points; the element converts it to output pixels and
//! draws it as a triangle list with raw GL on the renderer's context (the same technique as the
//! material pass), clipped to the damage rectangles through the scissor test. Textures come from
//! [`capture`]: a window rendered offscreen with the normal element tree.

use std::cell::RefCell;
use std::ffi::CString;
use std::rc::Rc;

use anyhow::{anyhow, Result};
use cupertino_effects::Mesh;
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::element::{Element, Id, Kind, RenderElement, UnderlyingStorage};
use smithay::backend::renderer::gles::{ffi, GlesError, GlesFrame, GlesRenderer, GlesTexture};
use smithay::backend::renderer::utils::{CommitCounter, OpaqueRegions};
use smithay::backend::renderer::{Bind, Color32F, Offscreen};
use smithay::utils::{Buffer, Physical, Rectangle, Scale, Size, Transform};

use super::snapshots::SnapshotLease;
use super::CupertinoElement;

const VERT: &str = "#version 100
uniform mat3 u_proj;
attribute vec2 a_pos;
attribute vec2 a_uv;
varying vec2 v_uv;
void main() {
    vec3 p = u_proj * vec3(a_pos, 1.0);
    gl_Position = vec4(p.xy, 0.0, 1.0);
    v_uv = a_uv;
}
";

const FRAG: &str = "#version 100
precision mediump float;
uniform sampler2D u_tex;
uniform float u_alpha;
varying vec2 v_uv;
void main() {
    gl_FragColor = texture2D(u_tex, v_uv) * u_alpha;
}
";

type GLuint = ffi::types::GLuint;
type GLint = ffi::types::GLint;

/// Whether textures produced by [`capture`] store the top image row at texture `v = 1` (verified
/// by `tests/headless.rs::capture_orientation`). The mesh shader flips `v` accordingly.
pub const CAPTURE_V_FLIPPED: bool = false;

struct MeshProgram {
    program: GLuint,
    proj: GLint,
    tex: GLint,
    alpha: GLint,
    pos: GLuint,
    uv: GLuint,
}

/// GL program of the mesh pass, created once per renderer.
pub struct MeshGl {
    prog: Option<MeshProgram>,
}

pub type SharedMesh = Rc<RefCell<MeshGl>>;

unsafe fn compile(gl: &ffi::Gles2, kind: u32, src: &str) -> Result<GLuint, String> {
    let sh = gl.CreateShader(kind);
    let ptr = src.as_ptr() as *const ffi::types::GLchar;
    let len = src.len() as GLint;
    gl.ShaderSource(sh, 1, &ptr, &len);
    gl.CompileShader(sh);
    let mut ok = 0;
    gl.GetShaderiv(sh, ffi::COMPILE_STATUS, &mut ok);
    if ok == 0 {
        let mut n = 0;
        gl.GetShaderiv(sh, ffi::INFO_LOG_LENGTH, &mut n);
        let mut buf = vec![0u8; n.max(1) as usize];
        let mut w = 0;
        gl.GetShaderInfoLog(sh, n, &mut w, buf.as_mut_ptr() as *mut _);
        gl.DeleteShader(sh);
        return Err(String::from_utf8_lossy(&buf[..w.max(0) as usize]).into_owned());
    }
    Ok(sh)
}

unsafe fn loc(gl: &ffi::Gles2, p: GLuint, name: &str) -> GLint {
    let c = CString::new(name).unwrap();
    gl.GetUniformLocation(p, c.as_ptr())
}

impl MeshGl {
    pub fn new(renderer: &mut GlesRenderer) -> Result<SharedMesh, String> {
        let prog = renderer
            .with_context(|gl| unsafe {
                let v = compile(gl, ffi::VERTEX_SHADER, VERT)
                    .map_err(|e| format!("mesh vertex: {e}"))?;
                let f = compile(gl, ffi::FRAGMENT_SHADER, FRAG)
                    .map_err(|e| format!("mesh fragment: {e}"))?;
                let p = gl.CreateProgram();
                gl.AttachShader(p, v);
                gl.AttachShader(p, f);
                gl.LinkProgram(p);
                gl.DeleteShader(v);
                gl.DeleteShader(f);
                let mut ok = 0;
                gl.GetProgramiv(p, ffi::LINK_STATUS, &mut ok);
                if ok == 0 {
                    return Err("mesh program failed to link".to_string());
                }
                let a = CString::new("a_pos").unwrap();
                let b = CString::new("a_uv").unwrap();
                Ok(MeshProgram {
                    program: p,
                    proj: loc(gl, p, "u_proj"),
                    tex: loc(gl, p, "u_tex"),
                    alpha: loc(gl, p, "u_alpha"),
                    pos: gl.GetAttribLocation(p, a.as_ptr()) as GLuint,
                    uv: gl.GetAttribLocation(p, b.as_ptr()) as GLuint,
                })
            })
            .map_err(|e| format!("making the context current failed: {e}"))??;
        Ok(Rc::new(RefCell::new(MeshGl { prog: Some(prog) })))
    }

    pub fn destroy(&mut self, renderer: &mut GlesRenderer) {
        if let Some(p) = self.prog.take() {
            let _ = renderer.with_context(|gl| unsafe { gl.DeleteProgram(p.program) });
        }
    }
}

/// A texture drawn through a mesh. Positions are output-local logical points.
pub struct MeshElement {
    id: Id,
    commit: CommitCounter,
    texture: GlesTexture,
    shared: SharedMesh,
    /// Triangle list: `(x, y, u, v)` in output pixels / texture coordinates.
    triangles: Vec<[f32; 4]>,
    bounds: Rectangle<i32, Physical>,
    alpha: f32,
    /// Keeps a retained snapshot until the element is gone.
    _lease: Option<SnapshotLease>,
}

impl std::fmt::Debug for MeshElement {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MeshElement")
            .field("bounds", &self.bounds)
            .field("triangles", &(self.triangles.len() / 3))
            .finish()
    }
}

impl MeshElement {
    /// `texture_uv` maps mesh texture coordinates (0..1 over the captured area, top-left origin)
    /// to the texture; `scale` converts points to pixels.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: Id,
        commit: CommitCounter,
        texture: GlesTexture,
        shared: SharedMesh,
        mesh: &Mesh,
        scale: f64,
        alpha: f32,
        lease: Option<SnapshotLease>,
    ) -> Self {
        let s = scale as f32;
        let mut triangles = Vec::with_capacity(mesh.cell_count() * 6);
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for v in &mesh.vertices {
            x0 = x0.min(v.position[0] * s);
            x1 = x1.max(v.position[0] * s);
            y0 = y0.min(v.position[1] * s);
            y1 = y1.max(v.position[1] * s);
        }
        for i in mesh.indices() {
            let v = mesh.vertices[i as usize];
            let tv = if CAPTURE_V_FLIPPED {
                1.0 - v.uv[1]
            } else {
                v.uv[1]
            };
            triangles.push([v.position[0] * s, v.position[1] * s, v.uv[0], tv]);
        }
        let bounds = if mesh.vertices.is_empty() {
            Rectangle::default()
        } else {
            let (l, t) = (x0.floor() as i32 - 1, y0.floor() as i32 - 1);
            Rectangle::new(
                (l, t).into(),
                (
                    (x1.ceil() as i32 + 1 - l).max(0),
                    (y1.ceil() as i32 + 1 - t).max(0),
                )
                    .into(),
            )
        };
        Self {
            id,
            commit,
            texture,
            shared,
            triangles,
            bounds,
            alpha,
            _lease: lease,
        }
    }

    pub fn triangle_count(&self) -> usize {
        self.triangles.len() / 3
    }
}

impl Element for MeshElement {
    fn id(&self) -> &Id {
        &self.id
    }

    fn current_commit(&self) -> CommitCounter {
        self.commit
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        Rectangle::from_size(Size::from((1.0, 1.0)))
    }

    fn geometry(&self, _scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.bounds
    }

    fn opaque_regions(&self, _scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        OpaqueRegions::default()
    }

    fn alpha(&self) -> f32 {
        self.alpha
    }

    fn kind(&self) -> Kind {
        Kind::Unspecified
    }
}

fn map_point(m: &[f32; 9], x: f32, y: f32) -> (f32, f32) {
    (m[0] * x + m[3] * y + m[6], m[1] * x + m[4] * y + m[7])
}

impl RenderElement<GlesRenderer> for MeshElement {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        _src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        _opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), GlesError> {
        if damage.is_empty() || self.triangles.is_empty() {
            return Ok(());
        }
        let proj = *frame.projection();
        let abs: Vec<Rectangle<i32, Physical>> = damage
            .iter()
            .map(|d| Rectangle::new(d.loc + dst.loc, d.size))
            .collect();
        let shared = self.shared.clone();
        let tex = self.texture.tex_id();
        let alpha = self.alpha;
        let tris = &self.triangles;
        frame.with_context(move |gl| unsafe {
            let guard = shared.borrow();
            let Some(p) = guard.prog.as_ref() else { return };
            let mut prev_program = 0;
            gl.GetIntegerv(ffi::CURRENT_PROGRAM, &mut prev_program);
            let mut prev_active = 0;
            gl.GetIntegerv(ffi::ACTIVE_TEXTURE, &mut prev_active);
            gl.ActiveTexture(ffi::TEXTURE0);
            let mut prev_tex = 0;
            gl.GetIntegerv(ffi::TEXTURE_BINDING_2D, &mut prev_tex);
            let mut viewport = [0; 4];
            gl.GetIntegerv(ffi::VIEWPORT, viewport.as_mut_ptr());
            let scissor_on = gl.IsEnabled(ffi::SCISSOR_TEST) != 0;
            let mut scissor = [0; 4];
            gl.GetIntegerv(ffi::SCISSOR_BOX, scissor.as_mut_ptr());
            let mut prev_buffer = 0;
            gl.GetIntegerv(ffi::ARRAY_BUFFER_BINDING, &mut prev_buffer);

            gl.UseProgram(p.program);
            gl.UniformMatrix3fv(p.proj, 1, ffi::FALSE, proj.as_ptr());
            gl.Uniform1i(p.tex, 0);
            gl.Uniform1f(p.alpha, alpha);
            gl.BindTexture(ffi::TEXTURE_2D, tex);
            gl.TexParameteri(
                ffi::TEXTURE_2D,
                ffi::TEXTURE_MIN_FILTER,
                ffi::LINEAR as GLint,
            );
            gl.TexParameteri(
                ffi::TEXTURE_2D,
                ffi::TEXTURE_MAG_FILTER,
                ffi::LINEAR as GLint,
            );
            gl.TexParameteri(
                ffi::TEXTURE_2D,
                ffi::TEXTURE_WRAP_S,
                ffi::CLAMP_TO_EDGE as GLint,
            );
            gl.TexParameteri(
                ffi::TEXTURE_2D,
                ffi::TEXTURE_WRAP_T,
                ffi::CLAMP_TO_EDGE as GLint,
            );
            gl.BindBuffer(ffi::ARRAY_BUFFER, 0);
            let stride = (4 * std::mem::size_of::<f32>()) as i32;
            let divisor = |gl: &ffi::Gles2, index: GLuint| {
                let mut d = 0;
                gl.GetVertexAttribiv(index, ffi::VERTEX_ATTRIB_ARRAY_DIVISOR, &mut d);
                d
            };
            // the renderer draws its own quads with instanced attributes: reset the divisors
            let prev_divisors = [divisor(gl, p.pos), divisor(gl, p.uv)];
            gl.VertexAttribDivisor(p.pos, 0);
            gl.VertexAttribDivisor(p.uv, 0);
            gl.EnableVertexAttribArray(p.pos);
            gl.EnableVertexAttribArray(p.uv);
            gl.VertexAttribPointer(
                p.pos,
                2,
                ffi::FLOAT,
                ffi::FALSE,
                stride,
                tris.as_ptr() as *const _,
            );
            gl.VertexAttribPointer(
                p.uv,
                2,
                ffi::FLOAT,
                ffi::FALSE,
                stride,
                (tris.as_ptr() as *const f32).add(2) as *const _,
            );
            gl.Enable(ffi::BLEND);
            gl.BlendFunc(ffi::ONE, ffi::ONE_MINUS_SRC_ALPHA);
            gl.Enable(ffi::SCISSOR_TEST);
            for d in &abs {
                let corners = [
                    map_point(&proj, d.loc.x as f32, d.loc.y as f32),
                    map_point(
                        &proj,
                        (d.loc.x + d.size.w) as f32,
                        (d.loc.y + d.size.h) as f32,
                    ),
                ];
                let to_win = |ndc: f32, origin: i32, size: i32| {
                    (ndc * 0.5 + 0.5) * size as f32 + origin as f32
                };
                let xs = corners.map(|c| to_win(c.0, viewport[0], viewport[2]));
                let ys = corners.map(|c| to_win(c.1, viewport[1], viewport[3]));
                let (sx, sy) = (
                    xs[0].min(xs[1]).floor() as i32,
                    ys[0].min(ys[1]).floor() as i32,
                );
                let (sw, sh) = (
                    xs[0].max(xs[1]).ceil() as i32 - sx,
                    ys[0].max(ys[1]).ceil() as i32 - sy,
                );
                if sw <= 0 || sh <= 0 {
                    continue;
                }
                gl.Scissor(sx, sy, sw, sh);
                gl.DrawArrays(ffi::TRIANGLES, 0, tris.len() as i32);
            }
            gl.DisableVertexAttribArray(p.pos);
            gl.DisableVertexAttribArray(p.uv);
            gl.VertexAttribDivisor(p.pos, prev_divisors[0] as GLuint);
            gl.VertexAttribDivisor(p.uv, prev_divisors[1] as GLuint);
            gl.Scissor(scissor[0], scissor[1], scissor[2], scissor[3]);
            if !scissor_on {
                gl.Disable(ffi::SCISSOR_TEST);
            }
            gl.BindBuffer(ffi::ARRAY_BUFFER, prev_buffer as GLuint);
            gl.BindTexture(ffi::TEXTURE_2D, prev_tex as GLuint);
            gl.ActiveTexture(prev_active as u32);
            gl.UseProgram(prev_program as GLuint);
        })?;
        Ok(())
    }

    fn underlying_storage(&self, _renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        None
    }
}

/// Render `elements` (front to back, output-local coordinates) into a new texture of `size`
/// pixels, transparent where nothing is drawn.
pub fn capture(
    renderer: &mut GlesRenderer,
    elements: &[CupertinoElement],
    size: Size<i32, Physical>,
    scale: f64,
) -> Result<GlesTexture> {
    let buf_size: Size<i32, Buffer> = (size.w.max(1), size.h.max(1)).into();
    let mut texture: GlesTexture = renderer
        .create_buffer(Fourcc::Abgr8888, buf_size)
        .map_err(|e| anyhow!("create snapshot texture: {e}"))?;
    let mut tracker = OutputDamageTracker::new((buf_size.w, buf_size.h), scale, Transform::Normal);
    {
        let mut target = renderer
            .bind(&mut texture)
            .map_err(|e| anyhow!("bind snapshot texture: {e}"))?;
        tracker
            .render_output(
                renderer,
                &mut target,
                0,
                elements,
                Color32F::new(0.0, 0.0, 0.0, 0.0),
            )
            .map_err(|e| anyhow!("render snapshot: {e:?}"))?;
    }
    Ok(texture)
}
