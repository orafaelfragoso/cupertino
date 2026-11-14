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
struct Placement {
    mat3x3 proj;
    vec4 quad;
    ivec4 flags;
};
struct VsOut {
    vec4 pos;
    vec2 px;
    vec2 uv;
};
struct Glass {
    vec4 quad;
    vec4 uv_x;
    vec4 uv_y;
    vec4 info;
    vec4 levels;
    vec4 fill;
    vec4 face;
    vec4 face_fill;
    vec4 bleed;
    vec4 highlight;
    vec4 highlight2_;
    vec4 glow;
    vec4 diffuse;
    vec4 hm[3];
    vec4 shadow;
    vec4 shadow_color;
    vec4 ring;
    vec4 shapes[16];
    vec4 shape_a[16];
    vec4 shape_b[16];
    vec4 cut;
    vec4 cut_shape;
    vec4 lens;
};
struct Field {
    float d;
    vec4 a;
    vec4 b;
};
const float CIRCULAR_EXPONENT = 2.001;
const int SHADOW_STEPS = 8;
const int MAX_STOPS = 8;
const vec3 LUMA = vec3(0.2126, 0.7152, 0.0722);
const float PI = 3.1415927;
const float LOBE_REACH = 0.75;
const float MIN_FIELD_SLOPE = 0.05;

layout(std140) uniform Placement_block_0Fragment { Placement _group_1_binding_0_fs; };

layout(std140) uniform Glass_block_1Fragment { Glass _group_0_binding_0_fs; };

uniform highp sampler2D _group_0_binding_1_fs;

uniform highp sampler2D _group_0_binding_2_fs;

uniform highp sampler2D _group_0_binding_3_fs;

uniform highp sampler2D _group_0_binding_4_fs;

uniform highp sampler2D _group_0_binding_5_fs;

smooth in vec2 _vs2fs_location0;
smooth in vec2 _vs2fs_location1;
layout(location = 0) out vec4 _fs2p_location0;

vec4 fullscreen_position(uint index) {
    float x_4 = ((float(((index << 1u) & 2u)) * 2.0) - 1.0);
    float y_2 = ((float((index & 2u)) * 2.0) - 1.0);
    return vec4(x_4, y_2, 0.0, 1.0);
}

float rounded_distance(vec2 p, vec2 half_, float e, float n_1) {
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
    if ((n_1 <= CIRCULAR_EXPONENT)) {
        return (sqrt(((qx * qx) + (qy * qy))) - e);
    }
    float u = max((qx / e), 1e-6);
    float v_1 = max((qy / e), 1e-6);
    float g = pow((pow(u, n_1) + pow(v_1, n_1)), (1.0 / n_1));
    float grad = (sqrt((pow(u, ((2.0 * n_1) - 2.0)) + pow(v_1, ((2.0 * n_1) - 2.0)))) / pow(g, (n_1 - 1.0)));
    float d_7 = (((g - 1.0) / grad) * e);
    return max(d_7, (max(qx, qy) - e));
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
    float a_2 = abs(x);
    t = (1.0 + ((0.278393 + ((0.230389 + (0.078108 * (a_2 * a_2))) * a_2)) * a_2));
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
    int i_2 = 0;
    float low_1 = (p_2.y - half_3.y);
    float high_1 = (p_2.y + half_3.y);
    float start = clamp((-3.0 * sigma_2), low_1, high_1);
    float end = clamp((3.0 * sigma_2), low_1, high_1);
    float step_ = ((end - start) / 8.0);
    y_1 = (start + (step_ * 0.5));
    bool loop_init = true;
    while(true) {
        if (!loop_init) {
            int _e43 = i_2;
            i_2 = (_e43 + 1);
        }
        loop_init = false;
        int _e27 = i_2;
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
    int i_3 = 0;
    float step_1 = ((6.0 * sigma_3) / 8.0);
    bool loop_init_1 = true;
    while(true) {
        if (!loop_init_1) {
            int _e24 = i_3;
            i_3 = (_e24 + 1);
        }
        loop_init_1 = false;
        int _e9 = i_3;
        if ((_e9 < SHADOW_STEPS)) {
        } else {
            break;
        }
        {
            float _e12 = mass;
            int _e15 = i_3;
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
    vec2 q_2 = (p_3 - shadow.offset);
    float _e16 = rounded_box_blur(q_2, caster.half_, shadow.sigma, radius);
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
    vec2 q_3 = (p_4 - shadow_1.offset);
    float _e15 = rounded_box_blur(q_3, caster_1.half_, shadow_1.sigma, radius_1);
    float _e17 = blur_mass(shadow_1.sigma);
    float blurred_1 = (_e15 / _e17);
    float outside_1 = max((1.0 - blurred_1), 0.0);
    float extent_1 = min(caster_1.extent, min(caster_1.half_.x, caster_1.half_.y));
    float _e32 = rounded_distance(p_4, caster_1.half_, extent_1, caster_1.exponent);
    float _e33 = coverage(_e32, px_3);
    return (clamp((outside_1 * shadow_1.density), 0.0, 1.0) * _e33);
}

vec4 gradient_at(vec4 offsets[2], vec4 colors[8], int count, float t_in) {
    float t_1 = 0.0;
    vec4 straight = vec4(0.0);
    int i_4 = 0;
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
                        i_4 = _e50;
                        break;
                    }
                    int _e51 = j;
                    i_4 = _e51;
                }
            }
            int _e55 = i_4;
            int _e59 = i_4;
            float a_3 = offsets[(_e55 / 4)][(_e59 % 4)];
            int _e63 = i_4;
            int _e69 = i_4;
            float b_1 = offsets[((_e63 + 1) / 4)][((_e69 + 1) % 4)];
            float span = (b_1 - a_3);
            if ((span > 0.0)) {
                float _e80 = t_1;
                k = ((_e80 - a_3) / span);
            }
            int _e83 = i_4;
            int _e85 = i_4;
            int _e89 = i_4;
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
    uint a_4 = ((p_5.x >> k_1) & 1u);
    uint b_2 = ((p_5.y >> k_1) & 1u);
    return ((2u * (a_4 ^ b_2)) + b_2);
}

uint dither_matrix(vec2 frag) {
    uvec2 p_15 = uvec2(floor(frag));
    uint _e5 = dither_pair(p_15, 0u);
    uint _e9 = dither_pair(p_15, 1u);
    uint _e13 = dither_pair(p_15, 2u);
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

vec3 dither_rgb(vec3 c_1, vec2 frag_3) {
    float _e2 = dither_offset(frag_3);
    return (c_1 + vec3((_e2 / 255.0)));
}

vec4 placed(vec4 c_2, vec2 frag_4) {
    int _e5 = _group_1_binding_0_fs.flags.x;
    if ((_e5 != 0)) {
        vec3 _e9 = dither_rgb(c_2.xyz, frag_4);
        return vec4(_e9, c_2.w);
    }
    return c_2;
}

float rect_sdf(vec2 p_6, vec4 r, float e_1, float n_2) {
    vec2 hs = (r.zw * 0.5);
    float ext = clamp(e_1, 0.0, min(hs.x, hs.y));
    vec2 q_4 = (abs((p_6 - (r.xy + hs))) - (hs - vec2(ext)));
    if ((((q_4.x <= 0.0) || (q_4.y <= 0.0)) || (ext <= 0.0))) {
        return ((length(max(q_4, vec2(0.0))) + min(max(q_4.x, q_4.y), 0.0)) - ext);
    }
    if ((n_2 <= 2.0)) {
        return (length(q_4) - ext);
    }
    vec2 u_1 = max((q_4 / vec2(ext)), vec2(1e-6));
    vec2 un = pow(u_1, vec2(n_2));
    float g_1 = pow((un.x + un.y), (1.0 / n_2));
    vec2 slope = ((un / u_1) / vec2(pow(g_1, (n_2 - 1.0))));
    return max((((g_1 - 1.0) * ext) / max(length(slope), 1e-6)), (max(q_4.x, q_4.y) - ext));
}

float refraction_profile(float t_2) {
    if ((t_2 >= 1.0)) {
        return 0.0;
    }
    float t2_ = (t_2 * t_2);
    return ((0.9 * exp((-4.2 * t_2))) * (1.0 - (t2_ * t2_)));
}

Field field(vec2 p_7) {
    Field f = Field(0.0, vec4(0.0), vec4(0.0));
    int i_5 = 1;
    float h = 0.0;
    float d_2 = 0.0;
    float _e4 = _group_0_binding_0_fs.info.x;
    int n_7 = int(_e4);
    float k_3 = _group_0_binding_0_fs.info.y;
    vec4 _e15 = _group_0_binding_0_fs.shapes[0];
    float _e20 = _group_0_binding_0_fs.shape_a[0].x;
    float _e25 = _group_0_binding_0_fs.shape_b[0].w;
    float _e26 = rect_sdf(p_7, _e15, _e20, _e25);
    f.d = _e26;
    vec4 _e31 = _group_0_binding_0_fs.shape_a[0];
    f.a = _e31;
    vec4 _e36 = _group_0_binding_0_fs.shape_b[0];
    f.b = _e36;
    bool loop_init_3 = true;
    while(true) {
        if (!loop_init_3) {
            int _e117 = i_5;
            i_5 = (_e117 + 1);
        }
        loop_init_3 = false;
        int _e39 = i_5;
        if ((_e39 < n_7)) {
        } else {
            break;
        }
        {
            int _e43 = i_5;
            vec4 _e45 = _group_0_binding_0_fs.shapes[_e43];
            int _e48 = i_5;
            float _e51 = _group_0_binding_0_fs.shape_a[_e48].x;
            int _e54 = i_5;
            float _e57 = _group_0_binding_0_fs.shape_b[_e54].w;
            float _e58 = rect_sdf(p_7, _e45, _e51, _e57);
            if ((k_3 <= 0.0)) {
                float _e66 = f.d;
                h = ((_e58 <= _e66) ? 1.0 : 0.0);
                float _e70 = f.d;
                d_2 = min(_e58, _e70);
            } else {
                float _e73 = f.d;
                h = clamp((0.5 + ((0.5 * (_e73 - _e58)) / k_3)), 0.0, 1.0);
                float _e84 = f.d;
                float _e85 = h;
                float _e87 = h;
                float _e89 = h;
                d_2 = (mix(_e84, _e58, _e85) - ((k_3 * _e87) * (1.0 - _e89)));
            }
            vec4 _e96 = f.a;
            int _e99 = i_5;
            vec4 _e101 = _group_0_binding_0_fs.shape_a[_e99];
            float _e102 = h;
            f.a = mix(_e96, _e101, _e102);
            vec4 _e106 = f.b;
            int _e109 = i_5;
            vec4 _e111 = _group_0_binding_0_fs.shape_b[_e109];
            float _e112 = h;
            f.b = mix(_e106, _e111, _e112);
            float _e115 = d_2;
            f.d = _e115;
        }
    }
    Field _e119 = f;
    return _e119;
}

float field_d(vec2 p_8) {
    Field _e1 = field(p_8);
    return _e1.d;
}

vec2 uv_of(vec2 p_9) {
    vec3 h_1 = vec3(p_9, 1.0);
    vec4 _e5 = _group_0_binding_0_fs.uv_x;
    vec4 _e10 = _group_0_binding_0_fs.uv_y;
    return vec2(dot(_e5.xyz, h_1), dot(_e10.xyz, h_1));
}

vec3 saturate_encoded(vec3 c_3, float s) {
    float y_3 = dot(c_3, LUMA);
    return max((vec3(y_3) + ((c_3 - vec3(y_3)) * s)), vec3(0.0));
}

vec3 tap(highp sampler2D t_3, vec2 uv) {
    vec4 _e4 = textureLod(t_3, vec2(uv), 0.0);
    return _e4.xyz;
}

vec3 blurred(vec2 uv_1, float pos) {
    vec3 c_4 = vec3(0.0);
    vec4 w_2 = max((vec4(1.0) - abs((vec4(clamp(pos, 0.0, 3.0)) - vec4(0.0, 1.0, 2.0, 3.0)))), vec4(0.0));
    if ((w_2.x > 0.0)) {
        vec3 _e27 = tap(_group_0_binding_1_fs, uv_1);
        vec3 _e29 = c_4;
        c_4 = (_e29 + (w_2.x * _e27));
    }
    if ((w_2.y > 0.0)) {
        vec3 _e36 = tap(_group_0_binding_2_fs, uv_1);
        vec3 _e38 = c_4;
        c_4 = (_e38 + (w_2.y * _e36));
    }
    if ((w_2.z > 0.0)) {
        vec3 _e45 = tap(_group_0_binding_3_fs, uv_1);
        vec3 _e47 = c_4;
        c_4 = (_e47 + (w_2.z * _e45));
    }
    if ((w_2.w > 0.0)) {
        vec3 _e54 = tap(_group_0_binding_4_fs, uv_1);
        vec3 _e56 = c_4;
        c_4 = (_e56 + (w_2.w * _e54));
    }
    vec3 _e58 = c_4;
    return _e58;
}

vec3 face_at(vec2 q_1, float pos_1, float max_luma, float w_1) {
    vec3 c_5 = vec3(0.0);
    vec3 s_1 = vec3(0.0);
    float l = 0.0;
    vec2 _e4 = uv_of(q_1);
    vec3 _e6 = blurred(_e4, (pos_1 * w_1));
    c_5 = _e6;
    vec3 _e9 = tap(_group_0_binding_5_fs, _e4);
    vec3 _e10 = c_5;
    vec3 _e11 = c_5;
    float _e16 = _group_0_binding_0_fs.fill.x;
    c_5 = mix(_e10, min(_e11, _e9), (_e16 * w_1));
    vec3 _e19 = c_5;
    vec3 _e20 = c_5;
    float _e25 = _group_0_binding_0_fs.fill.y;
    c_5 = mix(_e19, max(_e20, _e9), (_e25 * w_1));
    vec3 _e28 = c_5;
    float _e32 = _group_0_binding_0_fs.fill.z;
    c_5 = mix(_e28, _e9, (_e32 * w_1));
    vec3 _e35 = c_5;
    float _e39 = _group_0_binding_0_fs.face.x;
    vec3 _e40 = saturate_encoded(_e35, _e39);
    s_1 = _e40;
    vec3 _e42 = s_1;
    l = dot(_e42, LUMA);
    vec3 _e46 = s_1;
    float _e49 = l;
    vec3 _e51 = s_1;
    float _e54 = l;
    s_1 = (_e46 - (((1.0 - max_luma) * _e49) * ((0.75 * _e51) + vec3((0.25 * _e54)))));
    vec3 _e61 = s_1;
    l = dot(_e61, LUMA);
    vec3 _e64 = s_1;
    float _e68 = _group_0_binding_0_fs.face.z;
    float _e69 = l;
    float _e76 = _group_0_binding_0_fs.face.y;
    float _e79 = l;
    s_1 = (_e64 + vec3(((_e68 * (1.0 - _e69)) - ((1.0 - _e76) * _e79))));
    vec3 _e84 = s_1;
    float _e90 = _group_0_binding_0_fs.face.w;
    vec3 e_2 = clamp(_e84, vec3(0.0), vec3(min(_e90, 1.0)));
    vec4 _e97 = _group_0_binding_0_fs.face_fill;
    float _e102 = _group_0_binding_0_fs.face_fill.w;
    return mix(e_2, _e97.xyz, _e102);
}

float angle_between(float a_1, float b) {
    float d_8 = ((a_1 - b) - (floor(((a_1 - b) / 6.2831855)) * 6.2831855));
    return min(d_8, (6.2831855 - d_8));
}

float lobe(float normal_angle, float light, float spread_scale) {
    float _e6 = _group_0_binding_0_fs.highlight.z;
    float reach = ((_e6 * spread_scale) * LOBE_REACH);
    if ((reach <= 0.0)) {
        return 0.0;
    }
    float _e13 = angle_between(normal_angle, light);
    if ((_e13 >= reach)) {
        return 0.0;
    }
    float _e27 = _group_0_binding_0_fs.highlight2_.z;
    return pow((0.5 + (0.5 * cos(((_e13 / reach) * PI)))), max(_e27, 0.001));
}

vec3 key_color(vec3 c_6) {
    vec4 h_2 = vec4(c_6, 1.0);
    vec4 _e6 = _group_0_binding_0_fs.hm[0];
    vec4 _e11 = _group_0_binding_0_fs.hm[1];
    vec4 _e16 = _group_0_binding_0_fs.hm[2];
    return clamp(vec3(dot(_e6, h_2), dot(_e11, h_2), dot(_e16, h_2)), vec3(0.0), vec3(1.0));
}

float shadow_coverage(float d_3, float r_1) {
    if ((r_1 <= 0.0)) {
        return ((d_3 <= 0.0) ? 1.0 : 0.0);
    }
    float t_4 = clamp((0.5 - (d_3 / (3.0 * r_1))), 0.0, 1.0);
    return ((t_4 * t_4) * (3.0 - (2.0 * t_4)));
}

float rim_brightness(float l0_) {
    float l_1 = clamp(l0_, 0.0, 1.0);
    if ((l_1 < 0.25)) {
        return ((l_1 * 4.0) * 0.63);
    }
    if ((l_1 < 0.5)) {
        return mix(0.63, 1.0, ((l_1 - 0.25) * 4.0));
    }
    if ((l_1 < 0.75)) {
        return mix(1.0, 1.13, ((l_1 - 0.5) * 4.0));
    }
    return mix(1.13, 1.0, ((l_1 - 0.75) * 4.0));
}

float rim_direction(vec2 n_3) {
    float s_2 = ((2.0 * n_3.x) * n_3.y);
    return (0.15 + ((0.85 * s_2) * s_2));
}

float normal_cdf(float x_3) {
    return (1.0 / (1.0 + exp((-1.702 * x_3))));
}

float outside_alpha(vec2 p_10, float d_4) {
    float drop = 0.0;
    float line = 0.0;
    float _e7 = _group_0_binding_0_fs.shadow.z;
    if ((_e7 > 0.0)) {
        float _e13 = _group_0_binding_0_fs.shadow.z;
        vec4 _e16 = _group_0_binding_0_fs.shadow;
        float _e19 = field_d((p_10 - _e16.xy));
        float _e23 = _group_0_binding_0_fs.shadow.w;
        float _e24 = shadow_coverage(_e19, _e23);
        drop = (_e13 * _e24);
    }
    float _e31 = _group_0_binding_0_fs.glow.w;
    float _e37 = _group_0_binding_0_fs.ring.w;
    if (((_e31 > 0.0) && (_e37 > 0.0))) {
        float _e44 = _group_0_binding_0_fs.glow.w;
        float _e48 = _group_0_binding_0_fs.ring.w;
        line = (_e44 * clamp(((_e48 + 0.5) - d_4), 0.0, 1.0));
    }
    float _e56 = drop;
    float _e57 = line;
    return min((_e56 + _e57), 1.0);
}

vec2 field_normal(vec2 p_11) {
    float _e5 = field_d((p_11 + vec2(0.5, 0.0)));
    float _e10 = field_d((p_11 - vec2(0.5, 0.0)));
    float _e16 = field_d((p_11 + vec2(0.0, 0.5)));
    float _e21 = field_d((p_11 - vec2(0.0, 0.5)));
    vec2 g_2 = vec2((_e5 - _e10), (_e16 - _e21));
    float grad_len = length(g_2);
    if ((grad_len > 1e-6)) {
        return (g_2 / vec2(grad_len));
    }
    return vec2(0.0);
}

float edge_line_side_share(vec2 n_4) {
    float _e4 = _group_0_binding_0_fs.diffuse.w;
    float reach_1 = (_e4 * LOBE_REACH);
    if (((reach_1 <= 0.0) || (dot(n_4, n_4) < 1e-12))) {
        return 1.0;
    }
    float a_5 = acos(clamp((abs(n_4.x) / length(n_4)), 0.0, 1.0));
    if ((a_5 >= reach_1)) {
        return 0.0;
    }
    return (0.5 + (0.5 * cos(((a_5 / reach_1) * PI))));
}

float edge_line(vec2 p_12, float d_5, float pos_2, vec2 n_5) {
    float response = 1.0;
    float band = clamp((1.5 - d_5), 0.0, 1.0);
    float _e12 = _group_0_binding_0_fs.levels.x;
    if (((_e12 <= 0.0) || (band <= 0.0))) {
        return 0.0;
    }
    float _e19 = edge_line_side_share(n_5);
    if ((_e19 <= 0.0)) {
        return 0.0;
    }
    float _e28 = _group_0_binding_0_fs.cut_shape.w;
    if ((_e28 <= 0.5)) {
        vec2 _e31 = uv_of(p_12);
        vec3 _e32 = blurred(_e31, pos_2);
        float _e35 = rim_brightness(dot(_e32, LUMA));
        response = _e35;
    }
    float _e39 = _group_0_binding_0_fs.levels.x;
    float _e42 = response;
    return (((min(_e39, 1.0) * _e42) * band) * _e19);
}

vec4 outside(vec2 p_13, float d_6, float pos_3, vec2 n_6) {
    float _e4 = outside_alpha(p_13, d_6);
    float _e5 = edge_line(p_13, d_6, pos_3, n_6);
    float a_6 = (_e5 + (_e4 * (1.0 - _e5)));
    if ((a_6 <= 0.0)) {
        vec4 _e14 = _group_0_binding_0_fs.shadow_color;
        return vec4(_e14.xyz, 0.0);
    }
    vec4 _e20 = _group_0_binding_0_fs.shadow_color;
    return vec4((_e20.xyz * ((_e4 * (1.0 - _e5)) / a_6)), a_6);
}

float cut_coverage(vec2 p_14) {
    float _e4 = _group_0_binding_0_fs.cut_shape.z;
    if ((_e4 <= 0.0)) {
        return 1.0;
    }
    vec4 _e10 = _group_0_binding_0_fs.cut;
    float _e14 = _group_0_binding_0_fs.cut_shape.x;
    float _e18 = _group_0_binding_0_fs.cut_shape.y;
    float _e19 = rect_sdf(p_14, _e10, _e14, _e18);
    float _e23 = _group_0_binding_0_fs.info.z;
    return clamp((0.5 - (_e19 / _e23)), 0.0, 1.0);
}

void main() {
    VsOut fin = VsOut(gl_FragCoord, _vs2fs_location0, _vs2fs_location1);
    vec2 on = vec2(0.0);
    vec2 n = vec2(0.0);
    vec2 off = vec2(0.0);
    vec2 q = vec2(0.0);
    float blur_weight = 1.0;
    vec3 c = vec3(0.0);
    float w = 0.0;
    float top = 0.0;
    float bottom = 0.0;
    int i_1 = 1;
    float m = 0.0;
    vec3 rgb = vec3(0.0);
    vec2 p_16 = fin.px;
    float _e2 = cut_coverage(p_16);
    if ((_e2 <= 0.0)) {
        discard;
    }
    Field _e5 = field(p_16);
    float d_9 = _e5.d;
    if ((d_9 > 0.5)) {
        float _e15 = _group_0_binding_0_fs.diffuse.w;
        float _e21 = _group_0_binding_0_fs.levels.x;
        if ((((_e15 > 0.0) && (_e21 > 0.0)) && (d_9 < 1.5))) {
            vec2 _e28 = field_normal(p_16);
            on = _e28;
        }
        vec2 _e31 = on;
        vec4 _e32 = outside(p_16, d_9, _e5.b.z, _e31);
        float _e37 = _group_0_binding_0_fs.info.w;
        float a_7 = ((_e32.w * _e37) * _e2);
        if ((a_7 <= 0.0)) {
            discard;
        }
        vec4 _e46 = placed(vec4(_e32.xyz, a_7), fin.pos.xy);
        _fs2p_location0 = _e46;
        return;
    }
    float _e51 = field_d((p_16 + vec2(0.5, 0.0)));
    float _e56 = field_d((p_16 - vec2(0.5, 0.0)));
    float _e62 = field_d((p_16 + vec2(0.0, 0.5)));
    float _e67 = field_d((p_16 - vec2(0.0, 0.5)));
    vec2 g_3 = vec2((_e51 - _e56), (_e62 - _e67));
    float grad_len_1 = length(g_3);
    if ((grad_len_1 > 1e-6)) {
        n = (g_3 / vec2(grad_len_1));
    }
    if (((_e5.a.z > 0.0) && (d_9 <= 0.0))) {
        vec2 _e88 = n;
        float _e98 = refraction_profile(max((-(d_9) / _e5.a.z), 0.0));
        off = ((_e88 * _e5.a.y) * _e98);
    }
    vec2 _e100 = off;
    q = (p_16 + _e100);
    float _e106 = _group_0_binding_0_fs.lens.z;
    if ((_e106 > 1.0)) {
        vec4 _e112 = _group_0_binding_0_fs.shapes[0];
        vec4 _e117 = _group_0_binding_0_fs.shapes[0];
        vec2 lc = (_e112.xy + (0.5 * _e117.zw));
        vec2 _e122 = q;
        float _e127 = _group_0_binding_0_fs.lens.z;
        q = (lc + ((_e122 - lc) / vec2(_e127)));
    }
    float _e136 = _group_0_binding_0_fs.lens.x;
    float _e142 = _group_0_binding_0_fs.lens.y;
    if (((_e136 < 1.0) || (_e142 < 1.0))) {
        float _e151 = _group_0_binding_0_fs.shapes[0].y;
        float _e157 = _group_0_binding_0_fs.shapes[0].w;
        float rt = clamp(((p_16.y - _e151) / max(_e157, 0.001)), 0.0, 1.0);
        float _e167 = _group_0_binding_0_fs.lens.x;
        float _e171 = _group_0_binding_0_fs.lens.y;
        blur_weight = mix(_e167, _e171, smoothstep(0.0, 1.0, rt));
    }
    vec2 _e176 = q;
    float _e181 = blur_weight;
    vec3 _e182 = face_at(_e176, _e5.b.z, _e5.b.y, _e181);
    c = _e182;
    if (((_e5.a.w > 0.0) && (_e5.b.x > 0.0))) {
        float t_5 = clamp((-(d_9) / _e5.b.x), 0.0, 1.0);
        float w_3 = ((_e5.a.w * (1.0 - t_5)) * (1.0 - t_5));
        float _e211 = _group_0_binding_0_fs.bleed.y;
        float _e216 = _group_0_binding_0_fs.bleed.x;
        float _e220 = _group_0_binding_0_fs.bleed.y;
        vec2 _e223 = uv_of(p_16);
        vec3 _e224 = tap(_group_0_binding_5_fs, _e223);
        float _e228 = _group_0_binding_0_fs.bleed.z;
        vec3 _e229 = saturate_encoded(_e224, _e228);
        vec3 tone = (vec3(_e211) + ((_e216 - _e220) * min(_e229, vec3(1.0))));
        vec3 _e235 = c;
        vec3 _e237 = c;
        float _e242 = _group_0_binding_0_fs.bleed.w;
        vec3 b_3 = ((_e242 > 0.5) ? min(_e237, tone) : max(_e235, tone));
        vec3 _e246 = c;
        c = mix(_e246, b_3, w_3);
    }
    float _e251 = _group_0_binding_0_fs.highlight.x;
    float _e255 = _group_0_binding_0_fs.highlight2_.y;
    float _e262 = _group_0_binding_0_fs.highlight.w;
    bool rim_on = (((_e251 * _e255) != 0.0) && (_e262 > 0.0));
    float _e269 = _group_0_binding_0_fs.glow.x;
    float _e275 = _group_0_binding_0_fs.glow.y;
    bool glow_on = ((_e269 > 0.0) && (_e275 > 0.0));
    if (((rim_on || glow_on) && (grad_len_1 > 1e-6))) {
        float de = (d_9 / max(grad_len_1, MIN_FIELD_SLOPE));
        if (glow_on) {
            float _e287 = n.y;
            float _e290 = n.x;
            float na = atan(-(_e287), _e290);
            float _e295 = _group_0_binding_0_fs.highlight2_.w;
            float fill = max(_e295, 0.0);
            float _e301 = _group_0_binding_0_fs.highlight.y;
            float _e303 = lobe(na, _e301, 1.0);
            float _e307 = _group_0_binding_0_fs.highlight.y;
            float _e311 = lobe(na, (_e307 + PI), 1.0);
            float ends = (_e303 + (_e311 * fill));
            float _e317 = _group_0_binding_0_fs.glow.z;
            float _e322 = _group_0_binding_0_fs.glow.y;
            float xg = ((de - _e317) / _e322);
            float _e327 = _group_0_binding_0_fs.glow.x;
            w = ((_e327 * exp((-(xg) * xg))) * ends);
            float _e337 = _group_0_binding_0_fs.diffuse.z;
            float _e343 = _group_0_binding_0_fs.diffuse.x;
            if (((_e337 > 0.0) && (_e343 > 0.0))) {
                float _e350 = _group_0_binding_0_fs.glow.z;
                float _e355 = _group_0_binding_0_fs.glow.y;
                float _e359 = _group_0_binding_0_fs.diffuse.x;
                float xd = ((de - _e350) / (_e355 * _e359));
                float _e365 = _group_0_binding_0_fs.highlight.y;
                float _e369 = _group_0_binding_0_fs.diffuse.y;
                float _e370 = lobe(na, _e365, _e369);
                float _e374 = _group_0_binding_0_fs.highlight.y;
                float _e380 = _group_0_binding_0_fs.diffuse.y;
                float _e381 = lobe(na, (_e374 + PI), _e380);
                float wide = (_e370 + (_e381 * fill));
                float _e384 = w;
                float _e388 = _group_0_binding_0_fs.glow.x;
                float _e392 = _group_0_binding_0_fs.diffuse.z;
                w = (_e384 + (((_e388 * _e392) * exp((-(xd) * xd))) * wide));
            }
            vec3 _e400 = c;
            vec3 _e401 = c;
            vec3 _e402 = key_color(_e401);
            float _e403 = w;
            c = min((_e400 + (_e402 * _e403)), vec3(1.0));
        }
        if (rim_on) {
            float _e412 = _group_0_binding_0_fs.highlight.w;
            float band_1 = clamp(((_e412 + 0.5) - max(-(de), 0.0)), 0.0, 1.0);
            float _e425 = _group_0_binding_0_fs.highlight.x;
            float _e429 = _group_0_binding_0_fs.highlight2_.y;
            vec3 _e431 = c;
            float _e434 = rim_brightness(dot(_e431, LUMA));
            vec2 _e436 = n;
            float _e437 = rim_direction(_e436);
            float shift = ((((_e425 * _e429) * _e434) * _e437) * band_1);
            vec3 _e440 = c;
            c = clamp((_e440 + vec3(shift)), vec3(0.0), vec3(1.0));
        }
    }
    float _e451 = _group_0_binding_0_fs.fill.w;
    float _e457 = _group_0_binding_0_fs.levels.y;
    if (((_e451 > 0.0) || (_e457 > 0.0))) {
        float _e464 = _group_0_binding_0_fs.info.x;
        int vn = int(_e464);
        float _e470 = _group_0_binding_0_fs.shapes[0].y;
        top = _e470;
        float _e476 = _group_0_binding_0_fs.shapes[0].y;
        float _e481 = _group_0_binding_0_fs.shapes[0].w;
        bottom = (_e476 + _e481);
        bool loop_init_4 = true;
        while(true) {
            if (!loop_init_4) {
                int _e512 = i_1;
                i_1 = (_e512 + 1);
            }
            loop_init_4 = false;
            int _e486 = i_1;
            if ((_e486 < vn)) {
            } else {
                break;
            }
            {
                float _e488 = top;
                int _e491 = i_1;
                float _e494 = _group_0_binding_0_fs.shapes[_e491].y;
                top = min(_e488, _e494);
                float _e496 = bottom;
                int _e499 = i_1;
                float _e502 = _group_0_binding_0_fs.shapes[_e499].y;
                int _e505 = i_1;
                float _e508 = _group_0_binding_0_fs.shapes[_e505].w;
                bottom = max(_e496, (_e502 + _e508));
            }
        }
        float _e515 = top;
        float _e517 = bottom;
        float _e518 = top;
        float vt = clamp(((p_16.y - _e515) / max((_e517 - _e518), 0.001)), 0.0, 1.0);
        vec3 _e526 = c;
        float _e530 = _group_0_binding_0_fs.fill.w;
        float _e534 = _group_0_binding_0_fs.levels.y;
        c = (_e526 * (1.0 - clamp(mix(_e530, _e534, vt), 0.0, 1.0)));
    }
    float _e545 = _group_0_binding_0_fs.shadow_color.w;
    float _e551 = _group_0_binding_0_fs.ring.y;
    if (((_e545 > 0.0) && (_e551 > 0.0))) {
        float _e558 = _group_0_binding_0_fs.ring.x;
        float _e562 = field_d((p_16 + vec2(0.0, _e558)));
        float _e568 = _group_0_binding_0_fs.ring.z;
        if ((_e568 > 0.0)) {
            float _e575 = _group_0_binding_0_fs.ring.z;
            float _e577 = normal_cdf((-(_e562) / _e575));
            float _e581 = _group_0_binding_0_fs.ring.y;
            float _e587 = _group_0_binding_0_fs.ring.z;
            float _e589 = normal_cdf(((-(_e581) - _e562) / _e587));
            m = (_e577 - _e589);
        } else {
            float _e596 = _group_0_binding_0_fs.ring.y;
            if (((_e562 <= 0.0) && (_e562 >= -(_e596)))) {
                m = 1.0;
            }
        }
        vec3 _e601 = c;
        float _e605 = _group_0_binding_0_fs.shadow_color.w;
        float _e608 = m;
        c = (_e601 * (1.0 - (min(_e605, 1.0) * max(_e608, 0.0))));
    }
    float _e618 = _group_0_binding_0_fs.info.z;
    float cov = clamp((0.5 - (d_9 / _e618)), 0.0, 1.0);
    vec2 _e627 = n;
    vec4 _e628 = outside(p_16, d_9, _e5.b.z, _e627);
    float under = ((1.0 - cov) * _e628.w);
    float a_8 = (cov + under);
    vec3 _e634 = c;
    rgb = _e634;
    if ((a_8 > 0.0)) {
        vec3 _e638 = c;
        rgb = (((clamp(_e638, vec3(0.0), vec3(1.0)) * cov) + (_e628.xyz * under)) / vec3(a_8));
    }
    vec3 _e650 = rgb;
    float _e659 = _group_0_binding_0_fs.info.w;
    vec4 _e665 = placed(vec4(clamp(_e650, vec3(0.0), vec3(1.0)), ((a_8 * _e659) * _e2)), fin.pos.xy);
    _fs2p_location0 = _e665;
    return;
}

