#version 300 es

precision highp float;
precision highp int;

struct Caster {
    vec2 half_;
    float radius;
    float extent;
    float exponent;
};
struct Shadow {
    float sigma;
    vec2 offset;
    float density;
};
struct Clip {
    vec4 linear;
    vec4 moved;
    vec4 corner;
};
struct Draw {
    vec4 frame;
    vec4 mode;
    vec4 tint;
    vec4 source;
    vec4 crop;
    vec4 image_linear;
    vec4 image_moved;
    vec4 cover_rect;
    vec4 cover_at;
    vec4 clip_info;
    vec4 mask_rect;
    vec4 mask_at;
    Clip clips[4];
};
struct Instance {
    vec4 rect;
    vec4 atlas;
};
struct Varyings {
    vec4 position;
    vec2 atlas;
    vec2 first;
};
const float CIRCULAR_EXPONENT = 2.001;
const int SHADOW_STEPS = 8;
const int MAX_STOPS = 8;

layout(std140) uniform Draw_block_0Fragment { Draw _group_0_binding_0_fs; };

uniform highp sampler2D _group_0_binding_1_fs;

uniform highp sampler2D _group_0_binding_2_fs;

uniform highp sampler2D _group_0_binding_3_fs;

flat in vec2 _vs2fs_location0;
flat in vec2 _vs2fs_location1;
layout(location = 0) out vec4 _fs2p_location0;

vec4 fullscreen_position(uint index_1) {
    float x_3 = ((float(((index_1 << 1u) & 2u)) * 2.0) - 1.0);
    float y_2 = ((float((index_1 & 2u)) * 2.0) - 1.0);
    return vec4(x_3, y_2, 0.0, 1.0);
}

float rounded_distance(vec2 p, vec2 half_, float e, float n) {
    float qx = (abs(p.x) - (half_.x - e));
    float qy = (abs(p.y) - (half_.y - e));
    if (((qx <= 0.0) && (qy <= 0.0))) {
        return (max(qx, qy) - e);
    }
    if ((qx <= 0.0)) {
        return (qy - e);
    }
    if ((qy <= 0.0)) {
        return (qx - e);
    }
    if ((e <= 0.0)) {
        return sqrt(((qx * qx) + (qy * qy)));
    }
    if ((n <= CIRCULAR_EXPONENT)) {
        return (sqrt(((qx * qx) + (qy * qy))) - e);
    }
    float u = max((qx / e), 1e-6);
    float v_5 = max((qy / e), 1e-6);
    float g = pow((pow(u, n) + pow(v_5, n)), (1.0 / n));
    float grad = (sqrt((pow(u, ((2.0 * n) - 2.0)) + pow(v_5, ((2.0 * n) - 2.0)))) / pow(g, (n - 1.0)));
    float d_2 = (((g - 1.0) / grad) * e);
    return max(d_2, (max(qx, qy) - e));
}

float rect_distance(vec2 p_1, vec2 half_1) {
    float _e4 = rounded_distance(p_1, half_1, 0.0, 2.0);
    return _e4;
}

float coverage(float d, float px) {
    return clamp((0.5 - (d * px)), 0.0, 1.0);
}

float stroke_coverage(float d_1, float width, float px_1) {
    if ((width <= 0.0)) {
        return 0.0;
    }
    float _e6 = coverage(d_1, px_1);
    float _e8 = coverage((d_1 + width), px_1);
    return (_e6 - _e8);
}

float erf_approx(float x) {
    float t = 0.0;
    float a_1 = abs(x);
    t = (1.0 + ((0.278393 + ((0.230389 + (0.078108 * (a_1 * a_1))) * a_1)) * a_1));
    float _e14 = t;
    float _e15 = t;
    t = (_e14 * _e15);
    float _e18 = t;
    float _e19 = t;
    return (sign(x) * (1.0 - (1.0 / (_e18 * _e19))));
}

float gauss(float x_1, float sigma) {
    return (exp((-((x_1 * x_1)) / ((2.0 * sigma) * sigma))) / (2.5066283 * sigma));
}

float box_x(float x_2, float y, float sigma_1, float corner, vec2 half_2) {
    float delta = min(((half_2.y - corner) - abs(y)), 0.0);
    float curved = ((half_2.x - corner) + sqrt(max(((corner * corner) - (delta * delta)), 0.0)));
    float k_3 = (0.70710677 / sigma_1);
    float _e24 = erf_approx(((x_2 - curved) * k_3));
    float low = (0.5 + (0.5 * _e24));
    float _e31 = erf_approx(((x_2 + curved) * k_3));
    float high = (0.5 + (0.5 * _e31));
    return (high - low);
}

float rounded_box_blur(vec2 p_2, vec2 half_3, float sigma_2, float corner_1) {
    float y_1 = 0.0;
    float value = 0.0;
    int i = 0;
    float low_1 = (p_2.y - half_3.y);
    float high_1 = (p_2.y + half_3.y);
    float start = clamp((-3.0 * sigma_2), low_1, high_1);
    float end = clamp((3.0 * sigma_2), low_1, high_1);
    float step_ = ((end - start) / 8.0);
    y_1 = (start + (step_ * 0.5));
    bool loop_init = true;
    while(true) {
        if (!loop_init) {
            int _e43 = i;
            i = (_e43 + 1);
        }
        loop_init = false;
        int _e27 = i;
        if ((_e27 < SHADOW_STEPS)) {
        } else {
            break;
        }
        {
            float _e30 = value;
            float _e33 = y_1;
            float _e35 = box_x(p_2.x, (p_2.y - _e33), sigma_2, corner_1, half_3);
            float _e36 = y_1;
            float _e37 = gauss(_e36, sigma_2);
            value = (_e30 + ((_e35 * _e37) * step_));
            float _e41 = y_1;
            y_1 = (_e41 + step_);
        }
    }
    float _e46 = value;
    return _e46;
}

float blur_mass(float sigma_3) {
    float mass = 0.0;
    int i_1 = 0;
    float step_1 = ((6.0 * sigma_3) / 8.0);
    bool loop_init_1 = true;
    while(true) {
        if (!loop_init_1) {
            int _e24 = i_1;
            i_1 = (_e24 + 1);
        }
        loop_init_1 = false;
        int _e9 = i_1;
        if ((_e9 < SHADOW_STEPS)) {
        } else {
            break;
        }
        {
            float _e12 = mass;
            int _e15 = i_1;
            float _e21 = gauss(((-3.0 * sigma_3) + (step_1 * (float(_e15) + 0.5))), sigma_3);
            mass = (_e12 + (_e21 * step_1));
        }
    }
    float _e27 = mass;
    return _e27;
}

float drop_shadow(vec2 p_3, Caster caster, Shadow shadow, bool cut_out, float px_2) {
    float a = 0.0;
    float radius = min(caster.radius, min(caster.half_.x, caster.half_.y));
    vec2 q = (p_3 - shadow.offset);
    float _e16 = rounded_box_blur(q, caster.half_, shadow.sigma, radius);
    a = clamp((_e16 * shadow.density), 0.0, 1.0);
    if (cut_out) {
        float extent = min(caster.extent, min(caster.half_.x, caster.half_.y));
        float _e32 = rounded_distance(p_3, caster.half_, extent, caster.exponent);
        float _e33 = a;
        a = (_e33 * clamp((0.5 + (_e32 * px_2)), 0.0, 1.0));
    }
    float _e41 = a;
    return _e41;
}

float inner_shadow(vec2 p_4, Caster caster_1, Shadow shadow_1, float px_3) {
    float radius_1 = min(caster_1.radius, min(caster_1.half_.x, caster_1.half_.y));
    vec2 q_1 = (p_4 - shadow_1.offset);
    float _e15 = rounded_box_blur(q_1, caster_1.half_, shadow_1.sigma, radius_1);
    float _e17 = blur_mass(shadow_1.sigma);
    float blurred = (_e15 / _e17);
    float outside = max((1.0 - blurred), 0.0);
    float extent_1 = min(caster_1.extent, min(caster_1.half_.x, caster_1.half_.y));
    float _e32 = rounded_distance(p_4, caster_1.half_, extent_1, caster_1.exponent);
    float _e33 = coverage(_e32, px_3);
    return (clamp((outside * shadow_1.density), 0.0, 1.0) * _e33);
}

vec4 gradient_at(vec4 offsets[2], vec4 colors[8], int count, float t_in) {
    float t_1 = 0.0;
    vec4 straight = vec4(0.0);
    int i_2 = 0;
    int j = 0;
    float k = 1.0;
    t_1 = t_in;
    float _e5 = t_1;
    float _e6 = t_1;
    if ((_e5 != _e6)) {
        t_1 = 0.0;
    }
    int last = (count - 1);
    float first_offset = offsets[0].x;
    float last_offset = offsets[(last / 4)][(last % 4)];
    straight = colors[0];
    float _e21 = t_1;
    if ((_e21 <= first_offset)) {
        straight = colors[0];
    } else {
        float _e24 = t_1;
        if ((_e24 >= last_offset)) {
            straight = colors[last];
        } else {
            bool loop_init_2 = true;
            while(true) {
                if (!loop_init_2) {
                    int _e52 = j;
                    j = (_e52 + 1);
                }
                loop_init_2 = false;
                int _e31 = j;
                if ((_e31 < 7)) {
                } else {
                    break;
                }
                {
                    int _e34 = j;
                    if ((_e34 >= last)) {
                        break;
                    }
                    float _e36 = t_1;
                    int _e37 = j;
                    int _e43 = j;
                    if ((_e36 < offsets[((_e37 + 1) / 4)][((_e43 + 1) % 4)])) {
                        int _e50 = j;
                        i_2 = _e50;
                        break;
                    }
                    int _e51 = j;
                    i_2 = _e51;
                }
            }
            int _e55 = i_2;
            int _e59 = i_2;
            float a_2 = offsets[(_e55 / 4)][(_e59 % 4)];
            int _e63 = i_2;
            int _e69 = i_2;
            float b = offsets[((_e63 + 1) / 4)][((_e69 + 1) % 4)];
            float span = (b - a_2);
            if ((span > 0.0)) {
                float _e80 = t_1;
                k = ((_e80 - a_2) / span);
            }
            int _e83 = i_2;
            int _e85 = i_2;
            int _e89 = i_2;
            float _e92 = k;
            straight = (colors[_e83] + ((colors[(_e85 + 1)] - colors[_e89]) * _e92));
        }
    }
    vec4 _e95 = straight;
    float _e98 = straight.w;
    float _e101 = straight.w;
    return vec4((_e95.xyz * _e98), _e101);
}

uint dither_pair(uvec2 p_5, uint k_1) {
    uint a_3 = ((p_5.x >> k_1) & 1u);
    uint b_1 = ((p_5.y >> k_1) & 1u);
    return ((2u * (a_3 ^ b_1)) + b_1);
}

uint dither_matrix(vec2 frag) {
    uvec2 p_10 = uvec2(floor(frag));
    uint _e5 = dither_pair(p_10, 0u);
    uint _e9 = dither_pair(p_10, 1u);
    uint _e13 = dither_pair(p_10, 2u);
    return (((16u * _e5) + (4u * _e9)) + _e13);
}

float dither_offset(vec2 frag_1) {
    uint _e1 = dither_matrix(frag_1);
    return (((float(_e1) + 0.5) / 64.0) - 0.5);
}

float dither(float v, vec2 frag_2) {
    float _e2 = dither_offset(frag_2);
    return (v + (_e2 / 255.0));
}

vec3 dither_rgb(vec3 c, vec2 frag_3) {
    float _e2 = dither_offset(frag_3);
    return (c + vec3((_e2 / 255.0)));
}

uint div255_(uint v_1) {
    uint w = (v_1 + 128u);
    return ((w + (w >> 8u)) >> 8u);
}

uint coverage_byte(float v_2) {
    return uint(((clamp(v_2, 0.0, 1.0) * 255.0) + 0.5));
}

uint byte_of(float v_3) {
    return uint(((v_3 * 255.0) + 0.5));
}

uvec4 bytes_of(vec4 v_4) {
    return uvec4(((v_4 * 255.0) + vec4(0.5)));
}

uvec4 mul_bytes(uvec4 s, uint k_2) {
    uint _e4 = div255_((s.x * k_2));
    uint _e7 = div255_((s.y * k_2));
    uint _e10 = div255_((s.z * k_2));
    uint _e13 = div255_((s.w * k_2));
    return uvec4(_e4, _e7, _e10, _e13);
}

vec2 place(vec4 linear, vec2 moved, vec2 p_6) {
    return vec2((((linear.x * p_6.x) + (linear.z * p_6.y)) + moved.x), (((linear.y * p_6.x) + (linear.w * p_6.y)) + moved.y));
}

float outline_distance(vec2 p_7, vec2 half_4, float corner_2) {
    if ((corner_2 > 0.0)) {
        float _e6 = rounded_distance(p_7, half_4, corner_2, 2.0);
        return _e6;
    }
    float _e7 = rect_distance(p_7, half_4);
    return _e7;
}

uint clip_coverage(vec2 p_8, ivec2 wp) {
    uint m_1 = 255u;
    int i_3 = 0;
    uint c_1 = 0u;
    float _e7 = _group_0_binding_0_fs.clip_info.x;
    int count_1 = int(_e7);
    bool loop_init_3 = true;
    while(true) {
        if (!loop_init_3) {
            int _e42 = i_3;
            i_3 = (_e42 + 1);
        }
        loop_init_3 = false;
        int _e11 = i_3;
        if ((_e11 < 4)) {
        } else {
            break;
        }
        {
            int _e14 = i_3;
            if ((_e14 >= count_1)) {
                break;
            }
            int _e18 = i_3;
            Clip clip = _group_0_binding_0_fs.clips[_e18];
            vec2 _e24 = place(clip.linear, clip.moved.xy, p_8);
            float _e29 = outline_distance(_e24, clip.moved.zw, clip.corner.x);
            float _e32 = coverage(_e29, clip.corner.y);
            uint _e33 = coverage_byte(_e32);
            if ((_e33 == 0u)) {
                m_1 = 0u;
            } else {
                if ((_e33 != 255u)) {
                    uint _e39 = m_1;
                    uint _e41 = div255_((_e39 * _e33));
                    m_1 = _e41;
                }
            }
        }
    }
    float _e48 = _group_0_binding_0_fs.clip_info.y;
    if ((_e48 > 0.5)) {
        vec4 _e53 = _group_0_binding_0_fs.mask_rect;
        ivec4 r = ivec4(_e53);
        if (((((wp.x >= r.x) && (wp.y >= r.y)) && (wp.x < r.z)) && (wp.y < r.w))) {
            vec4 _e75 = _group_0_binding_0_fs.mask_at;
            vec4 _e82 = texelFetch(_group_0_binding_2_fs, ((ivec2(_e75.xy) + wp) - r.xy), 0);
            uint _e84 = byte_of(_e82.x);
            c_1 = _e84;
        }
        uint _e85 = m_1;
        uint _e86 = c_1;
        uint _e88 = div255_((_e85 * _e86));
        m_1 = _e88;
    }
    uint _e89 = m_1;
    return _e89;
}

vec4 texel(ivec2 t_2) {
    vec4 _e3 = _group_0_binding_0_fs.crop;
    ivec2 lo = ivec2(_e3.xy);
    vec4 _e8 = _group_0_binding_0_fs.crop;
    ivec2 hi = (ivec2(_e8.zw) - ivec2(1));
    vec4 _e17 = texelFetch(_group_0_binding_1_fs, min(max(t_2, lo), hi), 0);
    return _e17;
}

vec4 image_color(vec2 p_9) {
    vec4 _e3 = _group_0_binding_0_fs.image_linear;
    vec4 _e6 = _group_0_binding_0_fs.image_moved;
    vec2 _e8 = place(_e3, _e6.xy, p_9);
    float _e12 = _group_0_binding_0_fs.mode.z;
    if ((_e12 < 0.5)) {
        vec4 _e17 = texel(ivec2(floor(_e8)));
        return _e17;
    }
    vec2 u_1 = (_e8 - vec2(0.5));
    vec2 i_4 = floor(u_1);
    vec2 f = (u_1 - i_4);
    ivec2 t_3 = ivec2(i_4);
    vec4 _e24 = texel(t_3);
    vec4 _e33 = texel((t_3 + ivec2(1, 0)));
    vec4 top = ((_e24 * (1.0 - f.x)) + (_e33 * f.x));
    vec4 _e41 = texel((t_3 + ivec2(0, 1)));
    vec4 _e50 = texel((t_3 + ivec2(1, 1)));
    vec4 bottom = ((_e41 * (1.0 - f.x)) + (_e50 * f.x));
    return ((top * (1.0 - f.y)) + (bottom * f.y));
}

void main() {
    Varyings in_ = Varyings(gl_FragCoord, _vs2fs_location0, _vs2fs_location1);
    uint m = 255u;
    float cover = 1.0;
    vec4 frag_4 = in_.position;
    vec4 _e6 = _group_0_binding_0_fs.frame;
    vec2 window = (floor(frag_4.xy) + _e6.zw);
    ivec2 wp_1 = ivec2(window);
    vec2 p_11 = (window + vec2(0.5));
    float _e18 = _group_0_binding_0_fs.clip_info.x;
    float _e24 = _group_0_binding_0_fs.clip_info.y;
    if (((_e18 > 0.5) || (_e24 > 0.5))) {
        uint _e28 = clip_coverage(p_11, wp_1);
        m = _e28;
    }
    uint _e29 = m;
    if ((_e29 == 0u)) {
        discard;
    }
    float _e35 = _group_0_binding_0_fs.mode.x;
    int mode = int(_e35);
    if ((mode == 0)) {
        vec4 _e48 = texelFetch(_group_0_binding_1_fs, ivec2(((in_.atlas + floor(frag_4.xy)) - in_.first)), 0);
        uvec4 _e49 = bytes_of(_e48);
        float _e53 = _group_0_binding_0_fs.tint.w;
        uint _e55 = m;
        uint _e57 = div255_((uint(_e53) * _e55));
        uvec4 _e58 = mul_bytes(_e49, _e57);
        _fs2p_location0 = (vec4(_e58) / vec4(255.0));
        return;
    }
    if ((mode == 1)) {
        vec4 _e68 = _group_0_binding_0_fs.source;
        vec4 _e73 = texelFetch(_group_0_binding_1_fs, (wp_1 - ivec2(_e68.xy)), 0);
        uvec4 _e74 = bytes_of(_e73);
        uint _e75 = m;
        uvec4 _e76 = mul_bytes(_e74, _e75);
        float _e80 = _group_0_binding_0_fs.mode.y;
        uvec4 _e82 = mul_bytes(_e76, uint(_e80));
        _fs2p_location0 = (vec4(_e82) / vec4(255.0));
        return;
    }
    float _e92 = _group_0_binding_0_fs.mode.w;
    if ((_e92 > 0.5)) {
        vec4 _e97 = _group_0_binding_0_fs.cover_rect;
        ivec4 r_1 = ivec4(_e97);
        cover = 0.0;
        if (((((wp_1.x >= r_1.x) && (wp_1.y >= r_1.y)) && (wp_1.x < r_1.z)) && (wp_1.y < r_1.w))) {
            vec4 _e118 = _group_0_binding_0_fs.cover_at;
            vec4 _e125 = texelFetch(_group_0_binding_3_fs, ((ivec2(_e118.xy) + wp_1) - r_1.xy), 0);
            cover = _e125.x;
        }
    }
    vec4 _e127 = image_color(p_11);
    float _e128 = cover;
    uint _e130 = m;
    float _e138 = _group_0_binding_0_fs.mode.y;
    _fs2p_location0 = (((_e127 * _e128) * (float(_e130) / 255.0)) * (_e138 / 255.0));
    return;
}

