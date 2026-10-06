// Y'CbCr to R'G'B' conversion shared by WaterKit's GPU colour converters.
//
// This is a fragment, not a module: a converter prepends it to its own WGSL
// (shaderloom's `--prepend`, or string composition in a build script) and
// calls the functions below. It declares no bindings.
//
// Sample values arrive as code values normalized by the sample's largest
// code, `code / (2^bit_depth - 1)`, so 8-bit and 10-bit sources share one
// range definition. The output is non-linear R'G'B' in the source's own
// transfer function and primaries; linearization belongs to the caller.
//
// The YCBCR_MATRIX_* and YCBCR_RANGE_* mode constants are declared ahead of
// this text by waterkit-video-core's `YCBCR_WGSL`, from the same Rust
// definitions converters fill their uniforms with.

// Removes the range offset and scale: Y' in [0, 1], Cb and Cr in [-0.5, 0.5].
// Limited range places black at 16 and white at 235 (chroma 16..240) in
// 8-bit codes, scaled by 2^(bit_depth - 8) for deeper samples.
fn ycbcr_normalize(y: f32, cbcr: vec2<f32>, range_mode: u32, bit_depth: u32) -> vec3<f32> {
    let max_code = f32((1u << bit_depth) - 1u);
    let step = f32(1u << (bit_depth - 8u));
    let chroma_zero = 128.0 * step / max_code;
    if range_mode == YCBCR_RANGE_LIMITED {
        let luma = (y - 16.0 * step / max_code) * (max_code / (219.0 * step));
        let chroma = (cbcr - vec2<f32>(chroma_zero)) * (max_code / (224.0 * step));
        return vec3<f32>(luma, chroma);
    }
    return vec3<f32>(y, cbcr - vec2<f32>(chroma_zero));
}

// Applies a non-constant-luminance matrix to normalized Y'CbCr. Negative
// results are clamped to zero; values above one are left for the caller.
fn ycbcr_to_rgb(ycbcr: vec3<f32>, matrix_mode: u32) -> vec3<f32> {
    let y = ycbcr.x;
    let cb = ycbcr.y;
    let cr = ycbcr.z;

    var rgb = vec3<f32>(0.0);
    if matrix_mode == YCBCR_MATRIX_BT601 {
        rgb = vec3<f32>(y + 1.402 * cr, y - 0.344136 * cb - 0.714136 * cr, y + 1.772 * cb);
    } else if matrix_mode == YCBCR_MATRIX_BT2020 {
        rgb = vec3<f32>(y + 1.4746 * cr, y - 0.164553 * cb - 0.571353 * cr, y + 1.8814 * cb);
    } else {
        rgb = vec3<f32>(y + 1.5748 * cr, y - 0.187324 * cb - 0.468124 * cr, y + 1.8556 * cb);
    }
    return max(rgb, vec3<f32>(0.0));
}
