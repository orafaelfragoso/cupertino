// The toolkit's coverage draws: a rectangle or rounded rectangle (filled or stroked) by the shared distance, or an
// 8-bit coverage image (a glyph from the coverage atlas, a path rasterized on the CPU into the mask atlas), times
// the clips in force, laid in a solid colour or a gradient. The arithmetic is the CPU renderer's, in whole 8-bit
// steps: coverage and clips stored as bytes, multiplied with rounding, the paint stored as bytes (a gradient
// dithered by the window pixel), so both renderers give the same bytes up to the blend's own rounding.
//
// One instance per quad: its rectangle in target pixels and, for a coverage image, the atlas texel of its first
// pixel. Positions are window pixels: a target's pixel plus the target's origin in the window.

// An analytic clip: window pixel to the clip shape's centred units (a b c d, e f), its half size, corner and pixels
// per unit.
struct Clip {
    linear: vec4<f32>,
    moved: vec4<f32>,
    corner: vec4<f32>,
}

struct Draw {
    // The target's size in pixels, and the window pixel of its first pixel.
    frame: vec4<f32>,
    // Window pixel to the shape's centred units: a b c d, then e f and the half size.
    shape_linear: vec4<f32>,
    shape_moved: vec4<f32>,
    // Corner radius, pixels per unit, stroke width (0 for a fill), what covers (0 the shape, 1 the coverage image).
    shape: vec4<f32>,
    // The solid colour, premultiplied, in bytes.
    color: vec4<f32>,
    // Paint kind (0 solid, 1 linear, 2 radial), stop count, whether it is dithered.
    paint: vec4<f32>,
    // Window pixel to node coordinates: a b c d, then e f.
    paint_linear: vec4<f32>,
    paint_moved: vec4<f32>,
    // Linear: start and end; radial: centre and radius.
    paint_geometry: vec4<f32>,
    offsets: array<vec4<f32>, 2>,
    colors: array<vec4<f32>, 8>,
    // Analytic clips in force, whether the path clip mask is.
    clip_info: vec4<f32>,
    // The path clip mask's window pixels (x0 y0 x1 y1) and the atlas texel of its first pixel.
    mask_rect: vec4<f32>,
    mask_at: vec4<f32>,
    clips: array<Clip, 4>,
}

@group(0) @binding(0) var<uniform> draw: Draw;
@group(0) @binding(1) var coverage_image: texture_2d<f32>;
@group(0) @binding(2) var clip_mask: texture_2d<f32>;

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
    out.position = vec4<f32>(pixel / draw.frame.xy * 2.0 - 1.0, 0.0, 1.0);
    out.atlas = instance.atlas.xy;
    out.first = instance.rect.xy;
    return out;
}

// v / 255 rounded to nearest, for v up to 255 * 255.
fn div255(v: u32) -> u32 {
    let w = v + 128u;
    return (w + (w >> 8u)) >> 8u;
}

// A coverage from 0 to 1 as the byte the CPU stores: v * 255 + 0.5, truncated.
fn coverage_byte(v: f32) -> u32 {
    return u32(clamp(v, 0.0, 1.0) * 255.0 + 0.5);
}

// v held to 0 to 1 and stored in 8 bits, rounded to nearest (the shared definition's store).
fn quantize(v: f32) -> u32 {
    let x = clamp(v, 0.0, 1.0) * 255.0;
    let whole = floor(x);
    return u32(whole) + select(0u, 1u, x - whole >= 0.5);
}

// A texel of an 8-bit image as its byte.
fn byte_of(v: f32) -> u32 {
    return u32(v * 255.0 + 0.5);
}

// Window pixel centre to a shape's centred units.
fn place(linear: vec4<f32>, moved: vec2<f32>, p: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(linear.x * p.x + linear.z * p.y + moved.x, linear.y * p.x + linear.w * p.y + moved.y);
}

// Distance to a rectangle (corner 0) or rounded rectangle (circular corner), its outline moved out by `grow`.
fn outline_distance(p: vec2<f32>, half: vec2<f32>, corner: f32, grow: f32) -> f32 {
    if (corner > 0.0) {
        return rounded_distance(p, half, corner, 2.0) - grow;
    }
    return rect_distance(p, half + vec2<f32>(grow));
}

// The clips' coverage at window pixel centre `p`, 0 to 255: each clip's byte multiplied in turn.
fn clip_coverage(p: vec2<f32>, wp: vec2<i32>) -> u32 {
    var m = 255u;
    let count = i32(draw.clip_info.x);
    for (var i = 0; i < 4; i = i + 1) {
        if (i >= count) {
            break;
        }
        let clip = draw.clips[i];
        let q = place(clip.linear, clip.moved.xy, p);
        let c = coverage_byte(coverage(outline_distance(q, clip.moved.zw, clip.corner.x, 0.0), clip.corner.y));
        if (c == 0u) {
            m = 0u;
        } else if (c != 255u) {
            m = div255(m * c);
        }
    }
    if (draw.clip_info.y > 0.5) {
        let r = vec4<i32>(draw.mask_rect);
        var c = 0u;
        if (wp.x >= r.x && wp.y >= r.y && wp.x < r.z && wp.y < r.w) {
            let t = vec2<i32>(draw.mask_at.xy) + wp - r.xy;
            c = byte_of(textureLoad(clip_mask, t, 0).r);
        }
        m = div255(m * c);
    }
    return m;
}

// The paint's premultiplied bytes at window pixel centre `p` of pixel `wp`.
fn paint_bytes(p: vec2<f32>, frag: vec2<f32>) -> vec4<u32> {
    let kind = i32(draw.paint.x);
    if (kind == 0) {
        return vec4<u32>(draw.color);
    }
    let q = place(draw.paint_linear, draw.paint_moved.xy, p);
    var t = 0.0;
    let g = draw.paint_geometry;
    if (kind == 1) {
        let d = g.zw - g.xy;
        let len2 = d.x * d.x + d.y * d.y;
        if (len2 > 0.0) {
            t = ((q.x - g.x) * d.x + (q.y - g.y) * d.y) / len2;
        }
    } else {
        if (g.z <= 0.0) {
            t = 1.0;
        } else {
            let d = q - g.xy;
            t = sqrt(d.x * d.x + d.y * d.y) / g.z;
        }
    }
    let c = gradient_at(draw.offsets, draw.colors, i32(draw.paint.y), t);
    let alpha = quantize(c.a);
    var step = 0.0;
    if (draw.paint.z > 0.5) {
        step = dither_offset(frag) / 255.0;
    }
    let r = min(quantize(c.r + step), alpha);
    let gr = min(quantize(c.g + step), alpha);
    let b = min(quantize(c.b + step), alpha);
    return vec4<u32>(r, gr, b, alpha);
}

@fragment
fn fs_main(in: Varyings) -> @location(0) vec4<f32> {
    let frag = in.position;
    let window = floor(frag.xy) + draw.frame.zw;
    let wp = vec2<i32>(window);
    let p = window + vec2<f32>(0.5);
    var k = 0u;
    if (draw.shape.w > 0.5) {
        let t = vec2<i32>(in.atlas + floor(frag.xy) - in.first);
        k = byte_of(textureLoad(coverage_image, t, 0).r);
    } else {
        let q = place(draw.shape_linear, draw.shape_moved.xy, p);
        let half = draw.shape_moved.zw;
        let width = draw.shape.z;
        if (width > 0.0) {
            let outer = outline_distance(q, half, draw.shape.x, width * 0.5);
            k = coverage_byte(stroke_coverage(outer, width, draw.shape.y));
        } else {
            k = coverage_byte(coverage(outline_distance(q, half, draw.shape.x, 0.0), draw.shape.y));
        }
    }
    if (draw.clip_info.x > 0.5 || draw.clip_info.y > 0.5) {
        k = div255(clip_coverage(p, wp) * k);
    }
    if (k == 0u) {
        discard;
    }
    let s = paint_bytes(p, window + vec2<f32>(0.5));
    let out = vec4<u32>(div255(s.x * k), div255(s.y * k), div255(s.z * k), div255(s.w * k));
    return vec4<f32>(out) / 255.0;
}
