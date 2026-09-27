//! The YUV→RGBA shader ships pre-translated under `src/shaders/compiled`.
//! This test is the staleness gate: it re-packages the WGSL exactly as
//! `package-shaders.sh` does and asserts the checked-in artifacts match byte
//! for byte. Run `package-shaders.sh` to regenerate after editing the `.wgsl`.

use std::path::Path;

#[test]
fn packaged_shaders_are_current() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let source = std::fs::read_to_string(manifest_dir.join("src/yuv_to_rgba.wgsl"))
        .expect("failed to read src/yuv_to_rgba.wgsl");
    shaderloom::build::package_wgsl(
        "src/yuv_to_rgba.wgsl",
        &source,
        "yuv_color",
        "src/shaders/compiled",
    )
    .assert_current(manifest_dir);
}
