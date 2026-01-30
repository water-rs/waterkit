// NV12 to RGB conversion shader
// Y texture: R8Unorm (luminance)
// UV texture: Rg8Unorm (chrominance, half resolution)

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) idx: u32) -> VertexOutput {
    // Full-screen quad vertices
    var positions = array<vec2<f32>, 6>(
        vec2(-1.0, -1.0),
        vec2( 1.0, -1.0),
        vec2(-1.0,  1.0),
        vec2(-1.0,  1.0),
        vec2( 1.0, -1.0),
        vec2( 1.0,  1.0),
    );

    var uvs = array<vec2<f32>, 6>(
        vec2(0.0, 1.0),
        vec2(1.0, 1.0),
        vec2(0.0, 0.0),
        vec2(0.0, 0.0),
        vec2(1.0, 1.0),
        vec2(1.0, 0.0),
    );

    var out: VertexOutput;
    out.position = vec4(positions[idx], 0.0, 1.0);
    out.uv = uvs[idx];
    return out;
}

@group(0) @binding(0) var t_y: texture_2d<f32>;
@group(0) @binding(1) var t_uv: texture_2d<f32>;
@group(0) @binding(2) var s: sampler;

// BT.709 YUV to RGB conversion matrix
fn yuv_to_rgb(y: f32, u: f32, v: f32) -> vec3<f32> {
    // Shift UV from [0,1] to [-0.5, 0.5]
    let u_shifted = u - 0.5;
    let v_shifted = v - 0.5;

    // BT.709 coefficients
    let r = y + 1.5748 * v_shifted;
    let g = y - 0.1873 * u_shifted - 0.4681 * v_shifted;
    let b = y + 1.8556 * u_shifted;

    return clamp(vec3(r, g, b), vec3(0.0), vec3(1.0));
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let y = textureSample(t_y, s, in.uv).r;
    let uv = textureSample(t_uv, s, in.uv).rg;

    let rgb = yuv_to_rgb(y, uv.r, uv.g);
    return vec4(rgb, 1.0);
}
