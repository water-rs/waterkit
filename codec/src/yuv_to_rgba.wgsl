// YUV (NV12) to RGBA compute shader

@group(0) @binding(0) var y_texture: texture_2d<f32>;
@group(0) @binding(1) var uv_texture: texture_2d<f32>;
@group(0) @binding(2) var output: texture_storage_2d<rgba8unorm, write>;

// BT.601 YUV to RGB conversion matrix
fn yuv_to_rgb(y: f32, u: f32, v: f32) -> vec3<f32> {
    let y_norm = y - 0.0625;  // 16/256
    let u_norm = u - 0.5;
    let v_norm = v - 0.5;

    let r = 1.164 * y_norm + 1.596 * v_norm;
    let g = 1.164 * y_norm - 0.392 * u_norm - 0.813 * v_norm;
    let b = 1.164 * y_norm + 2.017 * u_norm;

    return clamp(vec3(r, g, b), vec3(0.0), vec3(1.0));
}

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let dims = textureDimensions(output);
    if global_id.x >= dims.x || global_id.y >= dims.y {
        return;
    }

    let coords = vec2<i32>(global_id.xy);
    let uv_coords = vec2<i32>(global_id.xy / 2u);

    let y = textureLoad(y_texture, coords, 0).r;
    let uv = textureLoad(uv_texture, uv_coords, 0).rg;

    let rgb = yuv_to_rgb(y, uv.r, uv.g);

    textureStore(output, coords, vec4(rgb, 1.0));
}
