#version 300 es
// Material region: SDF rounded mask, backdrop (or source) base colour, fill layers with their
// compositing filters (port of fs_material in src/shaders/material.wgsl). Use the blend state
// (SRC_ALPHA, ONE_MINUS_SRC_ALPHA) for colour and (ZERO, ONE) for alpha.
//
// fill layers and blend modes follow the decoded layer archives;
// plus-lighter / plus-darker and the separable blend modes are as designed; the tint layer colour
// is approximate; the continuous-corner shape is approximate (cupertino-geometry).
//
// Texture orientation (compositor copy): v_px is y-down output pixels; the backdrop is sampled with
// v_uv, the framebuffer-normalised coordinate computed by the vertex shader from the frame
// projection, so no row flip is needed. The backdrop may cover only part of the framebuffer, the
// part around the region it was blurred for: Region.info.zw is its origin and Region.sizes.zw its
// size, in framebuffer pixels.
precision highp float;
precision highp int;
precision highp sampler2D;

layout(std140) uniform Region {
    vec4 rect;
    vec4 shape;   // corner extent (px), superellipse exponent, opacity, AA width (px)
    vec4 info;    // number of fills, has backdrop, flip rows, 0
    vec4 sizes;   // target w, h, source w, h
    vec4 tint;
    vec4 fills[16];  // per fill: (blend id, tint flag, 0, 0), straight rgba
} R;

uniform sampler2D u_backdrop;

in vec2 v_px;
in vec2 v_uv;
layout(location = 0) out vec4 frag_color;

//_DITHER_

// Same formula as cupertino_geometry::RoundedRect::sdf_approx.
float region_sdf(vec2 p, vec2 hs, float e, float n) {
    vec2 q = abs(p) - (hs - vec2(e));
    if (q.x <= 0.0 && q.y <= 0.0) { return max(q.x, q.y) - e; }
    if (q.x <= 0.0) { return q.y - e; }
    if (q.y <= 0.0) { return q.x - e; }
    if (e <= 0.0) { return length(q); }
    if (n <= 2.001) { return length(q) - e; }
    float u = max(q.x / e, 1e-6);
    float v = max(q.y / e, 1e-6);
    float g = pow(pow(u, n) + pow(v, n), 1.0 / n);
    float grad = sqrt(pow(u, 2.0 * n - 2.0) + pow(v, 2.0 * n - 2.0)) / pow(g, n - 1.0);
    float d = (g - 1.0) / grad * e;
    return max(d, max(q.x, q.y) - e);
}

float soft_light(float s, float d) {
    if (s <= 0.5) { return d - (1.0 - 2.0 * s) * d * (1.0 - d); }
    float g;
    if (d <= 0.25) { g = ((16.0 * d - 12.0) * d + 4.0) * d; } else { g = sqrt(d); }
    return d + (2.0 * s - 1.0) * (g - d);
}

// d is opaque; s = straight source colour, a = source alpha.
// Blend ids: 0 normal, 1 plus-lighter, 2 plus-darker, 3 darken, 4 lighten, 5 soft-light, 6 copy.
vec3 blend(int mode, vec3 s, float a, vec3 d) {
    vec3 b;
    switch (mode) {
        case 1: b = min(vec3(1.0), s + d); break;
        case 2: b = max(vec3(0.0), s + d - vec3(1.0)); break;
        case 3: b = min(s, d); break;
        case 4: b = max(s, d); break;
        case 5: b = vec3(soft_light(s.r, d.r), soft_light(s.g, d.g), soft_light(s.b, d.b)); break;
        case 6: return s * a;
        default: b = s; break;
    }
    return d * (1.0 - a) + b * a;
}

void main() {
    vec2 hs = R.rect.zw * 0.5;
    float d = region_sdf(v_px - (R.rect.xy + hs), hs, R.shape.x, R.shape.y);
    float cov = clamp(0.5 - d / R.shape.w, 0.0, 1.0) * R.shape.z;
    if (cov <= 0.0) { discard; }
    // The backdrop is always bound: recipes without a backdrop filter use an unfiltered copy of
    // the framebuffer. It shares the orientation of the framebuffer.
    vec2 uv = (v_uv * R.sizes.xy - R.info.zw) / R.sizes.zw;
    vec3 base = texture(u_backdrop, uv).rgb;
    int n = int(R.info.x);
    for (int i = 0; i < n; i++) {
        vec4 a = R.fills[2 * i];
        vec4 b = R.fills[2 * i + 1];
        vec3 s = b.xyz;
        if (a.y > 0.5) { s = R.tint.rgb; }
        base = blend(int(a.x), s, b.w, base);
    }
    frag_color = vec4(clamp(base, vec3(0.0), vec3(1.0)), cov);
#ifdef CUPERTINO_DITHER
    frag_color.rgb = dither_rgb(frag_color.rgb, gl_FragCoord.xy);
#endif
}
