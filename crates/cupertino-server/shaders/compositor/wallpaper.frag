// Procedural desktop background: diagonal gradient with soft colour blobs (gives the material
// passes and shadows something to work on). Opaque.
varying vec2 v_coords;
uniform float alpha;
uniform vec2 area_pt;
uniform vec4 c0;
uniform vec4 c1;
uniform vec4 c2;
uniform vec4 c3;

float blob(vec2 uv, vec2 centre, float k) {
    vec2 d = (uv - centre) * vec2(area_pt.x / area_pt.y, 1.0);
    return exp(-dot(d, d) * k);
}

void main() {
    vec2 uv = v_coords;
    vec3 base = mix(c0.rgb, c1.rgb, clamp(uv.x * 0.45 + uv.y * 0.55, 0.0, 1.0));
    base = mix(base, c2.rgb, blob(uv, vec2(0.22, 0.30), 9.0) * 0.85);
    base = mix(base, c3.rgb, blob(uv, vec2(0.80, 0.68), 7.0) * 0.85);
    base = mix(base, c3.rgb, blob(uv, vec2(0.58, 0.04), 40.0) * 0.9);
    base = mix(base, c2.rgb, blob(uv, vec2(0.12, 0.02), 60.0) * 0.9);
    gl_FragColor = vec4(base, 1.0) * alpha;
}
