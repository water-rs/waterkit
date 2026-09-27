struct NagaConstants {
    int first_vertex;
    int first_instance;
    uint other;
};
ConstantBuffer<NagaConstants> _NagaConstants: register(b1);

struct VertexOutput {
    float4 position : SV_Position;
    float2 uv : LOC0;
};

struct ColorParams {
    uint matrix_mode;
    uint range_mode;
    uint primaries_mode;
    uint transfer_mode;
    uint target_mode;
    uint sample_mode;
    float max_content_light_nits;
    uint _padding1_;
};

static const uint MATRIX_BT709_ = 0u;
static const uint MATRIX_BT601_ = 1u;
static const uint MATRIX_BT2020_ = 2u;
static const uint MATRIX_BT2020_CONSTANT_LUMINANCE = 3u;
static const uint RANGE_LIMITED = 0u;
static const uint SAMPLE_NV12_ = 0u;
static const uint SAMPLE_P010_ = 1u;
static const uint PRIMARIES_BT709_ = 0u;
static const uint PRIMARIES_BT601_ = 1u;
static const uint PRIMARIES_DISPLAY_P3_ = 2u;
static const uint PRIMARIES_BT2020_ = 3u;
static const uint TRANSFER_SDR = 0u;
static const uint TRANSFER_PQ = 1u;
static const uint TRANSFER_HLG = 2u;
static const uint TARGET_GAMMA_SDR = 0u;
static const uint TARGET_LINEAR_SDR = 1u;
static const uint TARGET_LINEAR_HDR = 2u;
static const float SDR_REFERENCE_WHITE_NITS = 203.0;

Texture2D<float4> y_texture : register(t0);
Texture2D<float4> uv_texture : register(t1);
SamplerState nagaSamplerHeap[2048]: register(s0, space0);
SamplerComparisonState nagaComparisonSamplerHeap[2048]: register(s2048, space0);
StructuredBuffer<uint> nagaGroup0SamplerIndexArray : register(t2, space0);
static const SamplerState video_sampler = nagaSamplerHeap[nagaGroup0SamplerIndexArray[0]];
cbuffer color_params : register(b0) { ColorParams color_params; }
RWTexture2D<float4> linear_rgba_output : register(u0);

struct VertexOutput_vs_main {
    float2 uv_2 : LOC0;
    float4 position_1 : SV_Position;
};

float srgb_to_linear(float c)
{
    if ((c <= 0.04045)) {
        return (c / 12.92);
    }
    return pow(((c + 0.055) / 1.055), 2.4);
}

float linear_to_srgb(float c_1)
{
    if ((c_1 <= 0.0031308)) {
        return (c_1 * 12.92);
    }
    return ((1.055 * pow(c_1, 0.41666666)) - 0.055);
}

float bt709_to_linear(float c_2)
{
    if ((c_2 < 0.081)) {
        return (c_2 / 4.5);
    }
    return pow(((c_2 + 0.099) / 1.099), 2.2222223);
}

float linear_to_bt709_(float c_3)
{
    if ((c_3 < 0.018)) {
        return (c_3 * 4.5);
    }
    return ((1.099 * pow(c_3, 0.45)) - 0.099);
}

float pq_to_linear(float value)
{
    float v_1 = clamp(value, 0.0, 1.0);
    float v_pow = pow(v_1, (1.0 / 78.84375));
    float numerator = max((v_pow - 0.8359375), 0.0);
    float denominator = max((18.851563 - (18.6875 * v_pow)), 1e-6);
    float absolute_nits = (10000.0 * pow((numerator / denominator), (1.0 / 0.15930176)));
    return (absolute_nits / SDR_REFERENCE_WHITE_NITS);
}

float hlg_to_scene_linear(float value_1)
{
    float scene_linear = 0.0;

    float e = clamp(value_1, 0.0, 1.0);
    if ((e <= 0.5)) {
        scene_linear = ((e * e) / 3.0);
    } else {
        scene_linear = ((exp(((e - 0.5599107) / 0.17883277)) + 0.28466892) / 12.0);
    }
    float _e20 = scene_linear;
    return _e20;
}

float3 hlg_scene_to_display_linear(float3 scene_rgb)
{
    float3 safe = max(scene_rgb, (0.0).xxx);
    float scene_luminance = dot(safe, float3(0.2627, 0.678, 0.0593));
    float ootf_gain = pow(max(scene_luminance, 1e-6), (1.2 - 1.0));
    return ((safe * ootf_gain) * 4.9261084);
}

float3 decode_transfer_to_linear(float3 rgb, uint transfer_mode)
{
    if ((transfer_mode == TRANSFER_PQ)) {
        const float _e5 = pq_to_linear(rgb.x);
        const float _e7 = pq_to_linear(rgb.y);
        const float _e9 = pq_to_linear(rgb.z);
        return float3(_e5, _e7, _e9);
    }
    if ((transfer_mode == TRANSFER_HLG)) {
        const float _e14 = hlg_to_scene_linear(rgb.x);
        const float _e16 = hlg_to_scene_linear(rgb.y);
        const float _e18 = hlg_to_scene_linear(rgb.z);
        const float3 _e20 = hlg_scene_to_display_linear(float3(_e14, _e16, _e18));
        return _e20;
    }
    const float _e22 = bt709_to_linear(rgb.x);
    const float _e24 = bt709_to_linear(rgb.y);
    const float _e26 = bt709_to_linear(rgb.z);
    return float3(_e22, _e24, _e26);
}

float decode_transfer_scalar(float value_2, uint transfer_mode_1)
{
    if ((transfer_mode_1 == TRANSFER_PQ)) {
        const float _e4 = pq_to_linear(value_2);
        return _e4;
    }
    if ((transfer_mode_1 == TRANSFER_HLG)) {
        const float _e7 = hlg_to_scene_linear(value_2);
        return _e7;
    }
    const float _e8 = bt709_to_linear(value_2);
    return _e8;
}

float3 convert_primaries_to_srgb(float3 linear_rgb, uint primaries_mode)
{
    if ((primaries_mode == PRIMARIES_BT2020_)) {
        return float3((((1.6605 * linear_rgb.x) - (0.5876 * linear_rgb.y)) - (0.0728 * linear_rgb.z)), (((-0.1246 * linear_rgb.x) + (1.1329 * linear_rgb.y)) - (0.0083 * linear_rgb.z)), (((-0.0182 * linear_rgb.x) - (0.1006 * linear_rgb.y)) + (1.1188 * linear_rgb.z)));
    }
    if ((primaries_mode == PRIMARIES_DISPLAY_P3_)) {
        return float3((((1.2249 * linear_rgb.x) - (0.2247 * linear_rgb.y)) - (0.0002 * linear_rgb.z)), (((-0.042 * linear_rgb.x) + (1.0419 * linear_rgb.y)) + (0.0001 * linear_rgb.z)), (((-0.0197 * linear_rgb.x) - (0.0786 * linear_rgb.y)) + (1.0983 * linear_rgb.z)));
    }
    return linear_rgb;
}

float3 tone_map_hdr_to_sdr(float3 linear_rgb_1)
{
    float3 safe_1 = max(linear_rgb_1, (0.0).xxx);
    float _e6 = color_params.max_content_light_nits;
    float source_peak = max((_e6 / SDR_REFERENCE_WHITE_NITS), 1.0);
    float shoulder = max(((source_peak - 0.75) / 4.0), 0.25);
    float3 compressed = float3((0.75 + ((1.0 - 0.75) * (1.0 - exp((-((safe_1.x - 0.75)) / shoulder))))), (0.75 + ((1.0 - 0.75) * (1.0 - exp((-((safe_1.y - 0.75)) / shoulder))))), (0.75 + ((1.0 - 0.75) * (1.0 - exp((-((safe_1.z - 0.75)) / shoulder))))));
    return float3(((safe_1.x > 0.75) ? compressed.x : safe_1.x), ((safe_1.y > 0.75) ? compressed.y : safe_1.y), ((safe_1.z > 0.75) ? compressed.z : safe_1.z));
}

float3 normalize_yuv(float y_sample, float2 uv_sample)
{
    float y = (float)0;
    float u = (float)0;
    float v = (float)0;

    y = y_sample;
    u = uv_sample.x;
    v = uv_sample.y;
    uint _e9 = color_params.range_mode;
    if ((_e9 == RANGE_LIMITED)) {
        uint _e14 = color_params.sample_mode;
        if ((_e14 == SAMPLE_P010_)) {
            float _e17 = y;
            y = ((_e17 - 0.062561095) * 1.1678082);
            float _e22 = u;
            u = ((_e22 - 0.50048876) * 1.141741);
            float _e27 = v;
            v = ((_e27 - 0.50048876) * 1.141741);
        } else {
            float _e32 = y;
            y = ((_e32 - 0.0627451) * 1.1643835);
            float _e37 = u;
            u = ((_e37 - 0.5019608) * 1.1383928);
            float _e42 = v;
            v = ((_e42 - 0.5019608) * 1.1383928);
        }
    } else {
        uint _e49 = color_params.sample_mode;
        if ((_e49 == SAMPLE_P010_)) {
            float _e52 = u;
            u = (_e52 - 0.50048876);
            float _e55 = v;
            v = (_e55 - 0.50048876);
        } else {
            float _e58 = u;
            u = (_e58 - 0.5019608);
            float _e61 = v;
            v = (_e61 - 0.5019608);
        }
    }
    float _e64 = y;
    float _e65 = u;
    float _e66 = v;
    return float3(_e64, _e65, _e66);
}

float3 yuv_to_gamma_rgb(float3 yuv)
{
    float r = 0.0;
    float g = 0.0;
    float b = 0.0;

    float y_2 = yuv.x;
    float u_1 = yuv.y;
    float v_2 = yuv.z;
    uint _e12 = color_params.matrix_mode;
    if ((_e12 == MATRIX_BT601_)) {
        r = (y_2 + (1.402 * v_2));
        g = ((y_2 - (0.344136 * u_1)) - (0.714136 * v_2));
        b = (y_2 + (1.772 * u_1));
    } else {
        uint _e29 = color_params.matrix_mode;
        if ((_e29 == MATRIX_BT2020_)) {
            r = (y_2 + (1.4746 * v_2));
            g = ((y_2 - (0.164553 * u_1)) - (0.571353 * v_2));
            b = (y_2 + (1.8814 * u_1));
        } else {
            r = (y_2 + (1.5748 * v_2));
            g = ((y_2 - (0.187324 * u_1)) - (0.468124 * v_2));
            b = (y_2 + (1.8556 * u_1));
        }
    }
    float _e56 = r;
    float _e57 = g;
    float _e58 = b;
    return max(float3(_e56, _e57, _e58), (0.0).xxx);
}

float3 bt2020_constant_luminance_to_linear(float3 yuv_1)
{
    float y_gamma = yuv_1.x;
    float b_gamma = (y_gamma + (yuv_1.y * ((yuv_1.y <= 0.0) ? 1.9404 : 1.5816)));
    float r_gamma = (y_gamma + (yuv_1.z * ((yuv_1.z <= 0.0) ? 1.7184 : 0.9936)));
    uint _e22 = color_params.transfer_mode;
    const float _e23 = decode_transfer_scalar(y_gamma, _e22);
    uint _e26 = color_params.transfer_mode;
    const float _e27 = decode_transfer_scalar(r_gamma, _e26);
    uint _e30 = color_params.transfer_mode;
    const float _e31 = decode_transfer_scalar(b_gamma, _e30);
    float g_linear = (((_e23 - (0.2627 * _e27)) - (0.0593 * _e31)) / 0.678);
    float3 linear_rgb_4 = max(float3(_e27, g_linear, _e31), (0.0).xxx);
    uint _e46 = color_params.transfer_mode;
    if ((_e46 == TRANSFER_HLG)) {
        const float3 _e49 = hlg_scene_to_display_linear(linear_rgb_4);
        return _e49;
    }
    return linear_rgb_4;
}

float3 decode_yuv_to_linear(float y_1, float2 uv_1)
{
    float3 linear_rgb_2 = (0.0).xxx;

    const float3 _e2 = normalize_yuv(y_1, uv_1);
    uint _e8 = color_params.matrix_mode;
    if ((_e8 == MATRIX_BT2020_CONSTANT_LUMINANCE)) {
        const float3 _e11 = bt2020_constant_luminance_to_linear(_e2);
        linear_rgb_2 = _e11;
    } else {
        const float3 _e12 = yuv_to_gamma_rgb(_e2);
        uint _e15 = color_params.transfer_mode;
        const float3 _e16 = decode_transfer_to_linear(_e12, _e15);
        linear_rgb_2 = _e16;
    }
    float3 _e17 = linear_rgb_2;
    uint _e20 = color_params.primaries_mode;
    const float3 _e21 = convert_primaries_to_srgb(_e17, _e20);
    return _e21;
}

float4 render_yuv_sample(float2 sample_coordinates)
{
    float3 linear_rgb_3 = (float3)0;

    float4 _e3 = y_texture.Sample(video_sampler, sample_coordinates);
    float y_3 = _e3.x;
    float4 _e7 = uv_texture.Sample(video_sampler, sample_coordinates);
    float2 uv_3 = _e7.xy;
    const float3 _e9 = decode_yuv_to_linear(y_3, uv_3);
    linear_rgb_3 = _e9;
    uint _e13 = color_params.target_mode;
    if ((_e13 == TARGET_LINEAR_HDR)) {
        float3 _e16 = linear_rgb_3;
        return float4(max(_e16, (0.0).xxx), 1.0);
    }
    uint _e24 = color_params.transfer_mode;
    if ((_e24 != TRANSFER_SDR)) {
        float3 _e27 = linear_rgb_3;
        const float3 _e28 = tone_map_hdr_to_sdr(_e27);
        linear_rgb_3 = _e28;
    }
    float3 _e29 = linear_rgb_3;
    float3 clamped_linear = clamp(_e29, (0.0).xxx, (1.0).xxx);
    uint _e37 = color_params.target_mode;
    if ((_e37 == TARGET_LINEAR_SDR)) {
        return float4(clamped_linear, 1.0);
    }
    const float _e43 = linear_to_bt709_(clamped_linear.x);
    const float _e45 = linear_to_bt709_(clamped_linear.y);
    const float _e47 = linear_to_bt709_(clamped_linear.z);
    float3 gamma_sdr = float3(_e43, _e45, _e47);
    uint _e51 = color_params.target_mode;
    if ((_e51 == TARGET_GAMMA_SDR)) {
        return float4(gamma_sdr, 1.0);
    }
    return float4(clamped_linear, 1.0);
}

VertexOutput_vs_main vs_main(float2 position : LOC0, float2 uv : LOC1)
{
    VertexOutput output = (VertexOutput)0;

    output.position = float4(position, 0.0, 1.0);
    output.uv = uv;
    VertexOutput _e8 = output;
    const VertexOutput vertexoutput = _e8;
    const VertexOutput_vs_main vertexoutput_1 = { vertexoutput.uv, vertexoutput.position };
    return vertexoutput_1;
}

