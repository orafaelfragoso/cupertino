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
    vec4 shape_linear;
    vec4 shape_moved;
    vec4 shape;
    vec4 color;
    vec4 paint;
    vec4 paint_linear;
    vec4 paint_moved;
    vec4 paint_geometry;
    vec4 offsets[2];
    vec4 colors[8];
    vec4 clip_info;
    vec4 mask_rect;
    vec4 mask_at;
    Clip clips[4];
    vec4 corners;
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

layout(std140) uniform Draw_block_0Vertex { Draw _group_0_binding_0_vs; };

layout(location = 0) in vec4 _p2vs_location0;
layout(location = 1) in vec4 _p2vs_location1;
flat out vec2 _vs2fs_location0;
flat out vec2 _vs2fs_location1;

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

float box_x(float x_2, float y, float sigma_1, float corner_1, vec2 half_2) {
    float delta = min(((half_2.y - corner_1) - abs(y)), 0.0);
    float curved = ((half_2.x - corner_1) + sqrt(max(((corner_1 * corner_1) - (delta * delta)), 0.0)));
    float k_3 = (0.70710677 / sigma_1);
    float _e24 = erf_approx(((x_2 - curved) * k_3));
    float low = (0.5 + (0.5 * _e24));
    float _e31 = erf_approx(((x_2 + curved) * k_3));
    float high = (0.5 + (0.5 * _e31));
    return (high - low);
}

float rounded_box_blur(vec2 p_2, vec2 half_3, float sigma_2, float corner_2) {
    float y_1 = 0.0;
    float value = 0.0;
    int i = 0;
    float low_1 = (p_2.y - half_3.y);
    float high_1 = (p_2.y + half_3.y);
    float start = clamp((-3.0 * sigma_2), low_1, high_1);
    float end = clamp((3.0 * sigma_2), low_1, high_1);
    float step_1 = ((end - start) / 8.0);
    y_1 = (start + (step_1 * 0.5));
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
            float _e35 = box_x(p_2.x, (p_2.y - _e33), sigma_2, corner_2, half_3);
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
    int i_1 = 0;
    float step_2 = ((6.0 * sigma_3) / 8.0);
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
    vec2 q_1 = (p_3 - shadow.offset);
    float _e16 = rounded_box_blur(q_1, caster.half_, shadow.sigma, radius);
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
    vec2 q_2 = (p_4 - shadow_1.offset);
    float _e15 = rounded_box_blur(q_2, caster_1.half_, shadow_1.sigma, radius_1);
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
    float k_1 = 1.0;
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
                k_1 = ((_e80 - a_2) / span);
            }
            int _e83 = i_2;
            int _e85 = i_2;
            int _e89 = i_2;
            float _e92 = k_1;
            straight = (colors[_e83] + ((colors[(_e85 + 1)] - colors[_e89]) * _e92));
        }
    }
    vec4 _e95 = straight;
    float _e98 = straight.w;
    float _e101 = straight.w;
    return vec4((_e95.xyz * _e98), _e101);
}

uint dither_pair(uvec2 p_5, uint k_2) {
    uint a_3 = ((p_5.x >> k_2) & 1u);
    uint b_1 = ((p_5.y >> k_2) & 1u);
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

uint quantize(float v_3) {
    float x_4 = (clamp(v_3, 0.0, 1.0) * 255.0);
    float whole = floor(x_4);
    return (uint(whole) + (((x_4 - whole) >= 0.5) ? 1u : 0u));
}

uint byte_of(float v_4) {
    return uint(((v_4 * 255.0) + 0.5));
}

vec2 place(vec4 linear, vec2 moved, vec2 p_6) {
    return vec2((((linear.x * p_6.x) + (linear.z * p_6.y)) + moved.x), (((linear.y * p_6.x) + (linear.w * p_6.y)) + moved.y));
}

float outline_distance(vec2 p_7, vec2 half_4, float corner_3, float n_1, float grow) {
    if ((corner_3 > 0.0)) {
        float _e11 = rounded_distance(p_7, half_4, corner_3, ((n_1 > 0.0) ? n_1 : 2.0));
        return (_e11 - grow);
    }
    float _e15 = rect_distance(p_7, (half_4 + vec2(grow)));
    return _e15;
}

float corner_of(vec4 corners, vec2 q, vec2 half_5) {
    float e_1 = 0.0;
    e_1 = corners.x;
    if (((q.x >= 0.0) && (q.y < 0.0))) {
        e_1 = corners.y;
    } else {
        if (((q.x >= 0.0) && (q.y >= 0.0))) {
            e_1 = corners.z;
        } else {
            if (((q.x < 0.0) && (q.y >= 0.0))) {
                e_1 = corners.w;
            }
        }
    }
    float _e29 = e_1;
    return min(_e29, min(half_5.x, half_5.y));
}

uvec4 paint_bytes(vec2 p_9, vec2 frag_4) {
    float t_2 = 0.0;
    float step_ = 0.0;
    float _e5 = _group_0_binding_0_vs.paint.x;
    int kind = int(_e5);
    if ((kind == 0)) {
        vec4 _e11 = _group_0_binding_0_vs.color;
        return uvec4(_e11);
    }
    vec4 _e15 = _group_0_binding_0_vs.paint_linear;
    vec4 _e18 = _group_0_binding_0_vs.paint_moved;
    vec2 _e20 = place(_e15, _e18.xy, p_9);
    vec4 g_1 = _group_0_binding_0_vs.paint_geometry;
    if ((kind == 1)) {
        vec2 d_3 = (g_1.zw - g_1.xy);
        float len2_ = ((d_3.x * d_3.x) + (d_3.y * d_3.y));
        if ((len2_ > 0.0)) {
            t_2 = ((((_e20.x - g_1.x) * d_3.x) + ((_e20.y - g_1.y) * d_3.y)) / len2_);
        }
    } else {
        if ((g_1.z <= 0.0)) {
            t_2 = 1.0;
        } else {
            vec2 d_4 = (_e20 - g_1.xy);
            t_2 = (sqrt(((d_4.x * d_4.x) + (d_4.y * d_4.y))) / g_1.z);
        }
    }
    vec4 _e70[2] = _group_0_binding_0_vs.offsets;
    vec4 _e73[8] = _group_0_binding_0_vs.colors;
    float _e77 = _group_0_binding_0_vs.paint.y;
    float _e79 = t_2;
    vec4 _e80 = gradient_at(_e70, _e73, int(_e77), _e79);
    uint _e82 = quantize(_e80.w);
    float _e88 = _group_0_binding_0_vs.paint.z;
    if ((_e88 > 0.5)) {
        float _e94 = _group_0_binding_0_vs.paint.w;
        uint shift = uint(_e94);
        float _e104 = dither_offset((frag_4 + vec2(float((shift % 8u)), float((shift / 8u)))));
        step_ = (_e104 / 255.0);
    }
    float _e108 = step_;
    uint _e110 = quantize((_e80.x + _e108));
    uint r = min(_e110, _e82);
    float _e113 = step_;
    uint _e115 = quantize((_e80.y + _e113));
    uint gr = min(_e115, _e82);
    float _e118 = step_;
    uint _e120 = quantize((_e80.z + _e118));
    uint b_2 = min(_e120, _e82);
    return uvec4(r, gr, b_2, _e82);
}

void main() {
    uint index = uint(gl_VertexID);
    Instance instance = Instance(_p2vs_location0, _p2vs_location1);
    Varyings out_ = Varyings(vec4(0.0), vec2(0.0), vec2(0.0));
    vec2 uv = vec2(float((index & 1u)), float((index >> 1u)));
    vec2 pixel = mix(instance.rect.xy, instance.rect.zw, uv);
    vec4 _e18 = _group_0_binding_0_vs.frame;
    out_.position = vec4((((pixel / _e18.xy) * 2.0) - vec2(1.0)), 0.0, 1.0);
    out_.atlas = instance.atlas.xy;
    out_.first = instance.rect.xy;
    Varyings _e35 = out_;
    gl_Position = _e35.position;
    _vs2fs_location0 = _e35.atlas;
    _vs2fs_location1 = _e35.first;
    return;
}

