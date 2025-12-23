//! Android JNI generic test harness.

#![cfg(target_os = "android")]
#![allow(non_snake_case)]

use jni::objects::{JClass, JObject};
use jni::JNIEnv;

// This harness expects `waterkit_content` to be available.
// The CLI ensures this dependency is injected.
// We use a feature flag or conditional compilation to avoid checking errors when the dep is missing during normal builds?
// No, the harness crate is *only* useful when driven by CLI.
// But to allow `cargo check` on the workspace, we might need a dummy fallback.
// Or we just accept that `waterkit-test-android` won't compile without modification.

#[no_mangle]
pub extern "system" fn Java_com_waterkit_test_MainActivity_runTest(
    mut _env: JNIEnv,
    _class: JClass,
    _activity: JObject,
) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
        
    rt.block_on(async {
        // We use a macro or just direct call.
        // We assume `waterkit_content` crate root exposes `run()`.
        
        // UNCOMMENT THIS LINE AFTER CLI UPDATE:
        // waterkit_content::run().await;
        
        // For verify step now, I'll hardcode a print.
        println!("Harness running... (content binding pending)");
    });
}
