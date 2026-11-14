#version 300 es
// Separable Gaussian, clamp-to-edge (port of fs_blur in src/shaders/passes.wgsl).
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
    vec4 weights[9];   // one-sided Gaussian weights, tap i at weights[i / 4][i % 4]
    Op ops[8];
} P;

uniform sampler2D u_src;

layout(location = 0) out vec4 frag_color;

//_DITHER_

vec4 ld(ivec2 p) {
    ivec2 q = clamp(p, ivec2(0), P.sizes.xy - ivec2(1));
    return texelFetch(u_src, q, 0);
}

float weight(int i) {
    return P.weights[i / 4][i % 4];
}

void main() {
    ivec2 p = ivec2(gl_FragCoord.xy);
    ivec2 dir = P.dir_radius.xy;
    int radius = P.dir_radius.z;
    vec4 acc = ld(p) * weight(0);
    for (int i = 1; i <= radius; i++) {
        ivec2 o = dir * i;
        acc += (ld(p + o) + ld(p - o)) * weight(i);
    }
    frag_color = vec4(acc.rgb, 1.0);
#ifdef CUPERTINO_DITHER
    frag_color.rgb = dither_rgb(frag_color.rgb, gl_FragCoord.xy);
#endif
}
