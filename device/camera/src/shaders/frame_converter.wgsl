// Shared part of the camera frame converter: the conversion parameters, the
// upright output, and the orientation mapping. Each plane layout's entry
// point lives in its own file and is composed after this one by build.rs.

struct ConvertParams {
    // EXIF orientation (1-8) of the stored pixels.
    orientation: u32,
    // YCBCR_MATRIX_* and YCBCR_RANGE_* from waterkit-video-core's ycbcr.wgsl.
    matrix_mode: u32,
    range_mode: u32,
    // Significant bits per YCbCr sample.
    bit_depth: u32,
    // Factor taking a sampled unorm value to `code / (2^bit_depth - 1)`.
    code_scale: f32,
    // Stored (pre-orientation) frame size in pixels.
    stored_width: u32,
    stored_height: u32,
    _padding: u32,
}

@group(0) @binding(2) var<uniform> params: ConvertParams;
@group(0) @binding(3) var upright: texture_storage_2d<rgba8unorm, write>;

// The stored pixel shown at `xy` of the upright image.
fn stored_coordinate(xy: vec2<u32>) -> vec2<u32> {
    let last = vec2<u32>(params.stored_width - 1u, params.stored_height - 1u);
    var stored = xy;
    switch params.orientation {
        case 2u: { stored = vec2<u32>(last.x - xy.x, xy.y); }
        case 3u: { stored = vec2<u32>(last.x - xy.x, last.y - xy.y); }
        case 4u: { stored = vec2<u32>(xy.x, last.y - xy.y); }
        case 5u: { stored = vec2<u32>(xy.y, xy.x); }
        case 6u: { stored = vec2<u32>(xy.y, last.y - xy.x); }
        case 7u: { stored = vec2<u32>(last.x - xy.y, last.y - xy.x); }
        case 8u: { stored = vec2<u32>(last.x - xy.y, xy.x); }
        default: {}
    }
    return stored;
}

fn outside_upright(xy: vec2<u32>) -> bool {
    return any(xy >= textureDimensions(upright));
}

fn store_upright(xy: vec2<u32>, rgb: vec3<f32>) {
    textureStore(upright, vec2<i32>(xy), vec4<f32>(clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0));
}
