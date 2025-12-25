#[swift_bridge::bridge]
mod ffi {
    extern "Rust" {
        fn test_biometric();
    }
}

fn test_biometric() {
    println!("Testing biometric from Rust!");
    // In a real scenario, this would call waterkit_content::...
}
