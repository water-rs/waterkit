# waterkit-deeplink

Cross-platform deep linking and URL scheme handling for Rust.

Part of the [Waterkit](https://github.com/water-rs/waterkit) ecosystem.

## Features

- Receive incoming deep links (custom URL schemes and universal links)
- Open URLs in external apps
- Check if a URL scheme can be handled
- Parse deep link URLs into structured components

## Platform Support

| Platform | Status |
|----------|--------|
| iOS      | Native (UIApplication via Swift bridge) |
| macOS    | Native (NSAppleEventManager via Swift bridge) |
| Android  | Native (Intent via JNI/Kotlin) |
| Windows  | Open URL / can_open_url only (incoming-link listener pending) |
| Linux    | Open URL / can_open_url only (incoming-link listener pending) |

## Usage

```rust
use waterkit_deeplink::{DeepLinkHandler, DeepLink, open_url, can_open_url};

async fn example() -> Result<(), waterkit_deeplink::DeepLinkError> {
    // Listen for incoming deep links
    let (handler, rx) = DeepLinkHandler::start().await?;

    // Check initial launch link
    if let Some(link) = handler.initial_link()? {
        handle_link(&link);
    }

    // Open a URL externally
    open_url("https://example.com").await?;

    // Check if a URL scheme is registered
    let can_open = can_open_url("myapp://action").await?;

    Ok(())
}

fn handle_link(link: &DeepLink) {
    // Access parsed components
    let _scheme = &link.scheme;
    let _host = &link.host;
    let _path = &link.path;
    let _params = &link.query_params;
}
```

## License

MIT OR Apache-2.0
