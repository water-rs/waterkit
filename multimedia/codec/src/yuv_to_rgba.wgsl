// Decoded bi-planar YUV to linear RGBA16F conversion.
//
// Plane textures are integer formats (`R8Uint`/`Rg8Uint` for NV12,
// `R16Uint`/`Rg16Uint` for P010), sampled with textureLoad and passed through
// `ycbcr_codes` before the range/matrix/transfer decode.
//
// The range and matrix math is waterkit-video-core's `ycbcr.wgsl`, which the
// build script prepends to this file.

@group(0) @binding(0) var y_texture: texture_2d<u32>;
@group(0) @binding(1) var uv_texture: texture_2d<u32>;

struct ColorParams {
    matrix_mode: u32,
    range_mode: u32,
    primaries_mode: u32,
    transfer_mode: u32,
    target_mode: u32,
    sample_mode: u32,
    max_content_light_nits: f32,
    _padding1: u32,
}

@group(0) @binding(3) var<uniform> color_params: ColorParams;
@group(0) @binding(4) var linear_rgba_output: texture_storage_2d<rgba16float, write>;

const MATRIX_BT2020_CONSTANT_LUMINANCE: u32 = 3u;

const SAMPLE_NV12: u32 = 0u;
const SAMPLE_P010: u32 = 1u;

const PRIMARIES_BT709: u32 = 0u;
const PRIMARIES_BT601: u32 = 1u;
const PRIMARIES_DISPLAY_P3: u32 = 2u;
const PRIMARIES_BT2020: u32 = 3u;

const TRANSFER_SDR: u32 = 0u;
const TRANSFER_PQ: u32 = 1u;
const TRANSFER_HLG: u32 = 2u;

const SDR_REFERENCE_WHITE_NITS: f32 = 203.0;

fn bt709_to_linear(c: f32) -> f32 {
    if c < 0.081 {
        return c / 4.5;
    }
    return pow((c + 0.099) / 1.099, 1.0 / 0.45);
}

fn pq_to_linear(value: f32) -> f32 {
    let m1 = 2610.0 / 16384.0;
    let m2 = 2523.0 / 32.0;
    let c1 = 3424.0 / 4096.0;
    let c2 = 2413.0 / 128.0;
    let c3 = 2392.0 / 128.0;

    let v = clamp(value, 0.0, 1.0);
    let v_pow = pow(v, 1.0 / m2);
    let numerator = max(v_pow - c1, 0.0);
    let denominator = max(c2 - c3 * v_pow, 1e-6);
    let absolute_nits = 10000.0 * pow(numerator / denominator, 1.0 / m1);

    // Normalize to the framework-wide diffuse SDR reference white.
    return absolute_nits / SDR_REFERENCE_WHITE_NITS;
}

fn hlg_to_scene_linear(value: f32) -> f32 {
    let a = 0.17883277;
    let b = 0.28466892;
    let c = 0.55991073;
    let e = clamp(value, 0.0, 1.0);
    var scene_linear = 0.0;
    if e <= 0.5 {
        scene_linear = (e * e) / 3.0;
    } else {
        scene_linear = (exp((e - c) / a) + b) / 12.0;
    }

    return scene_linear;
}

fn hlg_scene_to_display_linear(scene_rgb: vec3<f32>) -> vec3<f32> {
    let safe = max(scene_rgb, vec3<f32>(0.0));
    let scene_luminance = dot(safe, vec3<f32>(0.2627, 0.6780, 0.0593));
    let system_gamma = 1.2;
    let ootf_gain = pow(max(scene_luminance, 1e-6), system_gamma - 1.0);
    return safe * ootf_gain * (1000.0 / SDR_REFERENCE_WHITE_NITS);
}

fn decode_transfer_to_linear(rgb: vec3<f32>, transfer_mode: u32) -> vec3<f32> {
    if transfer_mode == TRANSFER_PQ {
        return vec3<f32>(
            pq_to_linear(rgb.r),
            pq_to_linear(rgb.g),
            pq_to_linear(rgb.b),
        );
    }
    if transfer_mode == TRANSFER_HLG {
        return hlg_scene_to_display_linear(
            vec3<f32>(
                hlg_to_scene_linear(rgb.r),
                hlg_to_scene_linear(rgb.g),
                hlg_to_scene_linear(rgb.b),
            ),
        );
    }
    return vec3<f32>(
        bt709_to_linear(rgb.r),
        bt709_to_linear(rgb.g),
        bt709_to_linear(rgb.b),
    );
}

fn decode_transfer_scalar(value: f32, transfer_mode: u32) -> f32 {
    if transfer_mode == TRANSFER_PQ {
        return pq_to_linear(value);
    }
    if transfer_mode == TRANSFER_HLG {
        return hlg_to_scene_linear(value);
    }
    return bt709_to_linear(value);
}

fn convert_primaries_to_srgb(linear_rgb: vec3<f32>, primaries_mode: u32) -> vec3<f32> {
    if primaries_mode == PRIMARIES_BT2020 {
        return vec3<f32>(
            1.6605 * linear_rgb.r - 0.5876 * linear_rgb.g - 0.0728 * linear_rgb.b,
            -0.1246 * linear_rgb.r + 1.1329 * linear_rgb.g - 0.0083 * linear_rgb.b,
            -0.0182 * linear_rgb.r - 0.1006 * linear_rgb.g + 1.1188 * linear_rgb.b,
        );
    }

    if primaries_mode == PRIMARIES_DISPLAY_P3 {
        return vec3<f32>(
            1.2249 * linear_rgb.r - 0.2247 * linear_rgb.g - 0.0002 * linear_rgb.b,
            -0.0420 * linear_rgb.r + 1.0419 * linear_rgb.g + 0.0001 * linear_rgb.b,
            -0.0197 * linear_rgb.r - 0.0786 * linear_rgb.g + 1.0983 * linear_rgb.b,
        );
    }

    return linear_rgb;
}

fn sample_bit_depth() -> u32 {
    return select(8u, 10u, color_params.sample_mode == SAMPLE_P010);
}

fn sample_element_bits() -> u32 {
    return select(8u, 16u, color_params.sample_mode == SAMPLE_P010);
}

fn normalize_yuv(codes: vec3<f32>) -> vec3<f32> {
    return ycbcr_normalize(codes, color_params.range_mode, sample_bit_depth());
}

fn bt2020_constant_luminance_to_linear(yuv: vec3<f32>) -> vec3<f32> {
    let y_gamma = yuv.x;
    let b_gamma = y_gamma + yuv.y * select(1.5816, 1.9404, yuv.y <= 0.0);
    let r_gamma = y_gamma + yuv.z * select(0.9936, 1.7184, yuv.z <= 0.0);
    let y_linear = decode_transfer_scalar(y_gamma, color_params.transfer_mode);
    let r_linear = decode_transfer_scalar(r_gamma, color_params.transfer_mode);
    let b_linear = decode_transfer_scalar(b_gamma, color_params.transfer_mode);
    let g_linear =
        (y_linear - 0.2627 * r_linear - 0.0593 * b_linear) / 0.6780;
    let linear_rgb = max(
        vec3<f32>(r_linear, g_linear, b_linear),
        vec3<f32>(0.0),
    );
    if color_params.transfer_mode == TRANSFER_HLG {
        return hlg_scene_to_display_linear(linear_rgb);
    }
    return linear_rgb;
}

fn decode_yuv_to_linear(codes: vec3<f32>) -> vec3<f32> {
    let yuv = normalize_yuv(codes);
    var linear_rgb = vec3<f32>(0.0);
    if color_params.matrix_mode == MATRIX_BT2020_CONSTANT_LUMINANCE {
        linear_rgb = bt2020_constant_luminance_to_linear(yuv);
    } else {
        let gamma_rgb = ycbcr_to_rgb(yuv, color_params.matrix_mode);
        linear_rgb = decode_transfer_to_linear(gamma_rgb, color_params.transfer_mode);
    }
    return convert_primaries_to_srgb(linear_rgb, color_params.primaries_mode);
}

@compute @workgroup_size(8, 8)
fn convert_to_linear_rgba(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let dimensions = textureDimensions(linear_rgba_output);
    if global_id.x >= dimensions.x || global_id.y >= dimensions.y {
        return;
    }

    let y_coordinates = vec2<i32>(global_id.xy);
    let uv_coordinates = vec2<i32>(
        i32(global_id.x / 2u),
        i32(global_id.y / 2u),
    );
    let y = textureLoad(y_texture, y_coordinates, 0).r;
    let uv = textureLoad(uv_texture, uv_coordinates, 0).rg;
    let elements = vec3<u32>(y, uv);
    let codes = ycbcr_codes(elements, sample_element_bits(), sample_bit_depth());
    let linear_rgb = decode_yuv_to_linear(codes);
    textureStore(
        linear_rgba_output,
        y_coordinates,
        vec4<f32>(max(linear_rgb, vec3<f32>(0.0)), 1.0),
    );
}
