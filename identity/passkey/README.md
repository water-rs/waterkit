# Waterkit Passkey

Cross-platform passkey (WebAuthn) registration and authentication APIs for Rust apps.

## Features

- Ergonomic client API mirroring native passkey SDK flows
- Async registration and authentication ceremonies
- Strongly typed request and result models
- Raw-byte results plus WebAuthn JSON helper conversion

## Installation

```toml
[dependencies]
waterkit-passkey = "0.1"
# or
waterkit = { version = "0.1", features = ["passkey"] }
```

## Usage

```rust
use waterkit_passkey::{
    Challenge, PasskeyClient, RelyingParty, UserEntity, UserId,
};

async fn example() -> Result<(), waterkit_passkey::PasskeyError> {
    let rp = RelyingParty::new("example.com", "Example")?;
    let client = PasskeyClient::builder().relying_party(rp.clone()).build()?;

    let user = UserEntity::new(
        UserId::new(vec![1, 2, 3, 4])?,
        "alice@example.com",
        "Alice",
    )?;
    let challenge = Challenge::new(vec![0x11; 32])?;

    let registration = client.register(user, challenge.clone()).await?;
    let registration_payload = registration.to_webauthn_json();

    let assertion = client.authenticate(challenge).await?;
    let assertion_payload = assertion.to_webauthn_json();

    let _ = (registration_payload, assertion_payload);
    Ok(())
}
```
