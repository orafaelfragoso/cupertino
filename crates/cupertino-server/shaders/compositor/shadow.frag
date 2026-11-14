// Window shadow: Gaussian-blurred rounded box in closed form (separable error function across x,
// numeric integral across y). shadows are a blurred copy of the window shape;
// parameters. Output is premultiplied black.
varying vec2 v_coords;
uniform float alpha;
uniform vec2 area_pt;    // element size in points
uniform vec4 caster;     // window rectangle (x, y, w, h) in points, relative to the element origin
uniform vec4 params;     // corner radius (pt), sigma (pt), density, y offset (pt)
uniform vec4 flags;      // cut out the window shape (0/1), corner exponent, pixels per point, corner extent (pt)

vec4 erf4(vec4 x) {
    vec4 s = sign(x);
    vec4 a = abs(x);
    vec4 t = 1.0 + (0.278393 + (0.230389 + 0.078108 * (a * a)) * a) * a;
    t *= t;
    return s - s / (t * t);
}

float gauss(float x, float sigma) {
    return exp(-(x * x) / (2.0 * sigma * sigma)) / (2.50662827463 * sigma);
}

float box_x(float x, float y, float sigma, float corner, vec2 half_size) {
    float delta = min(half_size.y - corner - abs(y), 0.0);
    float curved = half_size.x - corner + sqrt(max(0.0, corner * corner - delta * delta));
    vec4 cdf = 0.5 + 0.5 * erf4(vec4(x - curved, x + curved, 0.0, 0.0) * (0.70710678 / sigma));
    return cdf.y - cdf.x;
}

float rounded_box_shadow(vec2 p, vec2 half_size, float sigma, float corner) {
    float low = p.y - half_size.y;
    float high = p.y + half_size.y;
    float start = clamp(-3.0 * sigma, low, high);
    float end = clamp(3.0 * sigma, low, high);
    float step = (end - start) / 8.0;
    float y = start + step * 0.5;
    float value = 0.0;
    for (int i = 0; i < 8; i++) {
        value += box_x(p.x, p.y - y, sigma, corner, half_size) * gauss(y, sigma) * step;
        y += step;
    }
    return value;
}

//_SDF_

//_DITHER_

void main() {
    vec2 pos = v_coords * area_pt;
    vec2 hs = caster.zw * 0.5;
    vec2 c = caster.xy + hs;
    float radius = min(params.x, min(hs.x, hs.y));
    float s = rounded_box_shadow(pos - (c + vec2(0.0, params.w)), hs, params.y, radius);
    float a = clamp(s * params.z, 0.0, 1.0);
    if (flags.x > 0.5) {
        float d = rounded_sdf(pos - c, hs, min(flags.w, min(hs.x, hs.y)), flags.y);
        a *= clamp(0.5 + d * flags.z, 0.0, 1.0);
    }
    gl_FragColor = vec4(0.0, 0.0, 0.0, a) * alpha;
#ifdef CUPERTINO_DITHER
    gl_FragColor.a = dither_alpha(gl_FragColor.a, gl_FragCoord.xy);
#endif
}
