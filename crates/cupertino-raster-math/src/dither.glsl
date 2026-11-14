// Position-keyed dithering: the 8 x 8 ordered matrix entry of the pixel, as an offset inside half an 8-bit step.
// The definition and its test vectors are in cupertino_raster_math::dither; GLSL ES 1.00 and 3.00 alike.
float dither_bit(float v, float k) {
    return mod(floor(v / k), 2.0);
}

float dither_pair(vec2 p, float k) {
    float a = dither_bit(p.x, k);
    float b = dither_bit(p.y, k);
    return 2.0 * abs(a - b) + b;
}

// `frag` is gl_FragCoord.xy, the centre of the pixel in the framebuffer.
float dither_offset(vec2 frag) {
    vec2 p = floor(frag);
    float m = 16.0 * dither_pair(p, 1.0) + 4.0 * dither_pair(p, 2.0) + dither_pair(p, 4.0);
    return (m + 0.5) / 64.0 - 0.5;
}

vec3 dither_rgb(vec3 c, vec2 frag) {
    return c + vec3(dither_offset(frag) / 255.0);
}

float dither_alpha(float a, vec2 frag) {
    return a + dither_offset(frag) / 255.0;
}
