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
#if defined(VIBRANT)
// x: 1 over a dark face, 0 over a light one; y: the coverage's weight; z: 1 for the plain label colour
uniform vec4 vibrant;
// how far towards white a full coverage lifts what lies behind a dark face; the blend screens it in
#define VIBRANT_LIFT 0.95
#endif

//_SDF_

void main() {
    vec4 color = texture2D(tex, v_coords);
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
