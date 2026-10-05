//! Compiles the preview harness's WGSL shader.

fn main() {
    shaderloom::build::compile_wgsl_shader("src/shader.wgsl", "camera_test");
}
