// Biplanar 4:2:0 YCbCr frame to the upright output. Chroma is sampled at the
// nearest co-sited position.

@group(0) @binding(0) var luma_plane: texture_2d<f32>;
@group(0) @binding(1) var chroma_plane: texture_2d<f32>;

@compute @workgroup_size(8, 8)
fn convert_ycbcr420(@builtin(global_invocation_id) id: vec3<u32>) {
    if outside_upright(id.xy) {
        return;
    }
    let stored = stored_coordinate(id.xy);
    let y = textureLoad(luma_plane, vec2<i32>(stored), 0).r;
    let cbcr = textureLoad(chroma_plane, vec2<i32>(stored >> vec2<u32>(1u)), 0).rg;
    let codes = ycbcr_unorm_codes(vec3<f32>(y, cbcr), params.element_bits, params.bit_depth);
    let ycbcr = ycbcr_normalize(codes, params.range_mode, params.bit_depth);
    store_upright(id.xy, ycbcr_to_rgb(ycbcr, params.matrix_mode));
}
