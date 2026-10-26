#version 300 es
// Material region quad (port of vs_region in src/shaders/material.wgsl). Draw as a 4-vertex
// triangle strip. Pixel coordinates are y-down; clip space has +y up.
precision highp float;
precision highp int;

layout(std140) uniform Region {
    vec4 rect;    // x, y, w, h in target pixels (y down)
    vec4 shape;   // corner extent (px), superellipse exponent, opacity, AA width (px)
    vec4 info;    // number of fills, has backdrop, source rows start at the bottom (0/1), 0
    vec4 sizes;   // target w, h, source w, h
    vec4 tint;    // tint layer colour (rgb)
    vec4 fills[16];
} R;

// Compositor integration: the quad (u_quad: x, y, w, h in output pixels, one draw per damage
// rectangle) is mapped through the frame projection (u_proj, column-major) so the output
// transform is honoured; v_uv is the matching framebuffer-normalised coordinate used to sample
// the captured backdrop.
uniform mat3 u_proj;
uniform vec4 u_quad;

out vec2 v_px;
out vec2 v_uv;

void main() {
    vec2 corner = vec2(float(gl_VertexID & 1), float((gl_VertexID >> 1) & 1));
    vec2 p = u_quad.xy + corner * u_quad.zw;
    vec3 c = u_proj * vec3(p, 1.0);
    gl_Position = vec4(c.xy, 0.0, 1.0);
    v_px = p;
    v_uv = c.xy * 0.5 + vec2(0.5);
}
