use std::path::{Path, PathBuf};

use crate::{Contact, ContactData, ContactsError};
use waterkit_fs::WaterFs;

const STORE_FILE_NAME: &str = "contacts.json";

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct ContactsStore {
    next_id: u64,
    contacts: Vec<Contact>,
}

impl Default for ContactsStore {
    fn default() -> Self {
        Self {
            next_id: 1,
            contacts: Vec::new(),
        }
    }
}

pub async fn fetch_all() -> Result<Vec<Contact>, ContactsError> {
    blocking::unblock(|| {
        let path = store_path()?;
        Ok(load_store(&path)?.contacts)
    })
    .await
}

pub async fn search(query: &str) -> Result<Vec<Contact>, ContactsError> {
    let query = query.trim().to_ascii_lowercase();
    blocking::unblock(move || {
        let path = store_path()?;
        let store = load_store(&path)?;
        if query.is_empty() {
            return Ok(store.contacts);
        }
        Ok(store
            .contacts
            .into_iter()
            .filter(|contact| contact_matches_query(contact, &query))
            .collect())
    })
    .await
}

pub async fn get(id: &str) -> Result<Contact, ContactsError> {
    let id = id.to_string();
    blocking::unblock(move || {
        let path = store_path()?;
        let store = load_store(&path)?;
        store
            .contacts
            .into_iter()
            .find(|contact| contact.id == id.as_str())
            .ok_or(ContactsError::NotFound(id))
    })
    .await
}

pub async fn create(data: ContactData) -> Result<Contact, ContactsError> {
    blocking::unblock(move || {
        let path = store_path()?;
        let mut store = load_store(&path)?;
        let id = format!("desktop-{}", store.next_id);
        store.next_id = store.next_id.checked_add(1).ok_or_else(|| {
            ContactsError::Platform("desktop contact id overflow".to_string())
        })?;
        let contact = Contact {
            id,
            given_name: data.given_name,
            family_name: data.family_name,
            organization: data.organization,
            phone_numbers: data.phone_numbers,
            email_addresses: data.email_addresses,
            postal_addresses: data.postal_addresses,
            birthday: data.birthday,
            note: data.note,
            thumbnail: None,
        };
        store.contacts.push(contact.clone());
        write_store(&path, &store)?;
        Ok(contact)
    })
    .await
}

pub async fn delete(id: &str) -> Result<(), ContactsError> {
    let id = id.to_string();
    blocking::unblock(move || {
        let path = store_path()?;
        let mut store = load_store(&path)?;
        let Some(position) = store
            .contacts
            .iter()
            .position(|contact| contact.id == id.as_str())
        else {
            return Err(ContactsError::NotFound(id));
        };
        store.contacts.remove(position);
        write_store(&path, &store)
    })
    .await
}

fn contact_matches_query(contact: &Contact, query: &str) -> bool {
    field_contains(contact.given_name.as_deref(), query)
        || field_contains(contact.family_name.as_deref(), query)
        || field_contains(contact.organization.as_deref(), query)
        || field_contains(contact.note.as_deref(), query)
        || contact
            .phone_numbers
            .iter()
            .any(|phone| phone.number.to_ascii_lowercase().contains(query))
        || contact
            .email_addresses
            .iter()
            .any(|email| email.address.to_ascii_lowercase().contains(query))
}

fn field_contains(value: Option<&str>, query: &str) -> bool {
    value.is_some_and(|field| field.to_ascii_lowercase().contains(query))
}

fn store_path() -> Result<PathBuf, ContactsError> {
    WaterFs::data_local_path(Path::new("waterkit").join("contacts").join(STORE_FILE_NAME)).map_err(
        |error| ContactsError::Platform(format!("resolve contacts store path: {error}")),
    )
}

fn load_store(path: &Path) -> Result<ContactsStore, ContactsError> {
    WaterFs::load_json_store(path).map_err(|error| {
        ContactsError::Platform(format!("load contacts store {}: {error}", path.display()))
    })
}

fn write_store(path: &Path, store: &ContactsStore) -> Result<(), ContactsError> {
    WaterFs::write_json_store(path, store).map_err(|error| {
        ContactsError::Platform(format!("write contacts store {}: {error}", path.display()))
    })
}
