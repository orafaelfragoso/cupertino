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
// A texture drawn smaller than its texels: each pixel is the average of the texels it covers, a box of `footprint`
// (texture coordinates) sampled by `taps` filtered lookups an axis, at most 8.
uniform vec2 footprint;
uniform vec2 taps;

vec4 covered() {
    vec2 pitch = footprint / taps;
    vec2 start = v_coords - footprint * 0.5 + pitch * 0.5;
    vec4 sum = vec4(0.0);
    for (int y = 0; y < 8; y++) {
        if (float(y) >= taps.y) break;
        for (int x = 0; x < 8; x++) {
            if (float(x) >= taps.x) break;
            sum += texture2D(tex, start + pitch * vec2(float(x), float(y)));
        }
    }
    return sum / (taps.x * taps.y);
}
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
#if defined(DEBUG_FLAGS)
    if (tint == 1.0)
        color = vec4(0.0, 0.2, 0.0, 0.2) + color * 0.8;
#endif
    gl_FragColor = color;
}
