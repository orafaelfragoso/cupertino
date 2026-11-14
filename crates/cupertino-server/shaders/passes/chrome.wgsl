// One draw of the chrome's list (a title bar, its buttons, the rim) laid over what the list's draws before it made,
// with the CPU painter's arithmetic: the draw reads the image below it from a texture and writes the result into
// another, blending off, so every pixel is the painter's floating-point source-over stored once in 8 bits (a
// gradient's colour dithered by its pixel before that store), whatever the driver's blending precision. The other
// mode copies a texture's pixels as they are, to bring the drawn quads back to the image below the next draw.
//
// One instance per quad: its rectangle in target pixels and, for a coverage image, the atlas texel of its first pixel.

// Uniform block of one draw (std140, mirrored by `render::chrome::ChromeBlock`).
struct Chrome {
    // The target's size in pixels; the mode (0 a draw, 1 a copy of `below`).
    frame: vec4<f32>,
    // The shape's centre and half size, in target pixels.
    outline: vec4<f32>,
    // The shape's corner extents: top left, top right, bottom right, bottom left.
    corners: vec4<f32>,
    // The corners' exponent, the width of the band inside the outline that is covered (0: the whole shape), what
    // covers (0 the shape, 1 the coverage image), whether the paint is dithered.
    shape: vec4<f32>,
    // A solid paint's colour, premultiplied, 0 to 1.
    color: vec4<f32>,
    // Paint kind (0 solid, 1 linear) and the gradient's stop count.
    paint: vec4<f32>,
    // A linear gradient's start and end, in target pixels.
    geometry: vec4<f32>,
    offsets: array<vec4<f32>, 2>,
    colors: array<vec4<f32>, 8>,
}

@group(0) @binding(0) var<uniform> C: Chrome;
@group(0) @binding(1) var coverage_image: texture_2d<f32>;
@group(0) @binding(2) var below: texture_2d<f32>;

struct Instance {
    // x0 y0 x1 y1 in target pixels.
    @location(0) rect: vec4<f32>,
    // The atlas texel of the quad's first pixel.
    @location(1) atlas: vec4<f32>,
}

struct Varyings {
    @builtin(position) position: vec4<f32>,
    @location(0) @interpolate(flat) atlas: vec2<f32>,
    @location(1) @interpolate(flat) first: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) index: u32, instance: Instance) -> Varyings {
    let uv = vec2<f32>(f32(index & 1u), f32(index >> 1u));
    let pixel = mix(instance.rect.xy, instance.rect.zw, uv);
    var out: Varyings;
    out.position = vec4<f32>(pixel / C.frame.xy * 2.0 - 1.0, 0.0, 1.0);
    out.atlas = instance.atlas.xy;
    out.first = instance.rect.xy;
    return out;
}

// v held to 0 to 1 and stored in 8 bits, rounded to nearest, a half up: the painter's store.
fn store8(v: f32) -> f32 {
    let x = clamp(v, 0.0, 1.0) * 255.0;
    let whole = floor(x);
    return whole + select(0.0, 1.0, x - whole >= 0.5);
}

// The corner extent of the quadrant of `q` (from the shape's centre), held to the half size.
fn extent_at(q: vec2<f32>, half: vec2<f32>) -> f32 {
    var e = C.corners.x;
    if (q.x >= 0.0 && q.y < 0.0) {
        e = C.corners.y;
    } else if (q.x >= 0.0 && q.y >= 0.0) {
        e = C.corners.z;
    } else if (q.x < 0.0 && q.y >= 0.0) {
        e = C.corners.w;
    }
    return min(e, min(half.x, half.y));
}

@fragment
fn fs_main(in: Varyings) -> @location(0) vec4<f32> {
    let pixel = floor(in.position.xy);
    let at = vec2<i32>(pixel);
    let dst = textureLoad(below, at, 0);
    if (C.frame.z > 0.5) {
        return dst;
    }
    let p = pixel + vec2<f32>(0.5);
    var cover = 0.0;
    if (C.shape.z > 0.5) {
        let t = vec2<i32>(in.atlas + pixel - in.first);
        cover = floor(textureLoad(coverage_image, t, 0).r * 255.0 + 0.5) / 255.0;
    } else {
        let q = p - C.outline.xy;
        let half = C.outline.zw;
        let d = rounded_distance(q, half, extent_at(q, half), C.shape.x);
        if (C.shape.y > 0.0) {
            cover = stroke_coverage(d, C.shape.y, 1.0);
        } else {
            cover = coverage(d, 1.0);
        }
    }
    if (cover <= 0.0) {
        return dst;
    }
    var c = C.color;
    if (C.paint.x > 0.5) {
        let g = C.geometry;
        let dir = g.zw - g.xy;
        let len2 = dir.x * dir.x + dir.y * dir.y;
        var t = 0.0;
        if (len2 > 0.0) {
            t = ((p.x - g.x) * dir.x + (p.y - g.y) * dir.y) / len2;
        }
        c = gradient_at(C.offsets, C.colors, i32(C.paint.y), t);
    }
    // the texels below are whole 8-bit steps, read back exactly
    let under = floor(dst * 255.0 + vec4<f32>(0.5)) / 255.0;
    let keep = 1.0 - c.a * cover;
    let alpha = store8(c.a * cover + under.a * keep);
    var v = c.rgb * cover + under.rgb * keep;
    if (C.shape.w > 0.5) {
        v = v + vec3<f32>(dither_offset(p) / 255.0);
    }
    let rgb = min(vec3<f32>(store8(v.r), store8(v.g), store8(v.b)), vec3<f32>(alpha));
    return vec4<f32>(rgb, alpha) / 255.0;
}
