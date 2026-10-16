// Packed YUYV 4:2:2 frame to the upright output. Each texel of the packed
// texture holds two horizontally adjacent pixels as (Y'0, Cb, Y'1, Cr).

@group(0) @binding(0) var yuyv_plane: texture_2d<f32>;

@compute @workgroup_size(8, 8)
fn convert_ycbcr422(@builtin(global_invocation_id) id: vec3<u32>) {
    if outside_upright(id.xy) {
        return;
    }
    let stored = stored_coordinate(id.xy);
    let texel = textureLoad(yuyv_plane, vec2<i32>(i32(stored.x / 2u), i32(stored.y)), 0);
    let y = select(texel.r, texel.b, (stored.x & 1u) == 1u);
    let codes = ycbcr_unorm_codes(vec3<f32>(y, texel.ga), params.element_bits, params.bit_depth);
    let ycbcr = ycbcr_normalize(codes, params.range_mode, params.bit_depth);
    store_upright(id.xy, ycbcr_to_rgb(ycbcr, params.matrix_mode));
}
