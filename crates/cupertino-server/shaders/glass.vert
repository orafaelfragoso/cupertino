#version 300 es
// Glass composite quad (port of vs_glass in src/shaders/glass.wgsl). Draw as a 4-vertex triangle strip, one draw per
// damage rectangle: u_quad (x, y, w, h in target pixels, y down) is mapped through the frame projection u_proj
// (column-major), so the output transform is honoured.
precision highp float;

uniform mat3 u_proj;
uniform vec4 u_quad;

out vec2 v_px;

void main() {
    vec2 corner = vec2(float(gl_VertexID & 1), float((gl_VertexID >> 1) & 1));
    vec2 p = u_quad.xy + corner * u_quad.zw;
    vec3 c = u_proj * vec3(p, 1.0);
    gl_Position = vec4(c.xy, 0.0, 1.0);
    v_px = p;
}
