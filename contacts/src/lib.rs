//! Cross-platform contacts access.
//!
//! Provides read/write access to the device's contacts store.
//! iOS/macOS and Android use native system stores. Windows/Linux use a
//! persistent desktop store under the user's local data directory.

#![warn(missing_docs)]
#![warn(missing_debug_implementations)]

mod sys;

#[doc(no_inline)]
pub use jiff::civil::Date;

/// A phone number with label.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub struct PhoneNumber {
    /// The phone number string.
    pub number: String,
    /// Label (e.g., "mobile", "home", "work").
    pub label: Option<String>,
}

/// An email address with label.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub struct EmailAddress {
    /// The email address string.
    pub address: String,
    /// Label (e.g., "home", "work").
    pub label: Option<String>,
}

/// A postal address.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub struct PostalAddress {
    /// Street address.
    pub street: Option<String>,
    /// City.
    pub city: Option<String>,
    /// State/province.
    pub state: Option<String>,
    /// Postal/ZIP code.
    pub postal_code: Option<String>,
    /// Country.
    pub country: Option<String>,
    /// Label.
    pub label: Option<String>,
}

/// A contact entry.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub struct Contact {
    /// Platform-specific contact identifier.
    pub id: String,
    /// Given (first) name.
    pub given_name: Option<String>,
    /// Family (last) name.
    pub family_name: Option<String>,
    /// Organization/company name.
    pub organization: Option<String>,
    /// Phone numbers.
    pub phone_numbers: Vec<PhoneNumber>,
    /// Email addresses.
    pub email_addresses: Vec<EmailAddress>,
    /// Postal addresses.
    pub postal_addresses: Vec<PostalAddress>,
    /// Birthday (calendar date; year may be a placeholder).
    pub birthday: Option<Date>,
    /// Note/memo.
    pub note: Option<String>,
    /// Thumbnail image data (PNG/JPEG bytes).
    pub thumbnail: Option<Vec<u8>>,
}

/// A request to create or update a contact.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub struct ContactData {
    /// Given (first) name.
    pub given_name: Option<String>,
    /// Family (last) name.
    pub family_name: Option<String>,
    /// Organization/company name.
    pub organization: Option<String>,
    /// Phone numbers.
    pub phone_numbers: Vec<PhoneNumber>,
    /// Email addresses.
    pub email_addresses: Vec<EmailAddress>,
    /// Postal addresses.
    pub postal_addresses: Vec<PostalAddress>,
    /// Birthday (calendar date; year may be a placeholder).
    pub birthday: Option<Date>,
    /// Note/memo.
    pub note: Option<String>,
}

/// Fetch all contacts.
///
/// # Errors
/// Returns error if contacts cannot be accessed.
pub async fn fetch_all() -> Result<Vec<Contact>, ContactsError> {
    sys::fetch_all().await
}

/// Search contacts by name.
///
/// # Errors
/// Returns error if search fails.
pub async fn search(query: &str) -> Result<Vec<Contact>, ContactsError> {
    sys::search(query).await
}

/// Get a single contact by ID.
///
/// # Errors
/// Returns error if the contact cannot be found.
pub async fn get(id: &str) -> Result<Contact, ContactsError> {
    sys::get(id).await
}

/// Create a new contact.
///
/// # Errors
/// Returns error if creation fails.
pub async fn create(data: ContactData) -> Result<Contact, ContactsError> {
    sys::create(data).await
}

/// Delete a contact by ID.
///
/// # Errors
/// Returns error if deletion fails.
pub async fn delete(id: &str) -> Result<(), ContactsError> {
    sys::delete(id).await
}

/// Errors in contacts operations.
#[derive(Debug, Clone, thiserror::Error)]
#[non_exhaustive]
pub enum ContactsError {
    /// Contacts access not available.
    #[error("contacts not available")]
    NotAvailable,
    /// Permission denied.
    #[error("contacts permission denied")]
    PermissionDenied,
    /// Contact not found.
    #[error("contact not found: {0}")]
    NotFound(String),
    /// Not supported on this platform.
    #[error("not supported")]
    Unsupported,
    /// Platform error.
    #[error("platform error: {0}")]
    Platform(String),
}
