// The toolkit's colour draws: a colour glyph from the colour atlas tinted by its run's alpha, a layer laid over at
// an opacity, or an image sampled nearest or linearly (its edges' coverage, when they do not lie on whole pixels,
// from a mask rasterized on the CPU), each times the clips in force. Glyphs and layers keep the CPU renderer's
// 8-bit arithmetic; an image is sampled in floats, as the CPU renderer's image pipeline samples it.
//
// One instance per quad: its rectangle in target pixels and, for a glyph, the atlas texel of its first pixel.

struct Clip {
    linear: vec4<f32>,
    moved: vec4<f32>,
    corner: vec4<f32>,
}

struct Draw {
    // The target's size in pixels, and the window pixel of its first pixel.
    frame: vec4<f32>,
    // What is drawn (0 a colour glyph, 1 a layer, 2 an image), the opacity in bytes, linear sampling, whether the
    // image's edge coverage mask is used.
    mode: vec4<f32>,
    // A glyph run's colour, premultiplied, in bytes.
    tint: vec4<f32>,
    // A layer: the window pixel of its first pixel.
    source: vec4<f32>,
    // An image: the texels sampled (x0 y0 x1 y1), and window pixel to texel coordinates (a b c d, then e f).
    crop: vec4<f32>,
    image_linear: vec4<f32>,
    image_moved: vec4<f32>,
    // The edge coverage mask's window pixels and the atlas texel of its first pixel.
    cover_rect: vec4<f32>,
    cover_at: vec4<f32>,
    clip_info: vec4<f32>,
    mask_rect: vec4<f32>,
    mask_at: vec4<f32>,
    clips: array<Clip, 4>,
}

@group(0) @binding(0) var<uniform> draw: Draw;
@group(0) @binding(1) var source: texture_2d<f32>;
@group(0) @binding(2) var clip_mask: texture_2d<f32>;
@group(0) @binding(3) var cover_mask: texture_2d<f32>;

struct Instance {
    @location(0) rect: vec4<f32>,
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

fn div255(v: u32) -> u32 {
    let w = v + 128u;
    return (w + (w >> 8u)) >> 8u;
}

fn coverage_byte(v: f32) -> u32 {
    return u32(clamp(v, 0.0, 1.0) * 255.0 + 0.5);
}

fn byte_of(v: f32) -> u32 {
    return u32(v * 255.0 + 0.5);
}

fn bytes_of(v: vec4<f32>) -> vec4<u32> {
    return vec4<u32>(v * 255.0 + vec4<f32>(0.5));
}

fn mul_bytes(s: vec4<u32>, k: u32) -> vec4<u32> {
    return vec4<u32>(div255(s.x * k), div255(s.y * k), div255(s.z * k), div255(s.w * k));
}

fn place(linear: vec4<f32>, moved: vec2<f32>, p: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(linear.x * p.x + linear.z * p.y + moved.x, linear.y * p.x + linear.w * p.y + moved.y);
}

fn outline_distance(p: vec2<f32>, half: vec2<f32>, corner: f32) -> f32 {
    if (corner > 0.0) {
        return rounded_distance(p, half, corner, 2.0);
    }
    return rect_distance(p, half);
}

fn clip_coverage(p: vec2<f32>, wp: vec2<i32>) -> u32 {
    var m = 255u;
    let count = i32(draw.clip_info.x);
    for (var i = 0; i < 4; i = i + 1) {
        if (i >= count) {
            break;
        }
        let clip = draw.clips[i];
        let q = place(clip.linear, clip.moved.xy, p);
        let c = coverage_byte(coverage(outline_distance(q, clip.moved.zw, clip.corner.x), clip.corner.y));
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
            c = byte_of(textureLoad(clip_mask, vec2<i32>(draw.mask_at.xy) + wp - r.xy, 0).r);
        }
        m = div255(m * c);
    }
    return m;
}

// A texel of the image, its index held inside the crop.
fn texel(t: vec2<i32>) -> vec4<f32> {
    let lo = vec2<i32>(draw.crop.xy);
    let hi = vec2<i32>(draw.crop.zw) - vec2<i32>(1);
    return textureLoad(source, clamp(t, lo, hi), 0);
}

fn image_color(p: vec2<f32>) -> vec4<f32> {
    let q = place(draw.image_linear, draw.image_moved.xy, p);
    if (draw.mode.z < 0.5) {
        return texel(vec2<i32>(floor(q)));
    }
    let u = q - vec2<f32>(0.5);
    let i = floor(u);
    let f = u - i;
    let t = vec2<i32>(i);
    let top = texel(t) * (1.0 - f.x) + texel(t + vec2<i32>(1, 0)) * f.x;
    let bottom = texel(t + vec2<i32>(0, 1)) * (1.0 - f.x) + texel(t + vec2<i32>(1, 1)) * f.x;
    return top * (1.0 - f.y) + bottom * f.y;
}

@fragment
fn fs_main(in: Varyings) -> @location(0) vec4<f32> {
    let frag = in.position;
    let window = floor(frag.xy) + draw.frame.zw;
    let wp = vec2<i32>(window);
    let p = window + vec2<f32>(0.5);
    var m = 255u;
    if (draw.clip_info.x > 0.5 || draw.clip_info.y > 0.5) {
        m = clip_coverage(p, wp);
    }
    if (m == 0u) {
        discard;
    }
    let mode = i32(draw.mode.x);
    if (mode == 0) {
        let s = bytes_of(textureLoad(source, vec2<i32>(in.atlas + floor(frag.xy) - in.first), 0));
        let k = div255(u32(draw.tint.w) * m);
        return vec4<f32>(mul_bytes(s, k)) / 255.0;
    }
    if (mode == 1) {
        let s = bytes_of(textureLoad(source, wp - vec2<i32>(draw.source.xy), 0));
        return vec4<f32>(mul_bytes(mul_bytes(s, m), u32(draw.mode.y))) / 255.0;
    }
    var cover = 1.0;
    if (draw.mode.w > 0.5) {
        let r = vec4<i32>(draw.cover_rect);
        cover = 0.0;
        if (wp.x >= r.x && wp.y >= r.y && wp.x < r.z && wp.y < r.w) {
            cover = textureLoad(cover_mask, vec2<i32>(draw.cover_at.xy) + wp - r.xy, 0).r;
        }
    }
    return image_color(p) * cover * (f32(m) / 255.0) * (draw.mode.y / 255.0);
}
