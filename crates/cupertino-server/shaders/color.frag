#version 300 es
// Colour filter chain (port of fs_color in src/shaders/passes.wgsl).
// luminance curve, saturation,
// monochrome approximate. Display filters: matrix values on linear light;
// invert and contrast on the encoded values.
precision highp float;
precision highp int;
precision highp sampler2D;

struct Op {
    vec4 a;  // kind (1 saturate, 2 luminance curve, 3 monochrome, 4 linear matrix, 5 invert,
             // 6 contrast, 7 matrix tail), amount, 0, 0; matrix: kind, row 0
    vec4 b;  // luminance curve: Horner coefficients; monochrome: colour; matrix: row 1, m20
};

layout(std140) uniform PassParams {
    ivec4 sizes;
    ivec4 dir_radius;  // ..., w = number of colour ops
    vec4 weights[9];
    Op ops[8];
} P;

uniform sampler2D u_src;

layout(location = 0) out vec4 frag_color;

const vec3 LUMA709 = vec3(0.2125, 0.7154, 0.0721);
const vec3 SAT709 = vec3(0.2126, 0.7152, 0.0722);

vec3 srgb_decode(vec3 c) {
    return mix(c / 12.92, pow((c + 0.055) / 1.055, vec3(2.4)), step(vec3(0.04045), c));
}

vec3 srgb_encode(vec3 c) {
    return mix(c * 12.92, 1.055 * pow(c, vec3(1.0 / 2.4)) - 0.055, step(vec3(0.0031308), c));
}

// `next` is the operation after `op` (the tail of a linear matrix).
vec3 color_op(vec3 c, Op op, Op next) {
    int k = int(op.a.x);
    if (k == 1) {
        float s = op.a.y;
        float l = dot(c, SAT709);
        return vec3(l) * (1.0 - s) + c * s;
    } else if (k == 2) {
        float x0 = dot(c, LUMA709);
        vec4 p = op.b;
        float x = clamp(((p.x * x0 + p.y) * x0 + p.z) * x0 + p.w, 0.0, 1.0);
        return mix(c, vec3(x), op.a.y);
    } else if (k == 3) {
        float l = dot(c, LUMA709);
        return mix(c, l * op.b.xyz, op.a.y);
    } else if (k == 4) {
        mat3 m = mat3(op.a.y, op.b.x, op.b.w, op.a.z, op.b.y, next.a.y, op.a.w, op.b.z, next.a.z);
        return srgb_encode(clamp(m * srgb_decode(clamp(c, 0.0, 1.0)), 0.0, 1.0));
    } else if (k == 5) {
        return vec3(1.0) - c;
    } else if (k == 6) {
        float t = op.a.y;
        if (t >= 1.0) { return step(vec3(0.5), c); }
        return clamp((c - vec3(0.5 * t)) / (1.0 - t), 0.0, 1.0);
    }
    return c;
}

void main() {
    vec3 rgb = texelFetch(u_src, ivec2(gl_FragCoord.xy), 0).rgb;
    int n = P.dir_radius.w;
    for (int i = 0; i < n; i++) {
        rgb = color_op(rgb, P.ops[i], P.ops[min(i + 1, 7)]);
    }
    frag_color = vec4(rgb, 1.0);
}
