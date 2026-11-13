#version 300 es
// Glass composite (port of fs_glass in src/shaders/glass.wgsl; the stages and values are described in src/glass.rs).
// Lengths are target pixels, y down; colours are sRGB-encoded throughout.
// The level textures are sampled through the uniform's affine map from target pixels to their coordinates. Use the
// blend state (SRC_ALPHA, ONE_MINUS_SRC_ALPHA) for colour and (ZERO, ONE) for alpha.
precision highp float;
precision highp int;
precision highp sampler2D;

layout(std140) uniform Glass {
    vec4 quad;
    vec4 uv_x;
    vec4 uv_y;
    vec4 info;        // shapes, blend width, antialiasing width, opacity
    vec4 levels;      // unused (0, 0), target w, h
    vec4 fill;        // blur fill: darken, lighten, normal
    vec4 face;        // saturation, white, black, clamp
    vec4 face_fill;
    vec4 bleed;       // white, black, saturation, darken
    vec4 highlight;   // amount, angle, spread, half-width
    vec4 highlight2;  // offset, colour bias, curvature
    vec4 glow;        // soft band: amount, half-width, offset
    vec4 hm[3];
    vec4 shadow;      // offset x, offset y, alpha, blur
    vec4 shadow_color;// rgb, ring alpha
    vec4 ring;        // offset, stroke, blur
    vec4 shapes[16];
    vec4 shape_a[16]; // radius, refraction offset at the edge, refraction band, bleed fraction
    vec4 shape_b[16]; // bleed band, largest luminance, where the shape's blur lies among the blurred copies
} G;

uniform sampler2D u_level0;
uniform sampler2D u_level1;
uniform sampler2D u_level2;
uniform sampler2D u_level3;
uniform sampler2D u_fill;

in vec2 v_px;
layout(location = 0) out vec4 frag_color;

const vec3 LUMA = vec3(0.2126, 0.7152, 0.0722);
const float PI = 3.14159265;

float rect_sdf(vec2 p, vec4 r, float radius) {
    vec2 hs = r.zw * 0.5;
    float rad = clamp(radius, 0.0, min(hs.x, hs.y));
    vec2 q = abs(p - (r.xy + hs)) - (hs - vec2(rad));
    return length(max(q, vec2(0.0))) + min(max(q.x, q.y), 0.0) - rad;
}

// Smooth union of every shape; the per-shape values are mixed with the same weights.
float field(vec2 p, out vec4 a, out vec4 b) {
    int n = int(G.info.x);
    float k = G.info.y;
    float d = rect_sdf(p, G.shapes[0], G.shape_a[0].x);
    a = G.shape_a[0];
    b = G.shape_b[0];
    for (int i = 1; i < n; i++) {
        float di = rect_sdf(p, G.shapes[i], G.shape_a[i].x);
        float h;
        if (k <= 0.0) {
            h = di <= d ? 1.0 : 0.0;
            d = min(di, d);
        } else {
            h = clamp(0.5 + 0.5 * (d - di) / k, 0.0, 1.0);
            d = mix(d, di, h) - k * h * (1.0 - h);
        }
        a = mix(a, G.shape_a[i], h);
        b = mix(b, G.shape_b[i], h);
    }
    return d;
}

float field_d(vec2 p) {
    vec4 a;
    vec4 b;
    return field(p, a, b);
}

vec2 uv_of(vec2 p) {
    vec3 h = vec3(p, 1.0);
    return vec2(dot(G.uv_x.xyz, h), dot(G.uv_y.xyz, h));
}

// saturation about the Rec. 709 luma of encoded values
vec3 saturate_encoded(vec3 c, float s) {
    float y = dot(c, LUMA);
    return max(vec3(y) + (c - vec3(y)) * s, vec3(0.0));
}

// Stage 1: the blurred copy of the shape's blur at `pos` among them, or the two around it mixed; a copy of no weight
// is not read.
vec3 blurred(vec2 uv, float pos) {
    vec4 w = max(vec4(1.0) - abs(vec4(clamp(pos, 0.0, 3.0)) - vec4(0.0, 1.0, 2.0, 3.0)), vec4(0.0));
    vec3 c = vec3(0.0);
    if (w.x > 0.0) { c += w.x * textureLod(u_level0, uv, 0.0).rgb; }
    if (w.y > 0.0) { c += w.y * textureLod(u_level1, uv, 0.0).rgb; }
    if (w.z > 0.0) { c += w.z * textureLod(u_level2, uv, 0.0).rgb; }
    if (w.w > 0.0) { c += w.w * textureLod(u_level3, uv, 0.0).rgb; }
    return c;
}

// Stages 1 and 2: the shape's blur, the blur fill and the face colour at q.
vec3 face_at(vec2 q, float pos, float max_luma) {
    vec2 uv = uv_of(q);
    vec3 c = blurred(uv, pos);
    vec3 f = textureLod(u_fill, uv, 0.0).rgb;
    c = mix(c, min(c, f), G.fill.x);
    c = mix(c, max(c, f), G.fill.y);
    c = mix(c, f, G.fill.z);
    // stage 2, on encoded values: saturation, luma compression, levels, limit, fill
    vec3 s = saturate_encoded(c, G.face.x);
    float l = dot(s, LUMA);
    s = s - (1.0 - max_luma) * l * (0.75 * s + vec3(0.25 * l));
    l = dot(s, LUMA);
    s = s + vec3(G.face.z * (1.0 - l) - (1.0 - G.face.y) * l);
    vec3 e = clamp(s, vec3(0.0), vec3(min(G.face.w, 1.0)));
    return mix(e, G.face_fill.rgb, G.face_fill.a);
}

float angle_between(float a, float b) {
    float t = 2.0 * PI;
    float d = a - b - floor((a - b) / t) * t;
    return min(d, t - d);
}

// a raised cosine of the angle from the light, nothing past LOBE_REACH of the spread
const float LOBE_REACH = 0.75;
// the shortest field slope the rim's distance to the edge divides by
const float MIN_FIELD_SLOPE = 0.05;

float lobe(float normal_angle, float light) {
    float reach = G.highlight.z * LOBE_REACH;
    if (reach <= 0.0) { return 0.0; }
    float a = angle_between(normal_angle, light);
    if (a >= reach) { return 0.0; }
    return pow(0.5 + 0.5 * cos(a / reach * PI), max(G.highlight2.z, 0.001));
}

vec3 key_color(vec3 c) {
    vec4 h = vec4(c, 1.0);
    return clamp(vec3(dot(G.hm[0], h), dot(G.hm[1], h), dot(G.hm[2], h)), vec3(0.0), vec3(1.0));
}

float coverage(float d, float r) {
    if (r <= 0.0) { return d <= 0.0 ? 1.0 : 0.0; }
    float t = clamp(0.5 - d / (3.0 * r), 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

void main() {
    vec2 p = v_px;
    vec4 fa;
    vec4 fb;
    float d = field(p, fa, fb);
    if (d > 0.5) {
        // stage 6: shadows outside the shape, drawn at their opacity (none at rest)
        bool outlined = G.glow.w > 0.0 && G.ring.w > 0.0;
        if (G.shadow.z <= 0.0 && G.shadow_color.w <= 0.0 && !outlined) { discard; }
        float drop = G.shadow.z * coverage(field_d(p - G.shadow.xy), G.shadow.w);
        float dr = field_d(p - vec2(0.0, G.ring.x));
        float ring = G.shadow_color.w * max(coverage(dr, G.ring.z) - coverage(dr + G.ring.y, G.ring.z), 0.0);
        // a hairline of the shadow's colour along the outside of the shape
        float line = outlined ? G.glow.w * clamp(G.ring.w + 0.5 - d, 0.0, 1.0) : 0.0;
        float a = min(drop + ring + line, 1.0) * G.info.w;
        if (a <= 0.0) { discard; }
        frag_color = vec4(G.shadow_color.rgb, a);
        return;
    }
    vec2 g = vec2(field_d(p + vec2(0.5, 0.0)) - field_d(p - vec2(0.5, 0.0)),
                  field_d(p + vec2(0.0, 0.5)) - field_d(p - vec2(0.0, 0.5)));
    float gl = length(g);
    vec2 n = gl > 1e-6 ? g / gl : vec2(0.0);
    // stage 3: refraction inside the band along the edge
    vec2 off = vec2(0.0);
    if (fa.z > 0.0 && d <= 0.0) {
        float t = clamp(-d / fa.z, 0.0, 1.0);
        off = n * fa.y * (1.0 - t) * (1.0 - t);
    }
    vec3 c = face_at(p + off, fb.z, fb.y);
    // stage 4: bleed of the blur fill
    if (fa.w > 0.0 && fb.x > 0.0) {
        float t = clamp(-d / fb.x, 0.0, 1.0);
        float w = fa.w * (1.0 - t) * (1.0 - t);
        vec3 tone = vec3(G.bleed.y) + (G.bleed.x - G.bleed.y)
            * min(saturate_encoded(textureLod(u_fill, uv_of(p), 0.0).rgb, G.bleed.z), vec3(1.0));
        vec3 b = G.bleed.w > 0.5 ? min(c, tone) : max(c, tone);
        c = mix(c, b, w);
    }
    // stage 5: key and fill rim lights, added along the light's axis
    bool rim_on = G.highlight.x > 0.0 && G.highlight.w > 0.0;
    bool glow_on = G.glow.x > 0.0 && G.glow.y > 0.0;
    if ((rim_on || glow_on) && gl > 1e-6) {
        float na = atan(-n.y, n.x);
        float ends = lobe(na, G.highlight.y) + lobe(na, G.highlight.y + PI) * max(1.0 + G.highlight2.y, 0.0);
        // the distance to the edge the field's slope gives: far inside along a seam between joined shapes
        float de = d / max(gl, MIN_FIELD_SLOPE);
        float strength = 0.0;
        if (rim_on) {
            float x = (de - G.highlight2.x) / G.highlight.w;
            strength = G.highlight.x * exp(-x * x);
        }
        // a soft band wider than the rim, lighting the same ends of the axis
        if (glow_on) {
            float xg = (de - G.glow.z) / G.glow.y;
            strength += G.glow.x * exp(-xg * xg);
        }
        c = min(c + key_color(c) * (strength * ends), vec3(1.0));
    }
    float cov = clamp(0.5 - d / G.info.z, 0.0, 1.0) * G.info.w;
    frag_color = vec4(clamp(c, vec3(0.0), vec3(1.0)), cov);
}
