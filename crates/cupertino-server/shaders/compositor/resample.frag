//_DEFINES_
// The output's frame drawn at its whole render scale, shrunk to the display's pixels: each pixel the average of the
// frame over the area it covers there (cupertino_raster_math::resample), then dithered by the pixel's position at the
// display's step. The frame is opaque: its alpha is not read.
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
uniform vec2 texels;      // the frame's size in texels
uniform vec2 footprint;   // texels a pixel covers a side
uniform float step;       // the display's quantization step (1/255, 1/1023), 0 for no dither

//_TAPS_
//_DITHER_

void main() {
#if defined(EXTERNAL)
    vec4 color = texture2D(tex, v_coords);
#else
    vec4 color = area_shrink(tex, v_coords * texels, footprint, texels);
#endif
    vec3 rgb = color.rgb + vec3(dither_offset(gl_FragCoord.xy) * step);
    gl_FragColor = vec4(rgb, 1.0) * alpha;
#if defined(DEBUG_FLAGS)
    if (tint == 1.0)
        gl_FragColor = vec4(0.0, 0.2, 0.0, 0.2) + gl_FragColor * 0.8;
#endif
}
