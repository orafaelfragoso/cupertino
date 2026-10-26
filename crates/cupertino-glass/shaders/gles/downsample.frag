#version 300 es
// 2x box downsample (port of fs_downsample in src/shaders/passes.wgsl).
// Pass uniforms use std140, byte-identical to PassParams in src/ops.rs.
precision highp float;
precision highp int;
precision highp sampler2D;

struct Op {
    vec4 a;
    vec4 b;
};

layout(std140) uniform PassParams {
    ivec4 sizes;       // src w, src h, dst w, dst h
    ivec4 dir_radius;  // dir x, dir y, blur radius, number of colour ops
    vec4 weights[9];
    Op ops[8];
} P;

uniform sampler2D u_src;

layout(location = 0) out vec4 frag_color;

vec4 ld(ivec2 p) {
    ivec2 q = clamp(p, ivec2(0), P.sizes.xy - ivec2(1));
    return texelFetch(u_src, q, 0);
}

void main() {
    ivec2 p = ivec2(gl_FragCoord.xy) * 2;
    vec4 c = (ld(p) + ld(p + ivec2(1, 0)) + ld(p + ivec2(0, 1)) + ld(p + ivec2(1, 1))) * 0.25;
    frag_color = vec4(c.rgb, 1.0);
}
