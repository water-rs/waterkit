# waterkit-contacts

Cross-platform contacts access for Rust.

Part of the [Waterkit](https://github.com/water-rs/waterkit) ecosystem.

## Features

- Fetch all contacts or search by name
- Get individual contacts by ID
- Create and delete contacts
- Full contact data: phone numbers, emails, postal addresses, birthdays, thumbnails

## Platform Support

| Platform | Status |
|----------|--------|
| iOS      | Native (CNContact via Swift bridge) |
| macOS    | Native (CNContact via Swift bridge) |
| Android  | Native (ContactsContract via JNI/Kotlin) |
| Windows  | Not supported |
| Linux    | Not supported |

## Usage

```rust
use waterkit_contacts::{fetch_all, search, create, ContactData, PhoneNumber};

async fn example() -> Result<(), waterkit_contacts::ContactsError> {
    // Fetch all contacts
    let contacts = fetch_all().await?;

    // Search by name
    let results = search("Alice").await?;

    // Create a new contact
    let contact = create(ContactData {
        given_name: Some("Bob".into()),
        family_name: Some("Smith".into()),
        phone_numbers: vec![PhoneNumber {
            number: "+1234567890".into(),
            label: Some("mobile".into()),
        }],
        ..Default::default()
    }).await?;

    Ok(())
}
```

## License

MIT OR Apache-2.0
