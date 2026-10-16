// Interleaved 8-bit RGB(A) frame to the upright output.

@group(0) @binding(0) var rgb_plane: texture_2d<f32>;

@compute @workgroup_size(8, 8)
fn convert_rgb(@builtin(global_invocation_id) id: vec3<u32>) {
    if outside_upright(id.xy) {
        return;
    }
    let stored = vec2<i32>(stored_coordinate(id.xy));
    store_upright(id.xy, textureLoad(rgb_plane, stored, 0).rgb);
}
