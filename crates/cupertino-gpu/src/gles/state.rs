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
//! [`Device::enter`]: super::Device::enter
//! [`Device::leave`]: super::Device::leave

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

pub(super) fn restore(gl: &glow::Context, s: &BorrowedState) {
    // Safety: the values were read from this context by `capture`, so each names an object of it or none.
    unsafe {
        gl.bind_vertex_array(s.vertex_array);
        gl.use_program(s.program);
        gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, s.draw_framebuffer);
        gl.bind_framebuffer(glow::READ_FRAMEBUFFER, s.read_framebuffer);
        let [x, y, w, h] = s.viewport;
        gl.viewport(x, y, w, h);
        let [x, y, w, h] = s.scissor_box;
        gl.scissor(x, y, w, h);
        set(gl, glow::SCISSOR_TEST, s.scissor_test);
        set(gl, glow::BLEND, s.blend);
        gl.blend_equation_separate(s.blend_equation[0] as u32, s.blend_equation[1] as u32);
        let [a, b, c, d] = s.blend_factors.map(|f| f as u32);
        gl.blend_func_separate(a, b, c, d);
        let [r, g, b, a] = s.color_mask;
        gl.color_mask(r, g, b, a);
        let [r, g, b, a] = s.clear_color;
        gl.clear_color(r, g, b, a);
        set(gl, glow::DEPTH_TEST, s.depth_test);
        set(gl, glow::STENCIL_TEST, s.stencil_test);
        set(gl, glow::CULL_FACE, s.cull_face);
        for (unit, (texture, sampler)) in s.textures.iter().enumerate() {
            gl.active_texture(glow::TEXTURE0 + unit as u32);
            gl.bind_texture(glow::TEXTURE_2D, *texture);
            gl.bind_sampler(unit as u32, *sampler);
        }
        gl.active_texture(s.active_texture as u32);
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
        gl.pixel_store_i32(glow::PACK_ALIGNMENT, s.pack_alignment);
        gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, s.unpack_alignment);
        gl.pixel_store_i32(glow::UNPACK_ROW_LENGTH, s.unpack_row_length);
    }
}
