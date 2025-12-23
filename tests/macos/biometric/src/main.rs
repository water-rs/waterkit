use waterkit_biometric as biometric;

#[tokio::main]
async fn main() {
    println!("Checking biometric availability...");
    let available = biometric::is_available().await;
    println!("Is available: {}", available);

    if available {
        if let Some(bio_type) = biometric::get_biometric_type().await {
            println!("Biometric type: {:?}", bio_type);
        }

        println!("Requesting authentication...");
        match biometric::authenticate("Test authentication from Rust").await {
            Ok(_) => println!("✅ Authentication SUCCESS!"),
            Err(e) => println!("❌ Authentication FAILED: {}", e),
        }
    } else {
        println!("Biometrics not available on this machine.");
    }
}
