#version 300 es
// Vibrant blending for template glyphs on materials (port of src/shaders/vibrant.wgsl).
//
// vibrantDark(s, d) = lightenSover(s*c1, colorDodge(s*c0, d)),
// vibrantLight(s, d) = darkenSover(s*c1, colorBurn(s*c0, d)); reversed swaps the nesting order.
// c0 / c1 are unknown effect constants supplied by the caller. u_backdrop must hold a copy of the
// current framebuffer content; u_glyphs is premultiplied. No hardware blending.
precision highp float;
precision highp int;
precision highp sampler2D;

layout(std140) uniform Vibrant {
    vec4 rect;
    vec4 uv;
    vec4 sizes;
    vec4 mode;
    vec4 c0;
    vec4 c1;
} V;

uniform sampler2D u_glyphs;
uniform sampler2D u_backdrop;

in vec2 v_tex;
layout(location = 0) out vec4 frag_color;

const vec3 LUMA709 = vec3(0.2125, 0.7154, 0.0721);

vec4 lighten_sover(vec4 s_in, vec4 d) {
    float y = 1.0 - dot(d.rgb, LUMA709);
    vec4 s = s_in * (y * y) * (y * y);
    return d * (1.0 - s.a) + s;
}

vec4 darken_sover(vec4 s_in, vec4 d) {
    float y = dot(d.rgb, LUMA709);
    vec4 s = s_in * (y * y) * (y * y);
    return d * (1.0 - s.a) + s;
}

vec4 color_dodge(vec4 s, vec4 d) {
    vec4 r = d * (1.0 - s.a) + s * (1.0 - d.a);
    vec3 t = vec3(s.a) - s.rgb;
    vec3 m = step(vec3(0.005), t);
    r.rgb += mix(vec3(s.a), d.rgb * s.a * s.a / max(t, vec3(0.005)), m);
    r.a += d.a * s.a;
    r.rgb = clamp(r.rgb, vec3(0.0), vec3(r.a));
    return r;
}

vec4 color_burn(vec4 s, vec4 d) {
    vec4 r = d * (1.0 - s.a) + s * (1.0 - d.a);
    float dasa = d.a * s.a;
    r.rgb += step(vec3(0.005), s.rgb) * (vec3(dasa) - s.a * s.a * (vec3(d.a) - d.rgb) / max(s.rgb, vec3(0.005)));
    r.a += dasa;
    r.rgb = clamp(r.rgb, vec3(0.0), vec3(r.a));
    return r;
}

vec4 vibrant_dark(vec4 s0, vec4 d0, bool reversed) {
    if (reversed) {
        return color_dodge(s0 * V.c0, lighten_sover(s0 * V.c1, d0));
    }
    return lighten_sover(s0 * V.c1, color_dodge(s0 * V.c0, d0));
}

vec4 vibrant_light(vec4 s0, vec4 d0, bool reversed) {
    if (reversed) {
        return color_burn(s0 * V.c0, darken_sover(s0 * V.c1, d0));
    }
    return darken_sover(s0 * V.c1, color_burn(s0 * V.c0, d0));
}

void main() {
    vec4 s0 = texture(u_glyphs, v_tex);
    vec4 d0 = vec4(texelFetch(u_backdrop, ivec2(gl_FragCoord.xy), 0).rgb, 1.0);
    bool reversed = V.mode.y > 0.5;
    vec4 r;
    if (V.mode.x > 0.5) { r = vibrant_light(s0, d0, reversed); } else { r = vibrant_dark(s0, d0, reversed); }
    frag_color = vec4(clamp(r.rgb, vec3(0.0), vec3(1.0)), 1.0);
}
