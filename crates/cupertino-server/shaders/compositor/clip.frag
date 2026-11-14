//_DEFINES_
// Rounded-corner mask for window surface textures. The mask is evaluated from the fragment's
// framebuffer position, mapped back to output pixels through the inverse of the frame projection,
// so it is independent of the output transform.
#if defined(EXTERNAL)
#extension GL_OES_EGL_image_external : require
#endif
#ifdef GL_FRAGMENT_PRECISION_HIGH
precision highp float;
#else
precision mediump float;
#endif
#if defined(EXTERNAL)
uniform samplerExternalOES tex;
#else
uniform sampler2D tex;
#endif
uniform float alpha;
varying vec2 v_coords;
#if defined(DEBUG_FLAGS)
uniform float tint;
#endif
uniform mat3 inv_proj;   // clip space -> output pixels
uniform vec2 viewport;   // framebuffer size in pixels
uniform vec4 clip_rect;  // x, y, w, h in output pixels
uniform vec4 clip_shape; // radius (px), corner exponent, 0, 0
#if defined(MINIFY)
// A texture drawn smaller than its texels: each pixel is the average of the texels it covers, `footprint` texels a
// side about its centre (at most 16), each texel weighted by how much of it the pixel covers. `texels` is the
// texture's size; a lookup at a texel's centre reads that texel alone, and past the edge the edge's texel.
uniform vec2 footprint;
uniform vec2 texels;

// One axis of a box from `lo` to `hi` (texels) over at most three texels, as two filtered lookups: the first two
// texels blended where their weights put the lookup, and the third at its centre. Places in `at`, weights in `weight`.
void two_taps(float lo, float hi, out vec2 at, out vec2 weight) {
    float first = floor(lo);
    float w0 = min(first + 1.0, hi) - lo;
    float w1 = clamp(min(first + 2.0, hi) - (first + 1.0), 0.0, 1.0);
    float w2 = max(hi - (first + 2.0), 0.0);
    float pair = w0 + w1;
    at = vec2(first + 0.5 + w1 / max(pair, 1e-6), first + 2.5);
    weight = vec2(pair, w2);
}

vec4 covered() {
    vec2 centre = v_coords * texels;
    vec2 lo = centre - footprint * 0.5;
    vec2 hi = centre + footprint * 0.5;
    if (footprint.x <= 2.0 && footprint.y <= 2.0) {
        // up to two texels a side: four lookups give the weights exactly, as the products of each axis's
        vec2 ax, wx, ay, wy;
        two_taps(lo.x, hi.x, ax, wx);
        two_taps(lo.y, hi.y, ay, wy);
        vec4 c = texture2D(tex, vec2(ax.x, ay.x) / texels) * (wx.x * wy.x)
            + texture2D(tex, vec2(ax.y, ay.x) / texels) * (wx.y * wy.x)
            + texture2D(tex, vec2(ax.x, ay.y) / texels) * (wx.x * wy.y)
            + texture2D(tex, vec2(ax.y, ay.y) / texels) * (wx.y * wy.y);
        return c / max((wx.x + wx.y) * (wy.x + wy.y), 1e-6);
    }
    vec2 first = floor(lo);
    vec4 sum = vec4(0.0);
    float total = 0.0;
    for (int y = 0; y < 17; y++) {
        float ty = first.y + float(y);
        if (ty >= hi.y) break;
        float wy = min(ty + 1.0, hi.y) - max(ty, lo.y);
        for (int x = 0; x < 17; x++) {
            float tx = first.x + float(x);
            if (tx >= hi.x) break;
            float wx = min(tx + 1.0, hi.x) - max(tx, lo.x);
            vec2 at = (clamp(vec2(tx, ty), vec2(0.0), texels - 1.0) + 0.5) / texels;
            sum += texture2D(tex, at) * (wx * wy);
            total += wx * wy;
        }
    }
    return sum / max(total, 1e-6);
}
#endif
#if defined(VIBRANT)
// x: 1 over a dark face, 0 over a light one; y: the coverage's weight; z: 1 for the plain label colour
uniform vec4 vibrant;
// how far towards white a full coverage lifts what lies behind a dark face; the blend screens it in
#define VIBRANT_LIFT 0.95
#endif

//_SDF_

void main() {
#if defined(MINIFY)
    vec4 color = covered();
#else
    vec4 color = texture2D(tex, v_coords);
#endif
#if defined(NO_ALPHA)
    color = vec4(color.rgb, 1.0);
#endif
    vec2 ndc = gl_FragCoord.xy / viewport * 2.0 - 1.0;
    vec2 p = (inv_proj * vec3(ndc, 1.0)).xy;
    vec2 hs = clip_rect.zw * 0.5;
    float radius = min(clip_shape.x, min(hs.x, hs.y));
    float d = rounded_sdf(p - (clip_rect.xy + hs), hs, radius, clip_shape.y);
    float cov = clamp(0.5 - d, 0.0, 1.0);
    color = color * (alpha * cov);
#if defined(VIBRANT)
    // the content's coverage alone: over a dark face it is screened in (the blend adds it times what is not white yet),
    // over a light one it darkens what lies behind, so either keeps the hue of the glass
    float va = color.a * vibrant.y;
    if (vibrant.z > 0.5 && vibrant.x > 0.5) {
        color = vec4(va);
    } else if (vibrant.x > 0.5) {
        color = vec4(vec3(VIBRANT_LIFT * va), 0.0);
    } else {
        color = vec4(0.0, 0.0, 0.0, va);
    }
#endif
#if defined(DEBUG_FLAGS)
    if (tint == 1.0)
        color = vec4(0.0, 0.2, 0.0, 0.2) + color * 0.8;
#endif
    gl_FragColor = color;
}
