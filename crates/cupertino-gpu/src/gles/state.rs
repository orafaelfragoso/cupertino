//! The state-restore contract of a borrowed device.
//!
//! A borrowed device draws inside its owner's context (the compositor's), between [`Device::enter`] and
//! [`Device::leave`]. `enter` saves, and `leave` puts back, everything a device call may change:
//!
//! * the program; the draw and read framebuffers; the viewport and the scissor box and test;
//! * blending (enabled, equations, factors), the colour write mask and the clear colour;
//! * depth test, stencil test and face culling enables (a pass turns them off);
//! * the active texture unit, and on each of the first [`UNITS`] units its 2D texture and its sampler;
//! * the generic array and uniform buffer bindings, and the uniform buffer range of each of the first [`UNITS`]
//!   points;
//! * the vertex array (the device uses one of its own, so the owner's attribute and index state is not touched);
//! * pack and unpack alignment and the unpack row length.
//!
//! Anything else (external-image texture bindings, renderbuffers, transform feedback, queries) is never touched by
//! the device. [`BorrowedState`] compares equal when the context holds the same state, which is how the contract
//! is tested.
//!
//! The device marks which of those groups ([`Touched`]) its calls changed since it was entered, and puts back only
//! those: state it never changed is not written again, so a pause between two of the owner's draws
//! ([`Device::pause`]) costs the few calls of what a pass set, and nothing is read.
//!
//! [`Device::enter`]: super::Device::enter
//! [`Device::leave`]: super::Device::leave
//! [`Device::pause`]: super::Device::pause

use glow::HasContext;

use super::UNITS;

/// The state a borrowed device saves on entering and restores on leaving.
#[derive(Clone, Debug, PartialEq)]
pub struct BorrowedState {
    program: Option<glow::Program>,
    draw_framebuffer: Option<glow::Framebuffer>,
    read_framebuffer: Option<glow::Framebuffer>,
    viewport: [i32; 4],
    scissor_box: [i32; 4],
    scissor_test: bool,
    blend: bool,
    blend_equation: [i32; 2],
    blend_factors: [i32; 4],
    color_mask: [bool; 4],
    clear_color: [f32; 4],
    depth_test: bool,
    stencil_test: bool,
    cull_face: bool,
    active_texture: i32,
    textures: Vec<(Option<glow::Texture>, Option<glow::Sampler>)>,
    array_buffer: Option<glow::Buffer>,
    uniform_buffer: Option<glow::Buffer>,
    uniform_ranges: Vec<(i32, i32, i32)>,
    vertex_array: Option<glow::VertexArray>,
    pack_alignment: i32,
    unpack_alignment: i32,
    unpack_row_length: i32,
}

pub(super) fn capture(gl: &glow::Context) -> BorrowedState {
    // Safety: plain queries of the current context.
    unsafe {
        let ints4 = |p| {
            let mut v = [0; 4];
            gl.get_parameter_i32_slice(p, &mut v);
            v
        };
        let mut clear_color = [0.0; 4];
        gl.get_parameter_f32_slice(glow::COLOR_CLEAR_VALUE, &mut clear_color);
        let active_texture = gl.get_parameter_i32(glow::ACTIVE_TEXTURE);
        let textures = (0..UNITS)
            .map(|unit| {
                gl.active_texture(glow::TEXTURE0 + unit);
                (
                    gl.get_parameter_texture(glow::TEXTURE_BINDING_2D),
                    gl.get_parameter_sampler(glow::SAMPLER_BINDING),
                )
            })
            .collect();
        gl.active_texture(active_texture as u32);
        let uniform_ranges = (0..UNITS)
            .map(|i| {
                (
                    gl.get_parameter_indexed_i32(glow::UNIFORM_BUFFER_BINDING, i),
                    gl.get_parameter_indexed_i32(glow::UNIFORM_BUFFER_START, i),
                    gl.get_parameter_indexed_i32(glow::UNIFORM_BUFFER_SIZE, i),
                )
            })
            .collect();
        BorrowedState {
            program: gl.get_parameter_program(glow::CURRENT_PROGRAM),
            draw_framebuffer: gl.get_parameter_framebuffer(glow::DRAW_FRAMEBUFFER_BINDING),
            read_framebuffer: gl.get_parameter_framebuffer(glow::READ_FRAMEBUFFER_BINDING),
            viewport: ints4(glow::VIEWPORT),
            scissor_box: ints4(glow::SCISSOR_BOX),
            scissor_test: gl.is_enabled(glow::SCISSOR_TEST),
            blend: gl.is_enabled(glow::BLEND),
            blend_equation: [
                gl.get_parameter_i32(glow::BLEND_EQUATION_RGB),
                gl.get_parameter_i32(glow::BLEND_EQUATION_ALPHA),
            ],
            blend_factors: [
                gl.get_parameter_i32(glow::BLEND_SRC_RGB),
                gl.get_parameter_i32(glow::BLEND_DST_RGB),
                gl.get_parameter_i32(glow::BLEND_SRC_ALPHA),
                gl.get_parameter_i32(glow::BLEND_DST_ALPHA),
            ],
            color_mask: gl.get_parameter_bool_array::<4>(glow::COLOR_WRITEMASK),
            clear_color,
            depth_test: gl.is_enabled(glow::DEPTH_TEST),
            stencil_test: gl.is_enabled(glow::STENCIL_TEST),
            cull_face: gl.is_enabled(glow::CULL_FACE),
            active_texture,
            textures,
            array_buffer: gl.get_parameter_buffer(glow::ARRAY_BUFFER_BINDING),
            uniform_buffer: gl.get_parameter_buffer(glow::UNIFORM_BUFFER_BINDING),
            uniform_ranges,
            vertex_array: gl.get_parameter_vertex_array(glow::VERTEX_ARRAY_BINDING),
            pack_alignment: gl.get_parameter_i32(glow::PACK_ALIGNMENT),
            unpack_alignment: gl.get_parameter_i32(glow::UNPACK_ALIGNMENT),
            unpack_row_length: gl.get_parameter_i32(glow::UNPACK_ROW_LENGTH),
        }
    }
}

/// The groups of [`BorrowedState`] a device call may change, as bits: what [`restore`] puts back.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Touched(pub(super) u16);

impl Touched {
    pub(super) const NONE: Touched = Touched(0);
    pub(super) const PROGRAM: Touched = Touched(1);
    /// The draw and read framebuffers.
    pub(super) const FRAMEBUFFERS: Touched = Touched(1 << 1);
    pub(super) const VIEWPORT: Touched = Touched(1 << 2);
    /// The scissor box and test.
    pub(super) const SCISSOR: Touched = Touched(1 << 3);
    /// Blending on or off, its equations and factors.
    pub(super) const BLEND: Touched = Touched(1 << 4);
    /// The colour write mask and the clear colour.
    pub(super) const MASKS: Touched = Touched(1 << 5);
    /// The depth and stencil tests and face culling.
    pub(super) const TESTS: Touched = Touched(1 << 6);
    /// The active unit and each unit's texture and sampler.
    pub(super) const TEXTURES: Touched = Touched(1 << 7);
    /// The generic array and uniform buffer bindings and the uniform buffer ranges.
    pub(super) const BUFFERS: Touched = Touched(1 << 8);
    /// Pack and unpack alignment and the unpack row length.
    pub(super) const PIXELS: Touched = Touched(1 << 9);

    pub(super) fn with(self, other: Touched) -> Touched {
        Touched(self.0 | other.0)
    }

    pub(super) fn has(self, other: Touched) -> bool {
        self.0 & other.0 != 0
    }
}

fn set(gl: &glow::Context, cap: u32, on: bool) {
    // Safety: a plain enable of the current context.
    unsafe {
        if on {
            gl.enable(cap)
        } else {
            gl.disable(cap)
        }
    }
}

/// Put back the groups `touched` of `s`, and the vertex array, which the device always binds its own of.
pub(super) fn restore(gl: &glow::Context, s: &BorrowedState, touched: Touched) {
    // Safety: the values were read from this context by `capture`, so each names an object of it or none.
    unsafe {
        gl.bind_vertex_array(s.vertex_array);
        if touched.has(Touched::PROGRAM) {
            gl.use_program(s.program);
        }
        if touched.has(Touched::FRAMEBUFFERS) {
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, s.draw_framebuffer);
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, s.read_framebuffer);
        }
        if touched.has(Touched::VIEWPORT) {
            let [x, y, w, h] = s.viewport;
            gl.viewport(x, y, w, h);
        }
        if touched.has(Touched::SCISSOR) {
            let [x, y, w, h] = s.scissor_box;
            gl.scissor(x, y, w, h);
            set(gl, glow::SCISSOR_TEST, s.scissor_test);
        }
        if touched.has(Touched::BLEND) {
            set(gl, glow::BLEND, s.blend);
            gl.blend_equation_separate(s.blend_equation[0] as u32, s.blend_equation[1] as u32);
            let [a, b, c, d] = s.blend_factors.map(|f| f as u32);
            gl.blend_func_separate(a, b, c, d);
        }
        if touched.has(Touched::MASKS) {
            let [r, g, b, a] = s.color_mask;
            gl.color_mask(r, g, b, a);
            let [r, g, b, a] = s.clear_color;
            gl.clear_color(r, g, b, a);
        }
        if touched.has(Touched::TESTS) {
            set(gl, glow::DEPTH_TEST, s.depth_test);
            set(gl, glow::STENCIL_TEST, s.stencil_test);
            set(gl, glow::CULL_FACE, s.cull_face);
        }
        if touched.has(Touched::TEXTURES) {
            for (unit, (texture, sampler)) in s.textures.iter().enumerate() {
                gl.active_texture(glow::TEXTURE0 + unit as u32);
                gl.bind_texture(glow::TEXTURE_2D, *texture);
                gl.bind_sampler(unit as u32, *sampler);
            }
            gl.active_texture(s.active_texture as u32);
        }
        if touched.has(Touched::BUFFERS) {
            for (i, &(buffer, start, size)) in s.uniform_ranges.iter().enumerate() {
                let buffer = std::num::NonZeroU32::new(buffer as u32).map(glow::NativeBuffer);
                if buffer.is_some() && size > 0 {
                    gl.bind_buffer_range(glow::UNIFORM_BUFFER, i as u32, buffer, start, size);
                } else {
                    gl.bind_buffer_base(glow::UNIFORM_BUFFER, i as u32, buffer);
                }
            }
            // after the indexed bindings, which set the generic one too
            gl.bind_buffer(glow::UNIFORM_BUFFER, s.uniform_buffer);
            gl.bind_buffer(glow::ARRAY_BUFFER, s.array_buffer);
        }
        if touched.has(Touched::PIXELS) {
            gl.pixel_store_i32(glow::PACK_ALIGNMENT, s.pack_alignment);
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, s.unpack_alignment);
            gl.pixel_store_i32(glow::UNPACK_ROW_LENGTH, s.unpack_row_length);
        }
    }
}

impl BorrowedState {
    /// The owner's draw framebuffer and viewport, as they were read.
    pub(super) fn target(&self) -> (Option<glow::Framebuffer>, [i32; 4]) {
        (self.draw_framebuffer, self.viewport)
    }
}
