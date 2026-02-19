# Waterkit Secret

Secure storage for sensitive data (passwords, tokens).

## Features

- **Unified API**: One `SecretManager` API across desktop and mobile.
- **Hardware-backed security on Android**: Encrypts with Android Keystore AES keys.
- **Native secure stores on desktop/Apple**: Uses platform keyring backends.

## Installation

```toml
[dependencies]
waterkit-secret = "0.1"
```

## Platform Support

| Platform | Backend |
| :--- | :--- |
| **macOS/iOS** | `Keychain` |
| **Android** | `AndroidKeyStore` (AES-GCM key) + encrypted payload in `SharedPreferences` |
| **Windows** | `Credential Locker` |
| **Linux** | `linux-keyutils` (kernel keyring) |

## Usage

```rust
use waterkit_secret::SecretManager;

async fn manage_secrets() {
    SecretManager::set("com.myapp.service", "api_token", "secret_value_123")
        .await
        .unwrap();

    let token = SecretManager::get("com.myapp.service", "api_token")
        .await
        .unwrap();

    SecretManager::delete("com.myapp.service", "api_token")
        .await
        .unwrap();

    let _ = token;
}
```
