use waterkit_biometric as biometric;

#[tokio::main]
async fn main() {
    println!("Probing biometric capabilities...");
    let caps = biometric::capabilities().await;
    println!("Available: {}", caps.available);

    if caps.available {
        if let Some(bio_type) = caps.kind {
            println!("Biometric type: {:?}", bio_type);
        }

        println!("Requesting authentication...");
        match biometric::authenticate("Test authentication from Rust").await {
            Ok(()) => println!("✅ Authentication SUCCESS!"),
            Err(e) => println!("❌ Authentication FAILED: {e}"),
        }
    } else {
        println!("Biometrics not available on this machine.");
    }
}
