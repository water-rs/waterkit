use waterkit_passkey as passkey;

#[tokio::main]
async fn main() {
    println!("Checking passkey availability...");

    match passkey::is_available().await {
        Ok(availability) => println!(
            "Passkey availability: supported={} uv={} discoverable={}",
            availability.is_platform_supported,
            availability.supports_user_verification,
            availability.supports_discoverable_credentials
        ),
        Err(error) => println!("Passkey availability check failed: {error}"),
    }
}
