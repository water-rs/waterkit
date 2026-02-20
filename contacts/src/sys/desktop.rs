use crate::{Contact, ContactData, ContactsError};

#[allow(clippy::unused_async)]
pub async fn fetch_all() -> Result<Vec<Contact>, ContactsError> {
    Err(ContactsError::NotSupported)
}

#[allow(clippy::unused_async)]
pub async fn search(_query: &str) -> Result<Vec<Contact>, ContactsError> {
    Err(ContactsError::NotSupported)
}

#[allow(clippy::unused_async)]
pub async fn get(_id: &str) -> Result<Contact, ContactsError> {
    Err(ContactsError::NotSupported)
}

#[allow(clippy::unused_async)]
pub async fn create(_data: ContactData) -> Result<Contact, ContactsError> {
    Err(ContactsError::NotSupported)
}

#[allow(clippy::unused_async)]
pub async fn delete(_id: &str) -> Result<(), ContactsError> {
    Err(ContactsError::NotSupported)
}
