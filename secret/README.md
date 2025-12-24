# Waterkit Secret

Secure storage for sensitive data (passwords, tokens).

## Features

- **Secure Storage**: Saves data to the system's secure element or encrypted store.
- **Simple Key-Value**: Store strings or binary data by key.

## Installation

```toml
[dependencies]
waterkit-secret = "0.1"
```

## Platform Support

| Platform | Backend |
| :--- | :--- |
| **macOS/iOS** | `Keychain` |
| **Android** | `EncryptedSharedPreferences` / `Keystore` |
| **Windows** | `Credential Locker` |
| **Linux** | `Secret Service` (Gnome/KDE) |

## Usage

```rust
use waterkit_secret::SecretStore;

async fn manage_secrets() {
    let store = SecretStore::new("com.myapp.service").await.unwrap();
    
    // Save
    store.set("api_token", "secret_value_123").await.unwrap();
    
    // Retrieve
    let token = store.get("api_token").await.unwrap();
    println!("Token: {}", token);
    
    // Delete
    store.delete("api_token").await.unwrap();
}
```
