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
struct Resolve {
    vec4 info;
    vec4 flags;
    vec4 origin;
    vec4 tint;
};
const float CIRCULAR_EXPONENT = 2.001;
const int SHADOW_STEPS = 8;
const int MAX_STOPS = 8;

layout(std140) uniform Resolve_block_0Fragment { Resolve _group_0_binding_0_fs; };

uniform highp sampler2D _group_0_binding_1_fs;

uniform highp sampler2D _group_0_binding_2_fs;

layout(location = 0) out vec4 _fs2p_location0;

vec4 fullscreen_position(uint index_1) {
    float x_4 = ((float(((index_1 << 1u) & 2u)) * 2.0) - 1.0);
    float y_2 = ((float((index_1 & 2u)) * 2.0) - 1.0);
    return vec4(x_4, y_2, 0.0, 1.0);
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
    float v_3 = max((qy / e), 1e-6);
    float g = pow((pow(u, n) + pow(v_3, n)), (1.0 / n));
    float grad = (sqrt((pow(u, ((2.0 * n) - 2.0)) + pow(v_3, ((2.0 * n) - 2.0)))) / pow(g, (n - 1.0)));
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
    float k_2 = (0.70710677 / sigma_1);
    float _e24 = erf_approx(((x_2 - curved) * k_2));
    float low = (0.5 + (0.5 * _e24));
    float _e31 = erf_approx(((x_2 + curved) * k_2));
    float high = (0.5 + (0.5 * _e31));
    return (high - low);
}

float rounded_box_blur(vec2 p_2, vec2 half_3, float sigma_2, float corner_1) {
    float y_1 = 0.0;
    float value = 0.0;
    int i_3 = 0;
    float low_1 = (p_2.y - half_3.y);
    float high_1 = (p_2.y + half_3.y);
    float start = clamp((-3.0 * sigma_2), low_1, high_1);
    float end = clamp((3.0 * sigma_2), low_1, high_1);
    float step_1 = ((end - start) / 8.0);
    y_1 = (start + (step_1 * 0.5));
    bool loop_init = true;
    while(true) {
        if (!loop_init) {
            int _e43 = i_3;
            i_3 = (_e43 + 1);
        }
        loop_init = false;
        int _e27 = i_3;
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
            value = (_e30 + ((_e35 * _e37) * step_1));
            float _e41 = y_1;
            y_1 = (_e41 + step_1);
        }
    }
    float _e46 = value;
    return _e46;
}

float blur_mass(float sigma_3) {
    float mass = 0.0;
    int i_4 = 0;
    float step_2 = ((6.0 * sigma_3) / 8.0);
    bool loop_init_1 = true;
    while(true) {
        if (!loop_init_1) {
            int _e24 = i_4;
            i_4 = (_e24 + 1);
        }
        loop_init_1 = false;
        int _e9 = i_4;
        if ((_e9 < SHADOW_STEPS)) {
        } else {
            break;
        }
        {
            float _e12 = mass;
            int _e15 = i_4;
            float _e21 = gauss(((-3.0 * sigma_3) + (step_2 * (float(_e15) + 0.5))), sigma_3);
            mass = (_e12 + (_e21 * step_2));
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
    float blurred_1 = (_e15 / _e17);
    float outside = max((1.0 - blurred_1), 0.0);
    float extent_1 = min(caster_1.extent, min(caster_1.half_.x, caster_1.half_.y));
    float _e32 = rounded_distance(p_4, caster_1.half_, extent_1, caster_1.exponent);
    float _e33 = coverage(_e32, px_3);
    return (clamp((outside * shadow_1.density), 0.0, 1.0) * _e33);
}

vec4 gradient_at(vec4 offsets[2], vec4 colors[8], int count, float t_in) {
    float t_1 = 0.0;
    vec4 straight = vec4(0.0);
    int i_5 = 0;
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
                        i_5 = _e50;
                        break;
                    }
                    int _e51 = j;
                    i_5 = _e51;
                }
            }
            int _e55 = i_5;
            int _e59 = i_5;
            float a_2 = offsets[(_e55 / 4)][(_e59 % 4)];
            int _e63 = i_5;
            int _e69 = i_5;
            float b = offsets[((_e63 + 1) / 4)][((_e69 + 1) % 4)];
            float span = (b - a_2);
            if ((span > 0.0)) {
                float _e80 = t_1;
                k = ((_e80 - a_2) / span);
            }
            int _e83 = i_5;
            int _e85 = i_5;
            int _e89 = i_5;
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

uint dither_matrix(vec2 frag_1) {
    uvec2 p_6 = uvec2(floor(frag_1));
    uint _e5 = dither_pair(p_6, 0u);
    uint _e9 = dither_pair(p_6, 1u);
    uint _e13 = dither_pair(p_6, 2u);
    return (((16u * _e5) + (4u * _e9)) + _e13);
}

float dither_offset(vec2 frag_2) {
    uint _e1 = dither_matrix(frag_2);
    return (((float(_e1) + 0.5) / 64.0) - 0.5);
}

float dither(float v, vec2 frag_3) {
    float _e2 = dither_offset(frag_3);
    return (v + (_e2 / 255.0));
}

vec3 dither_rgb(vec3 c, vec2 frag_4) {
    float _e2 = dither_offset(frag_4);
    return (c + vec3((_e2 / 255.0)));
}

uint div255_(uint v_1) {
    uint w = (v_1 + 128u);
    return ((w + (w >> 8u)) >> 8u);
}

uint quantize(float v_2) {
    float x_5 = (clamp(v_2, 0.0, 1.0) * 255.0);
    float whole = floor(x_5);
    return (uint(whole) + (((x_5 - whole) >= 0.5) ? 1u : 0u));
}

vec4 tap(ivec2 t_2, ivec2 size) {
    if (((((t_2.x < 0) || (t_2.y < 0)) || (t_2.x >= size.x)) || (t_2.y >= size.y))) {
        return vec4(0.0);
    }
    vec4 _e21 = texelFetch(_group_0_binding_1_fs, t_2, 0);
    return _e21;
}

vec4 blurred(vec2 x_3) {
    ivec2 size_1 = ivec2(uvec2(textureSize(_group_0_binding_1_fs, 0).xy));
    float factor = _group_0_binding_0_fs.info.y;
    vec4 _e14 = _group_0_binding_0_fs.info;
    vec2 u_1 = ((((x_3 + vec2(0.5)) - _e14.zw) / vec2(factor)) - vec2(0.5));
    vec2 i_6 = floor(u_1);
    vec2 f = (u_1 - i_6);
    ivec2 t_3 = ivec2(i_6);
    vec4 _e25 = tap(t_3, size_1);
    vec4 _e34 = tap((t_3 + ivec2(1, 0)), size_1);
    vec4 top = ((_e25 * (1.0 - f.x)) + (_e34 * f.x));
    vec4 _e42 = tap((t_3 + ivec2(0, 1)), size_1);
    vec4 _e51 = tap((t_3 + ivec2(1, 1)), size_1);
    vec4 bottom = ((_e42 * (1.0 - f.x)) + (_e51 * f.x));
    return ((top * (1.0 - f.y)) + (bottom * f.y));
}

void main() {
    vec4 frag = gl_FragCoord;
    float step_ = 0.0;
    uvec4 out_ = uvec4(0u);
    int i = 0;
    uvec4 s = uvec4(0u);
    int i_1 = 0;
    uvec4 out_1 = uvec4(0u);
    int i_2 = 0;
    vec2 x_6 = floor(frag.xy);
    vec4 _e5 = _group_0_binding_0_fs.origin;
    vec2 window = ((x_6 + _e5.xy) + vec2(0.5));
    float _e16 = _group_0_binding_0_fs.flags.x;
    if ((_e16 > 0.5)) {
        float _e19 = dither_offset(window);
        step_ = (_e19 / 255.0);
    }
    vec4 _e22 = blurred(x_6);
    float _e26 = _group_0_binding_0_fs.info.x;
    int mode = int(_e26);
    if ((mode == 0)) {
        uint _e31 = quantize(_e22.w);
        float _e33 = step_;
        uint _e35 = quantize((_e22.x + _e33));
        float _e38 = step_;
        uint _e40 = quantize((_e22.y + _e38));
        float _e43 = step_;
        uint _e45 = quantize((_e22.z + _e43));
        uvec3 c_1 = uvec3(min(_e35, _e31), min(_e40, _e31), min(_e45, _e31));
        _fs2p_location0 = (vec4(uvec4(c_1, _e31)) / vec4(255.0));
        return;
    }
    vec4 _e56 = texelFetch(_group_0_binding_2_fs, ivec2(x_6), 0);
    uvec4 c_2 = uvec4(((_e56 * 255.0) + vec4(0.5)));
    vec4 _e65 = _group_0_binding_0_fs.tint;
    uvec4 tint = uvec4(_e65);
    if ((mode == 1)) {
        float _e70 = step_;
        uint _e72 = quantize((_e22.w + _e70));
        if (((_e72 == 0u) || (c_2.w == 255u))) {
            _fs2p_location0 = (vec4(c_2) / vec4(255.0));
            return;
        }
        uint keep = (255u - c_2.w);
        out_ = c_2;
        bool loop_init_3 = true;
        while(true) {
            if (!loop_init_3) {
                int _e105 = i;
                i = (_e105 + 1);
            }
            loop_init_3 = false;
            int _e89 = i;
            if ((_e89 < 4)) {
            } else {
                break;
            }
            {
                int _e92 = i;
                int _e94 = i;
                int _e96 = i;
                uint _e99 = div255_((tint[_e96] * _e72));
                uint _e101 = div255_((_e99 * keep));
                out_[_e92] = min((c_2[_e94] + _e101), 255u);
            }
        }
        uvec4 _e108 = out_;
        _fs2p_location0 = (vec4(_e108) / vec4(255.0));
        return;
    }
    float _e119 = step_;
    uint _e121 = quantize((clamp((1.0 - _e22.w), 0.0, 1.0) + _e119));
    uint _e124 = div255_((_e121 * c_2.w));
    bool loop_init_4 = true;
    while(true) {
        if (!loop_init_4) {
            int _e139 = i_1;
            i_1 = (_e139 + 1);
        }
        loop_init_4 = false;
        int _e130 = i_1;
        if ((_e130 < 4)) {
        } else {
            break;
        }
        {
            int _e133 = i_1;
            int _e135 = i_1;
            uint _e138 = div255_((tint[_e135] * _e124));
            s[_e133] = _e138;
        }
    }
    uint _e144 = s.w;
    uint keep_1 = (255u - _e144);
    out_1 = c_2;
    bool loop_init_5 = true;
    while(true) {
        if (!loop_init_5) {
            int _e164 = i_2;
            i_2 = (_e164 + 1);
        }
        loop_init_5 = false;
        int _e149 = i_2;
        if ((_e149 < 4)) {
        } else {
            break;
        }
        {
            int _e152 = i_2;
            int _e154 = i_2;
            uint _e156 = s[_e154];
            int _e157 = i_2;
            uint _e160 = div255_((c_2[_e157] * keep_1));
            out_1[_e152] = min((_e156 + _e160), 255u);
        }
    }
    uvec4 _e167 = out_1;
    _fs2p_location0 = (vec4(_e167) / vec4(255.0));
    return;
}

