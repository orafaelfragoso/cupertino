// Solid rounded rectangle with optional border for shell UI (dock tiles, highlights, switcher).
// Output is premultiplied.
varying vec2 v_coords;
uniform float alpha;
uniform vec2 area_pt;
uniform vec4 rect;       // x, y, w, h in points relative to the element origin
uniform vec4 shape;      // radius (pt), border width (pt), corner exponent, pixels per point
uniform vec4 fill;       // straight rgba
uniform vec4 border;     // straight rgba

//_SDF_

void main() {
    vec2 pos = v_coords * area_pt;
    vec2 hs = rect.zw * 0.5;
    float radius = min(shape.x, min(hs.x, hs.y));
    float d = rounded_sdf(pos - (rect.xy + hs), hs, radius, shape.z);
    float px = shape.w;
    float outer = clamp(0.5 - d * px, 0.0, 1.0);
    float inner = shape.y > 0.0 ? clamp(0.5 - (d + shape.y) * px, 0.0, 1.0) : outer;
    vec4 f = vec4(fill.rgb * fill.a, fill.a) * inner;
    vec4 b = vec4(border.rgb * border.a, border.a) * (outer - inner);
    gl_FragColor = (f + b) * alpha;
}
