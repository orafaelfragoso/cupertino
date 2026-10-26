#version 300 es
// Glyph quad for vibrant blending (port of vs_vibrant in src/shaders/vibrant.wgsl).
// Draw as a 4-vertex triangle strip.
precision highp float;
precision highp int;

layout(std140) uniform Vibrant {
    vec4 rect;   // x, y, w, h in target pixels (y down)
    vec4 uv;     // glyph texture region (u0, v0, u1, v1)
    vec4 sizes;  // target w, h, 0, 0
    vec4 mode;   // x: 0 = dark, 1 = light; y: reversed (0/1)
    vec4 c0;
    vec4 c1;
} V;

out vec2 v_tex;

void main() {
    vec2 corner = vec2(float(gl_VertexID & 1), float((gl_VertexID >> 1) & 1));
    vec2 p = V.rect.xy + corner * V.rect.zw;
    gl_Position = vec4(p.x / V.sizes.x * 2.0 - 1.0, 1.0 - p.y / V.sizes.y * 2.0, 0.0, 1.0);
    v_tex = mix(V.uv.xy, V.uv.zw, corner);
}
