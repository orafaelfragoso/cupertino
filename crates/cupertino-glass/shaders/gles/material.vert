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

out vec2 v_px;

void main() {
    vec2 corner = vec2(float(gl_VertexID & 1), float((gl_VertexID >> 1) & 1));
    float pad = 1.0 + R.shape.w;
    vec2 p = R.rect.xy - vec2(pad) + corner * (R.rect.zw + vec2(2.0 * pad));
    gl_Position = vec4(p.x / R.sizes.x * 2.0 - 1.0, 1.0 - p.y / R.sizes.y * 2.0, 0.0, 1.0);
    v_px = p;
}
