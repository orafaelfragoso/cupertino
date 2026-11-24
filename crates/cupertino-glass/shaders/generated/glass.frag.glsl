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
    vec4 bleed;
    vec4 light;
    vec4 glow;
    vec4 diffuse;
    vec4 hm[3];
    vec4 shadow;
    vec4 shadow_color;
    vec4 shapes[16];
    vec4 shape_a[16];
    vec4 shape_b[16];
    vec4 shape_c[16];
    vec4 shape_face[16];
    vec4 shape_fill[16];
    vec4 shape_tint[16];
    vec4 shape_rim[16];
    vec4 shape_spec[16];
    vec4 shape_spec2_[16];
    vec4 shape_ring[16];
    vec4 shape_border[16];
    vec4 cut;
    vec4 cut_shape;
    vec4 lens;
    vec4 edge;
};
struct Field {
    float d;
    vec4 a;
    vec4 b;
    vec4 c;
    vec4 face;
    vec4 fill;
    vec4 tint;
    vec4 rim;
    vec4 spec;
    vec4 spec2_;
    vec4 ring;
    vec4 border;
};
const float CIRCULAR_EXPONENT = 2.001;
const int SHADOW_STEPS = 8;
const int MAX_STOPS = 8;
const vec3 LUMA = vec3(0.2126, 0.7152, 0.0722);
const float PI = 3.1415927;
const float BORDER_LIT_SHARE = 0.2;
const float LOBE_REACH = 0.75;
const float MIN_FIELD_SLOPE = 0.05;

layout(std140) uniform Placement_block_0Fragment { Placement _group_1_binding_0_fs; };

layout(std140) uniform Glass_block_1Fragment { Glass _group_0_binding_0_fs; };

uniform highp sampler2D _group_0_binding_1_fs;

uniform highp sampler2D _group_0_binding_2_fs;

uniform highp sampler2D _group_0_binding_3_fs;

uniform highp sampler2D _group_0_binding_4_fs;

uniform highp sampler2D _group_0_binding_5_fs;

uniform highp sampler2D _group_0_binding_6_fs;

uniform highp sampler2D _group_0_binding_7_fs;

smooth in vec2 _vs2fs_location0;
smooth in vec2 _vs2fs_location1;
layout(location = 0) out vec4 _fs2p_location0;

vec4 fullscreen_position(uint index) {
    float x_6 = ((float(((index << 1u) & 2u)) * 2.0) - 1.0);
    float y_3 = ((float((index & 2u)) * 2.0) - 1.0);
    return vec4(x_6, y_3, 0.0, 1.0);
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
    float u_1 = max((qx / e), 1e-6);
    float v_1 = max((qy / e), 1e-6);
    float g = pow((pow(u_1, n_1) + pow(v_1, n_1)), (1.0 / n_1));
    float grad = (sqrt((pow(u_1, ((2.0 * n_1) - 2.0)) + pow(v_1, ((2.0 * n_1) - 2.0)))) / pow(g, (n_1 - 1.0)));
    float d_12 = (((g - 1.0) / grad) * e);
    return max(d_12, (max(qx, qy) - e));
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
    float outside_2 = max((1.0 - blurred_1), 0.0);
    float extent_1 = min(caster_1.extent, min(caster_1.half_.x, caster_1.half_.y));
    float _e32 = rounded_distance(p_4, caster_1.half_, extent_1, caster_1.exponent);
    float _e33 = coverage(_e32, px_3);
    return (clamp((outside_2 * shadow_1.density), 0.0, 1.0) * _e33);
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
    uvec2 p_18 = uvec2(floor(frag));
    uint _e5 = dither_pair(p_18, 0u);
    uint _e9 = dither_pair(p_18, 1u);
    uint _e13 = dither_pair(p_18, 2u);
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
    vec2 u_2 = max((q_4 / vec2(ext)), vec2(1e-6));
    vec2 un = pow(u_2, vec2(n_2));
    float g_1 = pow((un.x + un.y), (1.0 / n_2));
    vec2 slope = ((un / u_2) / vec2(pow(g_1, (n_2 - 1.0))));
    return max((((g_1 - 1.0) * ext) / max(length(slope), 1e-6)), (max(q_4.x, q_4.y) - ext));
}

float refraction_profile(float t_2) {
    if ((t_2 >= 1.0)) {
        return 0.0;
    }
    float t2_ = (t_2 * t_2);
    return ((0.9 * exp((-4.2 * t_2))) * (1.0 - (t2_ * t2_)));
}

vec4 towards(vec4 x_3, vec4 y_2, float h) {
    return (x_3 + ((y_2 - x_3) * h));
}

Field field(vec2 p_7) {
    Field f = Field(0.0, vec4(0.0), vec4(0.0), vec4(0.0), vec4(0.0), vec4(0.0), vec4(0.0), vec4(0.0), vec4(0.0), vec4(0.0), vec4(0.0), vec4(0.0));
    int i_5 = 1;
    float h_1 = 0.0;
    float d_2 = 0.0;
    int i_6 = 0;
    float _e4 = _group_0_binding_0_fs.info.x;
    int n_9 = int(_e4);
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
    vec4 _e41 = _group_0_binding_0_fs.shape_c[0];
    f.c = _e41;
    vec4 _e46 = _group_0_binding_0_fs.shape_face[0];
    f.face = _e46;
    vec4 _e51 = _group_0_binding_0_fs.shape_fill[0];
    f.fill = _e51;
    vec4 _e56 = _group_0_binding_0_fs.shape_tint[0];
    f.tint = _e56;
    vec4 _e61 = _group_0_binding_0_fs.shape_rim[0];
    f.rim = _e61;
    vec4 _e66 = _group_0_binding_0_fs.shape_spec[0];
    f.spec = _e66;
    vec4 _e71 = _group_0_binding_0_fs.shape_spec2_[0];
    f.spec2_ = _e71;
    vec4 _e76 = _group_0_binding_0_fs.shape_ring[0];
    f.ring = _e76;
    vec4 _e81 = _group_0_binding_0_fs.shape_border[0];
    f.border = _e81;
    bool loop_init_3 = true;
    while(true) {
        if (!loop_init_3) {
            int _e252 = i_5;
            i_5 = (_e252 + 1);
        }
        loop_init_3 = false;
        int _e84 = i_5;
        if ((_e84 < n_9)) {
        } else {
            break;
        }
        {
            int _e88 = i_5;
            vec4 _e90 = _group_0_binding_0_fs.shapes[_e88];
            int _e93 = i_5;
            float _e96 = _group_0_binding_0_fs.shape_a[_e93].x;
            int _e99 = i_5;
            float _e102 = _group_0_binding_0_fs.shape_b[_e99].w;
            float _e103 = rect_sdf(p_7, _e90, _e96, _e102);
            if ((k_3 <= 0.0)) {
                float _e111 = f.d;
                h_1 = ((_e103 <= _e111) ? 1.0 : 0.0);
                float _e115 = f.d;
                d_2 = min(_e103, _e115);
            } else {
                float _e118 = f.d;
                h_1 = clamp((0.5 + ((0.5 * (_e118 - _e103)) / k_3)), 0.0, 1.0);
                float _e129 = f.d;
                float _e130 = h_1;
                float _e132 = h_1;
                float _e134 = h_1;
                d_2 = (mix(_e129, _e103, _e130) - ((k_3 * _e132) * (1.0 - _e134)));
            }
            vec4 _e141 = f.a;
            int _e144 = i_5;
            vec4 _e146 = _group_0_binding_0_fs.shape_a[_e144];
            float _e147 = h_1;
            f.a = mix(_e141, _e146, _e147);
            vec4 _e151 = f.b;
            int _e154 = i_5;
            vec4 _e156 = _group_0_binding_0_fs.shape_b[_e154];
            float _e157 = h_1;
            f.b = mix(_e151, _e156, _e157);
            vec4 _e161 = f.c;
            int _e164 = i_5;
            vec4 _e166 = _group_0_binding_0_fs.shape_c[_e164];
            float _e167 = h_1;
            vec4 _e168 = towards(_e161, _e166, _e167);
            f.c = _e168;
            vec4 _e171 = f.face;
            int _e174 = i_5;
            vec4 _e176 = _group_0_binding_0_fs.shape_face[_e174];
            float _e177 = h_1;
            vec4 _e178 = towards(_e171, _e176, _e177);
            f.face = _e178;
            vec4 _e181 = f.fill;
            int _e184 = i_5;
            vec4 _e186 = _group_0_binding_0_fs.shape_fill[_e184];
            float _e187 = h_1;
            vec4 _e188 = towards(_e181, _e186, _e187);
            f.fill = _e188;
            vec4 _e191 = f.tint;
            int _e194 = i_5;
            vec4 _e196 = _group_0_binding_0_fs.shape_tint[_e194];
            float _e197 = h_1;
            vec4 _e198 = towards(_e191, _e196, _e197);
            f.tint = _e198;
            vec4 _e201 = f.rim;
            int _e204 = i_5;
            vec4 _e206 = _group_0_binding_0_fs.shape_rim[_e204];
            float _e207 = h_1;
            vec4 _e208 = towards(_e201, _e206, _e207);
            f.rim = _e208;
            vec4 _e211 = f.spec;
            int _e214 = i_5;
            vec4 _e216 = _group_0_binding_0_fs.shape_spec[_e214];
            float _e217 = h_1;
            vec4 _e218 = towards(_e211, _e216, _e217);
            f.spec = _e218;
            vec4 _e221 = f.spec2_;
            int _e224 = i_5;
            vec4 _e226 = _group_0_binding_0_fs.shape_spec2_[_e224];
            float _e227 = h_1;
            vec4 _e228 = towards(_e221, _e226, _e227);
            f.spec2_ = _e228;
            vec4 _e231 = f.ring;
            int _e234 = i_5;
            vec4 _e236 = _group_0_binding_0_fs.shape_ring[_e234];
            float _e237 = h_1;
            vec4 _e238 = towards(_e231, _e236, _e237);
            f.ring = _e238;
            vec4 _e241 = f.border;
            int _e244 = i_5;
            vec4 _e246 = _group_0_binding_0_fs.shape_border[_e244];
            float _e247 = h_1;
            vec4 _e248 = towards(_e241, _e246, _e247);
            f.border = _e248;
            float _e250 = d_2;
            f.d = _e250;
        }
    }
    bool loop_init_4 = true;
    while(true) {
        if (!loop_init_4) {
            int _e403 = i_6;
            i_6 = (_e403 + 1);
        }
        loop_init_4 = false;
        int _e256 = i_6;
        float _e261 = _group_0_binding_0_fs.uv_y.w;
        if ((_e256 < ((_e261 > 0.5) ? n_9 : 0))) {
        } else {
            break;
        }
        {
            int _e268 = i_6;
            vec4 _e270 = _group_0_binding_0_fs.shapes[_e268];
            int _e273 = i_6;
            float _e276 = _group_0_binding_0_fs.shape_a[_e273].x;
            int _e279 = i_6;
            float _e282 = _group_0_binding_0_fs.shape_b[_e279].w;
            float _e283 = rect_sdf(p_7, _e270, _e276, _e282);
            float _e287 = _group_0_binding_0_fs.info.z;
            float cov = clamp((0.5 - (_e283 / _e287)), 0.0, 1.0);
            if ((cov > 0.0)) {
                float _e300 = f.a.w;
                int _e303 = i_6;
                float _e306 = _group_0_binding_0_fs.shape_a[_e303].w;
                float _e309 = f.a.w;
                f.a.w = (_e300 + ((_e306 - _e309) * cov));
                float _e317 = f.c.w;
                int _e320 = i_6;
                float _e323 = _group_0_binding_0_fs.shape_c[_e320].w;
                float _e326 = f.c.w;
                f.c.w = (_e317 + ((_e323 - _e326) * cov));
                vec4 _e332 = f.face;
                int _e335 = i_6;
                vec4 _e337 = _group_0_binding_0_fs.shape_face[_e335];
                vec4 _e338 = towards(_e332, _e337, cov);
                f.face = _e338;
                vec4 _e341 = f.fill;
                int _e344 = i_6;
                vec4 _e346 = _group_0_binding_0_fs.shape_fill[_e344];
                vec4 _e347 = towards(_e341, _e346, cov);
                f.fill = _e347;
                vec4 _e350 = f.tint;
                int _e353 = i_6;
                vec4 _e355 = _group_0_binding_0_fs.shape_tint[_e353];
                vec4 _e356 = towards(_e350, _e355, cov);
                f.tint = _e356;
                vec4 _e359 = f.rim;
                int _e362 = i_6;
                vec4 _e364 = _group_0_binding_0_fs.shape_rim[_e362];
                vec4 _e365 = towards(_e359, _e364, cov);
                f.rim = _e365;
                vec4 _e368 = f.spec;
                int _e371 = i_6;
                vec4 _e373 = _group_0_binding_0_fs.shape_spec[_e371];
                vec4 _e374 = towards(_e368, _e373, cov);
                f.spec = _e374;
                vec4 _e377 = f.spec2_;
                int _e380 = i_6;
                vec4 _e382 = _group_0_binding_0_fs.shape_spec2_[_e380];
                vec4 _e383 = towards(_e377, _e382, cov);
                f.spec2_ = _e383;
                vec4 _e386 = f.ring;
                int _e389 = i_6;
                vec4 _e391 = _group_0_binding_0_fs.shape_ring[_e389];
                vec4 _e392 = towards(_e386, _e391, cov);
                f.ring = _e392;
                vec4 _e395 = f.border;
                int _e398 = i_6;
                vec4 _e400 = _group_0_binding_0_fs.shape_border[_e398];
                vec4 _e401 = towards(_e395, _e400, cov);
                f.border = _e401;
            }
        }
    }
    Field _e405 = f;
    return _e405;
}

float field_d(vec2 p_8) {
    float fd = 0.0;
    int i_7 = 1;
    float _e4 = _group_0_binding_0_fs.info.x;
    int n_10 = int(_e4);
    float k_4 = _group_0_binding_0_fs.info.y;
    vec4 _e13 = _group_0_binding_0_fs.shapes[0];
    float _e18 = _group_0_binding_0_fs.shape_a[0].x;
    float _e23 = _group_0_binding_0_fs.shape_b[0].w;
    float _e24 = rect_sdf(p_8, _e13, _e18, _e23);
    fd = _e24;
    bool loop_init_5 = true;
    while(true) {
        if (!loop_init_5) {
            int _e70 = i_7;
            i_7 = (_e70 + 1);
        }
        loop_init_5 = false;
        int _e28 = i_7;
        if ((_e28 < n_10)) {
        } else {
            break;
        }
        {
            int _e32 = i_7;
            vec4 _e34 = _group_0_binding_0_fs.shapes[_e32];
            int _e37 = i_7;
            float _e40 = _group_0_binding_0_fs.shape_a[_e37].x;
            int _e43 = i_7;
            float _e46 = _group_0_binding_0_fs.shape_b[_e43].w;
            float _e47 = rect_sdf(p_8, _e34, _e40, _e46);
            if ((k_4 <= 0.0)) {
                float _e50 = fd;
                fd = min(_e47, _e50);
            } else {
                float _e52 = fd;
                float h_2 = clamp((0.5 + ((0.5 * (_e52 - _e47)) / k_4)), 0.0, 1.0);
                float _e62 = fd;
                fd = (mix(_e62, _e47, h_2) - ((k_4 * h_2) * (1.0 - h_2)));
            }
        }
    }
    float _e72 = fd;
    return _e72;
}

vec2 uv_of(vec2 p_9) {
    vec3 h_3 = vec3(p_9, 1.0);
    vec4 _e5 = _group_0_binding_0_fs.uv_x;
    vec4 _e10 = _group_0_binding_0_fs.uv_y;
    return vec2(dot(_e5.xyz, h_3), dot(_e10.xyz, h_3));
}

vec3 saturate_encoded(vec3 c_3, float s) {
    float y_4 = dot(c_3, LUMA);
    return max((vec3(y_4) + ((c_3 - vec3(y_4)) * s)), vec3(0.0));
}

vec3 tap(highp sampler2D t_3, vec2 uv) {
    vec4 _e4 = textureLod(t_3, vec2(uv), 0.0);
    return _e4.xyz;
}

vec3 blurred(vec2 uv_1, float pos) {
    vec3 c_4 = vec3(0.0);
    vec4 w_3 = max((vec4(1.0) - abs((vec4(clamp(pos, 0.0, 3.0)) - vec4(0.0, 1.0, 2.0, 3.0)))), vec4(0.0));
    if ((w_3.x > 0.0)) {
        vec3 _e27 = tap(_group_0_binding_1_fs, uv_1);
        vec3 _e29 = c_4;
        c_4 = (_e29 + (w_3.x * _e27));
    }
    if ((w_3.y > 0.0)) {
        vec3 _e36 = tap(_group_0_binding_2_fs, uv_1);
        vec3 _e38 = c_4;
        c_4 = (_e38 + (w_3.y * _e36));
    }
    if ((w_3.z > 0.0)) {
        vec3 _e45 = tap(_group_0_binding_3_fs, uv_1);
        vec3 _e47 = c_4;
        c_4 = (_e47 + (w_3.z * _e45));
    }
    if ((w_3.w > 0.0)) {
        vec3 _e54 = tap(_group_0_binding_4_fs, uv_1);
        vec3 _e56 = c_4;
        c_4 = (_e56 + (w_3.w * _e54));
    }
    vec3 _e58 = c_4;
    return _e58;
}

vec3 face_at(vec2 q_1, float pos_1, float max_luma, float w_1, vec4 face) {
    vec3 c_5 = vec3(0.0);
    vec3 s_1 = vec3(0.0);
    float l = 0.0;
    vec2 _e5 = uv_of(q_1);
    vec3 _e7 = blurred(_e5, (pos_1 * w_1));
    c_5 = _e7;
    vec3 _e10 = tap(_group_0_binding_5_fs, _e5);
    vec3 _e11 = c_5;
    vec3 _e12 = c_5;
    float _e17 = _group_0_binding_0_fs.fill.x;
    c_5 = mix(_e11, min(_e12, _e10), (_e17 * w_1));
    vec3 _e20 = c_5;
    vec3 _e21 = c_5;
    float _e26 = _group_0_binding_0_fs.fill.y;
    c_5 = mix(_e20, max(_e21, _e10), (_e26 * w_1));
    vec3 _e29 = c_5;
    float _e33 = _group_0_binding_0_fs.fill.z;
    c_5 = mix(_e29, _e10, (_e33 * w_1));
    vec3 _e36 = c_5;
    vec3 _e38 = saturate_encoded(_e36, face.x);
    s_1 = _e38;
    vec3 _e40 = s_1;
    l = dot(_e40, LUMA);
    vec3 _e44 = s_1;
    float _e47 = l;
    vec3 _e49 = s_1;
    float _e52 = l;
    s_1 = (_e44 - (((1.0 - max_luma) * _e47) * ((0.75 * _e49) + vec3((0.25 * _e52)))));
    vec3 _e59 = s_1;
    l = dot(_e59, LUMA);
    vec3 _e62 = s_1;
    float _e64 = l;
    float _e71 = l;
    s_1 = (_e62 + vec3(((face.z * (1.0 - _e64)) - ((1.0 - face.y) * _e71))));
    vec3 _e76 = s_1;
    return clamp(_e76, vec3(0.0), vec3(min(face.w, 1.0)));
}

vec3 bleed_tone(vec3 c_6) {
    float y_5 = dot(c_6, LUMA);
    float _e9 = _group_0_binding_0_fs.bleed.z;
    vec3 s_3 = (vec3(y_5) + ((c_6 - vec3(y_5)) * _e9));
    float l_1 = dot(s_3, LUMA);
    float _e17 = _group_0_binding_0_fs.bleed.y;
    float _e24 = _group_0_binding_0_fs.bleed.x;
    return clamp((s_3 + vec3(((_e17 * (1.0 - l_1)) - ((1.0 - _e24) * l_1)))), vec3(0.0), vec3(1.0));
}

float angle_between(float a_1, float b) {
    float d_13 = ((a_1 - b) - (floor(((a_1 - b) / 6.2831855)) * 6.2831855));
    return min(d_13, (6.2831855 - d_13));
}

float lobe(float normal_angle, float light, float spread_scale) {
    float _e6 = _group_0_binding_0_fs.light.y;
    float reach = ((_e6 * spread_scale) * LOBE_REACH);
    if ((reach <= 0.0)) {
        return 0.0;
    }
    float _e13 = angle_between(normal_angle, light);
    if ((_e13 >= reach)) {
        return 0.0;
    }
    float _e27 = _group_0_binding_0_fs.light.z;
    return pow((0.5 + (0.5 * cos(((_e13 / reach) * PI)))), max(_e27, 0.001));
}

float light_arc(float normal_angle_1, float light_1, float spread) {
    if ((spread <= 0.0)) {
        return 0.0;
    }
    float _e7 = angle_between(normal_angle_1, light_1);
    return (1.0 - smoothstep(0.0, spread, _e7));
}

float key_and_fill(float na, vec4 spec, float fill, float spread_1) {
    float _e5 = light_arc(na, spec.w, spread_1);
    float _e9 = light_arc(na, (spec.w + PI), spread_1);
    return (_e5 + (fill * _e9));
}

float specular(float depth, float na_1, vec4 spec_1, vec4 spec2_) {
    float band = 0.0;
    float glow = 0.0;
    if ((spec_1.z > 0.0)) {
        band = (1.0 - smoothstep(0.0, spec_1.z, depth));
    }
    float _e16 = key_and_fill(na_1, spec_1, spec2_.x, spec_1.y);
    float dh = (spec_1.z * spec2_.y);
    if (((dh > 0.0) && (depth < dh))) {
        float td = (1.0 - (depth / dh));
        float _e34 = key_and_fill(na_1, spec_1, spec2_.x, (spec_1.y * spec2_.z));
        glow = (((spec2_.w * _e34) * td) * td);
    }
    float _e39 = band;
    float _e41 = glow;
    return (spec_1.x * ((_e16 * _e39) + _e41));
}

float border_line(float d_3, vec2 n_3, Field f_1) {
    float line = 0.0;
    if (((f_1.border.x <= 0.0) || (f_1.border.y <= 0.0))) {
        return 0.0;
    }
    line = (f_1.border.x * clamp((((0.5 * f_1.border.y) + 0.5) - abs(d_3)), 0.0, 1.0));
    float _e28 = line;
    if ((((_e28 > 0.0) && (f_1.spec.x > 0.0)) && (dot(n_3, n_3) > 0.0))) {
        float _e49 = key_and_fill(atan(-(n_3.y), n_3.x), f_1.spec, f_1.spec2_.x, f_1.spec.y);
        float _e50 = line;
        line = (_e50 * (1.0 - (0.8 * min(1.0, _e49))));
    }
    float _e58 = line;
    return _e58;
}

vec3 key_color(vec3 c_7) {
    vec4 h_4 = vec4(c_7, 1.0);
    vec4 _e6 = _group_0_binding_0_fs.hm[0];
    vec4 _e11 = _group_0_binding_0_fs.hm[1];
    vec4 _e16 = _group_0_binding_0_fs.hm[2];
    return clamp(vec3(dot(_e6, h_4), dot(_e11, h_4), dot(_e16, h_4)), vec3(0.0), vec3(1.0));
}

float shadow_coverage(float d_4, float r_1) {
    if ((r_1 <= 0.0)) {
        return ((d_4 <= 0.0) ? 1.0 : 0.0);
    }
    float t_4 = clamp((0.5 - (d_4 / (3.0 * r_1))), 0.0, 1.0);
    return ((t_4 * t_4) * (3.0 - (2.0 * t_4)));
}

float rim_brightness(float l0_) {
    float l_2 = clamp(l0_, 0.0, 1.0);
    if ((l_2 < 0.25)) {
        return ((l_2 * 4.0) * 0.63);
    }
    if ((l_2 < 0.5)) {
        return mix(0.63, 1.0, ((l_2 - 0.25) * 4.0));
    }
    if ((l_2 < 0.75)) {
        return mix(1.0, 1.13, ((l_2 - 0.5) * 4.0));
    }
    return mix(1.13, 1.0, ((l_2 - 0.75) * 4.0));
}

float rim_direction(vec2 n_4) {
    float s_4 = ((2.0 * n_4.x) * n_4.y);
    return (0.15 + ((0.85 * s_4) * s_4));
}

float normal_cdf(float x_4) {
    return (1.0 / (1.0 + exp((-1.702 * x_4))));
}

float outside_alpha(vec2 p_10, float d_5) {
    float drop = 0.0;
    float line_1 = 0.0;
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
    float _e37 = _group_0_binding_0_fs.shadow_color.w;
    if (((_e31 > 0.0) && (_e37 > 0.0))) {
        float _e44 = _group_0_binding_0_fs.glow.w;
        float _e48 = _group_0_binding_0_fs.shadow_color.w;
        line_1 = (_e44 * clamp(((_e48 + 0.5) - d_5), 0.0, 1.0));
    }
    float _e56 = drop;
    float _e57 = line_1;
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

float edge_line_side_share(vec2 n_5) {
    float _e4 = _group_0_binding_0_fs.diffuse.w;
    float reach_1 = (_e4 * LOBE_REACH);
    if (((reach_1 <= 0.0) || (dot(n_5, n_5) < 1e-12))) {
        return 1.0;
    }
    float a_5 = acos(clamp((abs(n_5.x) / length(n_5)), 0.0, 1.0));
    if ((a_5 >= reach_1)) {
        return 0.0;
    }
    return (0.5 + (0.5 * cos(((a_5 / reach_1) * PI))));
}

float edge_line(vec2 p_12, float d_6, float pos_2, vec2 n_6) {
    float response = 1.0;
    float band_1 = clamp((1.5 - d_6), 0.0, 1.0);
    float _e12 = _group_0_binding_0_fs.levels.x;
    if (((_e12 <= 0.0) || (band_1 <= 0.0))) {
        return 0.0;
    }
    float _e19 = edge_line_side_share(n_6);
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
    return (((min(_e39, 1.0) * _e42) * band_1) * _e19);
}

vec4 outer_refraction(vec2 p_13, float d_7, vec2 n_7, Field f_2) {
    if ((((f_2.c.w <= 0.0) || (f_2.c.z <= 0.0)) || (f_2.c.y == 0.0))) {
        return vec4(0.0);
    }
    float t_5 = (max(d_7, 0.0) / f_2.c.z);
    if ((t_5 >= 1.0)) {
        return vec4(0.0);
    }
    float _e33 = refraction_profile(t_5);
    vec2 _e36 = uv_of((p_13 + ((n_7 * f_2.c.y) * _e33)));
    vec3 _e37 = tap(_group_0_binding_7_fs, _e36);
    return vec4(_e37, (min(f_2.c.w, 1.0) * (1.0 - smoothstep(0.0, 1.0, t_5))));
}

vec4 outside(vec2 p_14, float d_8, float pos_3, vec2 n_8, Field f_3) {
    float line_2 = 0.0;
    vec4 o_1 = vec4(0.0);
    float _e5 = outside_alpha(p_14, d_8);
    float _e6 = edge_line(p_14, d_8, pos_3, n_8);
    line_2 = _e6;
    float _e8 = border_line(d_8, n_8, f_3);
    if ((_e8 > 0.0)) {
        float _e11 = line_2;
        float _e13 = line_2;
        line_2 = ((_e11 + _e8) - (_e13 * _e8));
    }
    float _e16 = line_2;
    float _e17 = line_2;
    float a_6 = (_e16 + (_e5 * (1.0 - _e17)));
    vec4 _e24 = _group_0_binding_0_fs.shadow_color;
    o_1 = vec4(_e24.xyz, 0.0);
    if ((a_6 > 0.0)) {
        vec4 _e33 = _group_0_binding_0_fs.shadow_color;
        float _e35 = line_2;
        o_1 = vec4((_e33.xyz * ((_e5 * (1.0 - _e35)) / a_6)), a_6);
    }
    vec4 _e42 = outer_refraction(p_14, d_8, n_8, f_3);
    if ((_e42.w > 0.0)) {
        float _e47 = o_1.w;
        float _e50 = o_1.w;
        float ao = (_e47 + (_e42.w * (1.0 - _e50)));
        vec4 _e55 = o_1;
        float _e58 = o_1.w;
        float _e64 = o_1.w;
        o_1 = vec4((((_e55.xyz * _e58) + ((_e42.xyz * _e42.w) * (1.0 - _e64))) / vec3(ao)), ao);
    }
    vec4 _e72 = o_1;
    return _e72;
}

float gaussian_cdf(float x_5) {
    float z = (abs(x_5) * 0.70710677);
    float t_6 = (1.0 / (1.0 + (0.3275911 * z)));
    float poly = (t_6 * (0.2548296 + (t_6 * (-0.28449675 + (t_6 * (1.4214137 + (t_6 * (-1.453152 + (t_6 * 1.0614054)))))))));
    float erf = (1.0 - (poly * exp((-(z) * z))));
    return (0.5 * (1.0 + ((x_5 >= 0.0) ? erf : -(erf))));
}

float gaussian_ramp(float u) {
    float _e1 = gaussian_cdf(u);
    return ((u * _e1) + (0.3989423 * exp(((-0.5 * u) * u))));
}

float edge_share(float d_9, float w_2, float s_2) {
    if ((s_2 <= 0.0)) {
        return clamp((min((d_9 + 0.5), w_2) - max((d_9 - 0.5), 0.0)), 0.0, 1.0);
    }
    float _e19 = gaussian_ramp(((d_9 + 0.5) / s_2));
    float _e23 = gaussian_ramp(((d_9 - 0.5) / s_2));
    float _e29 = gaussian_ramp((((d_9 + 0.5) - w_2) / s_2));
    float _e35 = gaussian_ramp((((d_9 - 0.5) - w_2) / s_2));
    float r_2 = (s_2 * (((_e19 - _e23) - _e29) + _e35));
    return clamp(r_2, 0.0, 1.0);
}

float edge_alpha(vec2 p_15) {
    float d_10[16] = float[16](0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    bool near = false;
    int i_8 = 0;
    float keep = 1.0;
    int i_9 = 0;
    float outside_1 = 0.0;
    int j_1 = 0;
    float w_4 = _group_0_binding_0_fs.lens.w;
    float _e10 = _group_0_binding_0_fs.edge.w;
    if (((w_4 <= 0.0) || (_e10 <= 0.0))) {
        return 0.0;
    }
    float s_5 = _group_0_binding_0_fs.uv_x.w;
    float reach_2 = (0.5 + (3.0 * max(s_5, 0.0)));
    float _e28 = _group_0_binding_0_fs.info.x;
    int n_11 = int(_e28);
    bool loop_init_6 = true;
    while(true) {
        if (!loop_init_6) {
            int _e71 = i_8;
            i_8 = (_e71 + 1);
        }
        loop_init_6 = false;
        int _e35 = i_8;
        if ((_e35 < n_11)) {
        } else {
            break;
        }
        {
            int _e37 = i_8;
            int _e41 = i_8;
            vec4 _e43 = _group_0_binding_0_fs.shapes[_e41];
            int _e46 = i_8;
            float _e49 = _group_0_binding_0_fs.shape_a[_e46].x;
            int _e52 = i_8;
            float _e55 = _group_0_binding_0_fs.shape_b[_e52].w;
            float _e56 = rect_sdf(p_15, _e43, _e49, _e55);
            d_10[_e37] = _e56;
            bool _e57 = near;
            int _e58 = i_8;
            float _e60 = d_10[_e58];
            int _e63 = i_8;
            float _e65 = d_10[_e63];
            near = (_e57 || ((_e60 > -(reach_2)) && (_e65 < (w_4 + reach_2))));
        }
    }
    bool _e73 = near;
    if (!(_e73)) {
        return 0.0;
    }
    bool loop_init_7 = true;
    while(true) {
        if (!loop_init_7) {
            int _e124 = i_9;
            i_9 = (_e124 + 1);
        }
        loop_init_7 = false;
        int _e80 = i_9;
        if ((_e80 < n_11)) {
        } else {
            break;
        }
        {
            int _e82 = i_9;
            float _e84 = d_10[_e82];
            float _e85 = edge_share(_e84, w_4, s_5);
            if ((_e85 <= 0.0)) {
                continue;
            }
            outside_1 = 1.0;
            j_1 = 0;
            bool loop_init_8 = true;
            while(true) {
                if (!loop_init_8) {
                    int _e110 = j_1;
                    j_1 = (_e110 + 1);
                }
                loop_init_8 = false;
                int _e92 = j_1;
                if ((_e92 < n_11)) {
                } else {
                    break;
                }
                {
                    int _e94 = j_1;
                    int _e95 = i_9;
                    if ((_e94 != _e95)) {
                        int _e97 = j_1;
                        float _e99 = d_10[_e97];
                        float _e107 = outside_1;
                        outside_1 = (_e107 * (1.0 - clamp((0.5 - _e99), 0.0, 1.0)));
                    }
                }
            }
            float _e115 = _group_0_binding_0_fs.edge.w;
            float _e117 = outside_1;
            float _e121 = keep;
            keep = (_e121 * (1.0 - ((_e115 * _e85) * _e117)));
        }
    }
    float _e126 = keep;
    return clamp((1.0 - _e126), 0.0, 1.0);
}

vec4 with_edge(vec4 c_8, vec2 p_16) {
    float _e2 = edge_alpha(p_16);
    if ((_e2 <= 0.0)) {
        return c_8;
    }
    float a_7 = ((c_8.w + _e2) - (c_8.w * _e2));
    vec4 _e18 = _group_0_binding_0_fs.edge;
    return vec4(((((c_8.xyz * c_8.w) * (1.0 - _e2)) + (_e18.xyz * _e2)) / vec3(a_7)), a_7);
}

float cut_coverage(vec2 p_17) {
    float _e4 = _group_0_binding_0_fs.cut_shape.z;
    if ((_e4 <= 0.0)) {
        return 1.0;
    }
    vec4 _e10 = _group_0_binding_0_fs.cut;
    float _e14 = _group_0_binding_0_fs.cut_shape.x;
    float _e18 = _group_0_binding_0_fs.cut_shape.y;
    float _e19 = rect_sdf(p_17, _e10, _e14, _e18);
    float _e23 = _group_0_binding_0_fs.info.z;
    return clamp((0.5 - (_e19 / _e23)), 0.0, 1.0);
}

bool outside_needs_normal(float d_11, Field f_4) {
    float _e5 = _group_0_binding_0_fs.diffuse.w;
    float _e11 = _group_0_binding_0_fs.levels.x;
    bool side_line = (((_e5 > 0.0) && (_e11 > 0.0)) && (d_11 < 1.5));
    bool border = (((f_4.border.x > 0.0) && (f_4.border.y > 0.0)) && (d_11 < ((0.5 * f_4.border.y) + 0.5)));
    bool bent = ((((f_4.c.w > 0.0) && (f_4.c.z > 0.0)) && (f_4.c.y != 0.0)) && (d_11 < f_4.c.z));
    return ((side_line || border) || bent);
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
    vec2 p_19 = fin.px;
    float _e2 = cut_coverage(p_19);
    if ((_e2 <= 0.0)) {
        discard;
    }
    Field _e5 = field(p_19);
    float d_14 = _e5.d;
    if ((d_14 > 0.5)) {
        bool _e12 = outside_needs_normal(d_14, _e5);
        if (_e12) {
            vec2 _e13 = field_normal(p_19);
            on = _e13;
        }
        vec2 _e16 = on;
        vec4 _e17 = outside(p_19, d_14, _e5.b.z, _e16, _e5);
        vec4 _e18 = with_edge(_e17, p_19);
        float _e23 = _group_0_binding_0_fs.info.w;
        float a_8 = ((_e18.w * _e23) * _e2);
        if ((a_8 <= 0.0)) {
            discard;
        }
        vec4 _e32 = placed(vec4(_e18.xyz, a_8), fin.pos.xy);
        _fs2p_location0 = _e32;
        return;
    }
    float _e37 = field_d((p_19 + vec2(0.5, 0.0)));
    float _e42 = field_d((p_19 - vec2(0.5, 0.0)));
    float _e48 = field_d((p_19 + vec2(0.0, 0.5)));
    float _e53 = field_d((p_19 - vec2(0.0, 0.5)));
    vec2 g_3 = vec2((_e37 - _e42), (_e48 - _e53));
    float grad_len_1 = length(g_3);
    if ((grad_len_1 > 1e-6)) {
        n = (g_3 / vec2(grad_len_1));
    }
    if (((_e5.a.z > 0.0) && (d_14 <= 0.0))) {
        vec2 _e74 = n;
        float _e84 = refraction_profile(max((-(d_14) / _e5.a.z), 0.0));
        off = ((_e74 * _e5.a.y) * _e84);
    }
    vec2 _e86 = off;
    q = (p_19 + _e86);
    float _e92 = _group_0_binding_0_fs.lens.z;
    if ((_e92 > 1.0)) {
        vec4 _e98 = _group_0_binding_0_fs.shapes[0];
        vec4 _e103 = _group_0_binding_0_fs.shapes[0];
        vec2 lc = (_e98.xy + (0.5 * _e103.zw));
        vec2 _e108 = q;
        float _e113 = _group_0_binding_0_fs.lens.z;
        q = (lc + ((_e108 - lc) / vec2(_e113)));
    }
    float _e122 = _group_0_binding_0_fs.lens.x;
    float _e128 = _group_0_binding_0_fs.lens.y;
    if (((_e122 < 1.0) || (_e128 < 1.0))) {
        float _e137 = _group_0_binding_0_fs.shapes[0].y;
        float _e143 = _group_0_binding_0_fs.shapes[0].w;
        float rt = clamp(((p_19.y - _e137) / max(_e143, 0.001)), 0.0, 1.0);
        float _e153 = _group_0_binding_0_fs.lens.x;
        float _e157 = _group_0_binding_0_fs.lens.y;
        blur_weight = mix(_e153, _e157, smoothstep(0.0, 1.0, rt));
    }
    vec2 _e162 = q;
    float _e167 = blur_weight;
    vec3 _e169 = face_at(_e162, _e5.b.z, _e5.b.y, _e167, _e5.face);
    c = _e169;
    if ((((_e5.a.w > 0.0) && (_e5.b.x > 0.0)) && (-(d_14) < _e5.b.x))) {
        float tb = (max(-(d_14), 0.0) / _e5.b.x);
        vec2 _e192 = n;
        vec2 _e200 = uv_of((p_19 + ((_e192 * _e5.c.x) * (1.0 - tb))));
        vec3 _e201 = tap(_group_0_binding_6_fs, _e200);
        vec3 _e202 = bleed_tone(_e201);
        float wb = ((min(_e5.a.w, 1.0) * (1.0 - tb)) * (1.0 - tb));
        vec3 _e213 = c;
        vec3 _e214 = c;
        float _e219 = _group_0_binding_0_fs.bleed.w;
        c = mix(_e213, ((_e219 > 0.5) ? min(_e214, _e202) : _e202), wb);
    }
    vec3 _e224 = c;
    c = mix(_e224, _e5.fill.xyz, _e5.fill.w);
    if ((_e5.tint.w > 0.0)) {
        vec3 _e234 = c;
        c = mix(_e234, _e5.tint.xyz, _e5.tint.w);
    }
    float de = (d_14 / max(grad_len_1, MIN_FIELD_SLOPE));
    bool rim_on = (((_e5.rim.x * _e5.rim.y) != 0.0) && (_e5.rim.z > 0.0));
    float _e258 = _group_0_binding_0_fs.glow.x;
    float _e264 = _group_0_binding_0_fs.glow.y;
    bool glow_on = ((_e258 > 0.0) && (_e264 > 0.0));
    if (((rim_on || glow_on) && (grad_len_1 > 1e-6))) {
        if (glow_on) {
            float _e273 = n.y;
            float _e276 = n.x;
            float na_2 = atan(-(_e273), _e276);
            float _e281 = _group_0_binding_0_fs.light.w;
            float fill_1 = max(_e281, 0.0);
            float _e287 = _group_0_binding_0_fs.light.x;
            float _e289 = lobe(na_2, _e287, 1.0);
            float _e293 = _group_0_binding_0_fs.light.x;
            float _e297 = lobe(na_2, (_e293 + PI), 1.0);
            float ends = (_e289 + (_e297 * fill_1));
            float _e303 = _group_0_binding_0_fs.glow.z;
            float _e308 = _group_0_binding_0_fs.glow.y;
            float xg = ((de - _e303) / _e308);
            float _e313 = _group_0_binding_0_fs.glow.x;
            w = ((_e313 * exp((-(xg) * xg))) * ends);
            float _e323 = _group_0_binding_0_fs.diffuse.z;
            float _e329 = _group_0_binding_0_fs.diffuse.x;
            if (((_e323 > 0.0) && (_e329 > 0.0))) {
                float _e336 = _group_0_binding_0_fs.glow.z;
                float _e341 = _group_0_binding_0_fs.glow.y;
                float _e345 = _group_0_binding_0_fs.diffuse.x;
                float xd = ((de - _e336) / (_e341 * _e345));
                float _e351 = _group_0_binding_0_fs.light.x;
                float _e355 = _group_0_binding_0_fs.diffuse.y;
                float _e356 = lobe(na_2, _e351, _e355);
                float _e360 = _group_0_binding_0_fs.light.x;
                float _e366 = _group_0_binding_0_fs.diffuse.y;
                float _e367 = lobe(na_2, (_e360 + PI), _e366);
                float wide = (_e356 + (_e367 * fill_1));
                float _e370 = w;
                float _e374 = _group_0_binding_0_fs.glow.x;
                float _e378 = _group_0_binding_0_fs.diffuse.z;
                w = (_e370 + (((_e374 * _e378) * exp((-(xd) * xd))) * wide));
            }
            vec3 _e386 = c;
            vec3 _e387 = c;
            vec3 _e388 = key_color(_e387);
            float _e389 = w;
            c = min((_e386 + (_e388 * _e389)), vec3(1.0));
        }
        if (rim_on) {
            float band_2 = clamp(((_e5.rim.z + 0.5) - max(-(de), 0.0)), 0.0, 1.0);
            vec2 _e411 = n;
            float _e412 = rim_direction(_e411);
            float k_5 = (((_e5.rim.x * _e5.rim.y) * _e412) * band_2);
            float _e416 = c.x;
            float _e417 = rim_brightness(_e416);
            float _e419 = c.y;
            float _e420 = rim_brightness(_e419);
            float _e422 = c.z;
            float _e423 = rim_brightness(_e422);
            vec3 shift = (vec3(_e417, _e420, _e423) * k_5);
            vec3 _e426 = c;
            c = clamp((_e426 + shift), vec3(0.0), vec3(1.0));
        }
    }
    vec2 _e433 = n;
    float _e434 = border_line(d_14, _e433, _e5);
    if ((_e434 > 0.0)) {
        vec3 _e437 = c;
        c = (_e437 * (1.0 - _e434));
    }
    if (((_e5.spec.x > 0.0) && (grad_len_1 > 1e-6))) {
        float _e449 = n.y;
        float _e452 = n.x;
        float na_3 = atan(-(_e449), _e452);
        vec3 _e454 = c;
        float _e460 = specular(max(-(de), 0.0), na_3, _e5.spec, _e5.spec2_);
        c = clamp((_e454 + vec3(_e460)), vec3(0.0), vec3(1.0));
    }
    float _e471 = _group_0_binding_0_fs.fill.w;
    float _e477 = _group_0_binding_0_fs.levels.y;
    if (((_e471 > 0.0) || (_e477 > 0.0))) {
        float _e484 = _group_0_binding_0_fs.info.x;
        int vn = int(_e484);
        float _e490 = _group_0_binding_0_fs.shapes[0].y;
        top = _e490;
        float _e496 = _group_0_binding_0_fs.shapes[0].y;
        float _e501 = _group_0_binding_0_fs.shapes[0].w;
        bottom = (_e496 + _e501);
        bool loop_init_9 = true;
        while(true) {
            if (!loop_init_9) {
                int _e532 = i_1;
                i_1 = (_e532 + 1);
            }
            loop_init_9 = false;
            int _e506 = i_1;
            if ((_e506 < vn)) {
            } else {
                break;
            }
            {
                float _e508 = top;
                int _e511 = i_1;
                float _e514 = _group_0_binding_0_fs.shapes[_e511].y;
                top = min(_e508, _e514);
                float _e516 = bottom;
                int _e519 = i_1;
                float _e522 = _group_0_binding_0_fs.shapes[_e519].y;
                int _e525 = i_1;
                float _e528 = _group_0_binding_0_fs.shapes[_e525].w;
                bottom = max(_e516, (_e522 + _e528));
            }
        }
        float _e535 = top;
        float _e537 = bottom;
        float _e538 = top;
        float vt = clamp(((p_19.y - _e535) / max((_e537 - _e538), 0.001)), 0.0, 1.0);
        vec3 _e546 = c;
        float _e550 = _group_0_binding_0_fs.fill.w;
        float _e554 = _group_0_binding_0_fs.levels.y;
        c = (_e546 * (1.0 - clamp(mix(_e550, _e554, vt), 0.0, 1.0)));
    }
    if (((_e5.ring.w > 0.0) && (_e5.ring.y > 0.0))) {
        float _e576 = field_d((p_19 - vec2(0.0, _e5.ring.x)));
        if ((_e5.ring.z > 0.0)) {
            float _e587 = normal_cdf((-(_e576) / _e5.ring.z));
            float _e595 = normal_cdf(((-(_e5.ring.y) - _e576) / _e5.ring.z));
            m = (_e587 - _e595);
        } else {
            if (((_e576 <= 0.0) && (_e576 >= -(_e5.ring.y)))) {
                m = 1.0;
            }
        }
        vec3 _e605 = c;
        float _e610 = m;
        c = (_e605 * (1.0 - (min(_e5.ring.w, 1.0) * max(_e610, 0.0))));
    }
    float _e620 = _group_0_binding_0_fs.info.z;
    float cov_1 = clamp((0.5 - (d_14 / _e620)), 0.0, 1.0);
    vec2 _e629 = n;
    vec4 _e630 = outside(p_19, d_14, _e5.b.z, _e629, _e5);
    float under = ((1.0 - cov_1) * _e630.w);
    float a_9 = (cov_1 + under);
    vec3 _e636 = c;
    rgb = _e636;
    if ((a_9 > 0.0)) {
        vec3 _e640 = c;
        rgb = (((clamp(_e640, vec3(0.0), vec3(1.0)) * cov_1) + (_e630.xyz * under)) / vec3(a_9));
    }
    vec3 _e652 = rgb;
    vec4 _e659 = with_edge(vec4(clamp(_e652, vec3(0.0), vec3(1.0)), a_9), p_19);
    float _e670 = _group_0_binding_0_fs.info.w;
    vec4 _e676 = placed(vec4(clamp(_e659.xyz, vec3(0.0), vec3(1.0)), ((_e659.w * _e670) * _e2)), fin.pos.xy);
    _fs2p_location0 = _e676;
    return;
}

