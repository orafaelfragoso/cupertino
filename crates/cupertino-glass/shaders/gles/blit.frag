#version 300 es
// Copies the source into the target (port of fs_blit in src/shaders/passes.wgsl).
precision highp float;
precision highp int;
precision highp sampler2D;

struct Op {
    vec4 a;
    vec4 b;
};

layout(std140) uniform PassParams {
    ivec4 sizes;  // src w, src h, dst w, dst h
    ivec4 dir_radius;
    vec4 weights[9];
    Op ops[8];
} P;

uniform sampler2D u_src;

layout(location = 0) out vec4 frag_color;

void main() {
    vec2 uv = gl_FragCoord.xy / vec2(P.sizes.zw);
    frag_color = vec4(texture(u_src, uv).rgb, 1.0);
}
