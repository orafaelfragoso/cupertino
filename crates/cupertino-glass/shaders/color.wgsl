// The backdrop's colour filter chain: luminance curve, saturation, monochrome (approximate), and the display
// filters: a matrix on linear light, invert and contrast on the encoded values.

// include pass.wgsl

const LUMA709 = vec3<f32>(0.2125, 0.7154, 0.0721);   // luminance curve weights
const SAT709 = vec3<f32>(0.2126, 0.7152, 0.0722);    // saturation matrix weights

fn srgb_decode(c: vec3<f32>) -> vec3<f32> {
  return select(pow((c + 0.055) / 1.055, vec3<f32>(2.4)), c / 12.92, c < vec3<f32>(0.04045));
}

fn srgb_encode(c: vec3<f32>) -> vec3<f32> {
  return select(1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - 0.055, c * 12.92, c < vec3<f32>(0.0031308));
}

// `next` is the operation after `op` (the tail of a linear matrix).
fn color_op(c: vec3<f32>, op: Op, next: Op) -> vec3<f32> {
  let k = i32(op.a.x);
  if (k == 1) {            // saturate
    let s = op.a.y;
    let l = dot(c, SAT709);
    return vec3<f32>(l) * (1.0 - s) + c * s;
  } else if (k == 2) {     // luminance curve: b = Horner coefficients (a, b, c, d), a.y = amount
    let x0 = dot(c, LUMA709);
    let p = op.b;
    let x = clamp(((p.x * x0 + p.y) * x0 + p.z) * x0 + p.w, 0.0, 1.0);
    return mix(c, vec3<f32>(x), op.a.y);
  } else if (k == 3) {     // monochrome (approximate)
    let l = dot(c, LUMA709);
    return mix(c, l * op.b.xyz, op.a.y);
  } else if (k == 4) {     // display filter matrix on linear light (linear)
    let m = mat3x3<f32>(op.a.y, op.b.x, op.b.w, op.a.z, op.b.y, next.a.y, op.a.w, op.b.z, next.a.z);
    return srgb_encode(clamp(m * srgb_decode(clamp(c, vec3<f32>(0.0), vec3<f32>(1.0))), vec3<f32>(0.0), vec3<f32>(1.0)));
  } else if (k == 5) {     // invert
    return vec3<f32>(1.0) - c;
  } else if (k == 6) {     // contrast stretch
    let t = op.a.y;
    if (t >= 1.0) { return step(vec3<f32>(0.5), c); }
    return clamp((c - vec3<f32>(0.5 * t)) / (1.0 - t), vec3<f32>(0.0), vec3<f32>(1.0));
  }
  return c;
}

@fragment
fn fs_main(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
  var rgb = ld(vec2<i32>(frag.xy)).rgb;
  let n = P.dir_radius.w;
  for (var i = 0; i < n; i++) { rgb = color_op(rgb, P.ops[i], P.ops[min(i + 1, 7)]); }
  return vec4<f32>(rgb, 1.0);
}
