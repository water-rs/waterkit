use crate::{Contact, ContactData, ContactsError};

#[swift_bridge::bridge]
mod ffi {
    extern "Swift" {
        fn contacts_fetch_all(callback: Box<dyn FnOnce(String, String) -> ()>);
        fn contacts_search(query: &str, callback: Box<dyn FnOnce(String, String) -> ()>);
        fn contacts_get(id: &str, callback: Box<dyn FnOnce(String, String) -> ()>);
        fn contacts_create(json: &str, callback: Box<dyn FnOnce(String, String) -> ()>);
        fn contacts_delete(id: &str, callback: Box<dyn FnOnce(String) -> ()>);
    }
}

pub async fn fetch_all() -> Result<Vec<Contact>, ContactsError> {
    let (tx, rx) = futures::channel::oneshot::channel();
    ffi::contacts_fetch_all(Box::new(move |json: String, error: String| {
        if error.is_empty() {
            let _ = tx.send(Ok(json));
        } else {
            let _ = tx.send(Err(ContactsError::PlatformError(error)));
        }
    }));
    let json = rx
        .await
        .map_err(|_| ContactsError::PlatformError("callback dropped".into()))??;
    Ok(parse_contacts_json(&json))
}

pub async fn search(query: &str) -> Result<Vec<Contact>, ContactsError> {
    let (tx, rx) = futures::channel::oneshot::channel();
    ffi::contacts_search(
        query,
        Box::new(move |json: String, error: String| {
            if error.is_empty() {
                let _ = tx.send(Ok(json));
            } else {
                let _ = tx.send(Err(ContactsError::PlatformError(error)));
            }
        }),
    );
    let json = rx
        .await
        .map_err(|_| ContactsError::PlatformError("callback dropped".into()))??;
    Ok(parse_contacts_json(&json))
}

pub async fn get(id: &str) -> Result<Contact, ContactsError> {
    let (tx, rx) = futures::channel::oneshot::channel();
    ffi::contacts_get(
        id,
        Box::new(move |json: String, error: String| {
            if error.is_empty() {
                let _ = tx.send(Ok(json));
            } else {
                let _ = tx.send(Err(ContactsError::PlatformError(error)));
            }
        }),
    );
    let json = rx
        .await
        .map_err(|_| ContactsError::PlatformError("callback dropped".into()))??;
    parse_contacts_json(&json)
        .into_iter()
        .next()
        .ok_or_else(|| ContactsError::NotFound(id.to_string()))
}

pub async fn create(data: ContactData) -> Result<Contact, ContactsError> {
    let json = serialize_contact_data(&data);
    let (tx, rx) = futures::channel::oneshot::channel();
    ffi::contacts_create(
        &json,
        Box::new(move |result_json: String, error: String| {
            if error.is_empty() {
                let _ = tx.send(Ok(result_json));
            } else {
                let _ = tx.send(Err(ContactsError::PlatformError(error)));
            }
        }),
    );
    let result_json = rx
        .await
        .map_err(|_| ContactsError::PlatformError("callback dropped".into()))??;
    parse_contacts_json(&result_json)
        .into_iter()
        .next()
        .ok_or_else(|| ContactsError::PlatformError("failed to create contact".into()))
}

pub async fn delete(id: &str) -> Result<(), ContactsError> {
    let (tx, rx) = futures::channel::oneshot::channel();
    ffi::contacts_delete(
        id,
        Box::new(move |error: String| {
            if error.is_empty() {
                let _ = tx.send(Ok(()));
            } else {
                let _ = tx.send(Err(ContactsError::PlatformError(error)));
            }
        }),
    );
    rx.await
        .map_err(|_| ContactsError::PlatformError("callback dropped".into()))?
}

fn parse_contacts_json(json: &str) -> Vec<Contact> {
    json.lines()
        .filter(|l| !l.is_empty())
        .map(|line| {
            let parts: Vec<&str> = line.split('\t').collect();
            Contact {
                id: parts.first().unwrap_or(&"").to_string(),
                given_name: parts
                    .get(1)
                    .filter(|s| !s.is_empty())
                    .map(ToString::to_string),
                family_name: parts
                    .get(2)
                    .filter(|s| !s.is_empty())
                    .map(ToString::to_string),
                organization: parts
                    .get(3)
                    .filter(|s| !s.is_empty())
                    .map(ToString::to_string),
                phone_numbers: parts
                    .get(4)
                    .unwrap_or(&"")
                    .split(',')
                    .filter(|s| !s.is_empty())
                    .map(|p| crate::PhoneNumber {
                        number: p.to_string(),
                        label: None,
                    })
                    .collect(),
                email_addresses: parts
                    .get(5)
                    .unwrap_or(&"")
                    .split(',')
                    .filter(|s| !s.is_empty())
                    .map(|e| crate::EmailAddress {
                        address: e.to_string(),
                        label: None,
                    })
                    .collect(),
                postal_addresses: Vec::new(),
                birthday: parts
                    .get(6)
                    .filter(|s| !s.is_empty())
                    .map(ToString::to_string),
                note: parts
                    .get(7)
                    .filter(|s| !s.is_empty())
                    .map(ToString::to_string),
                thumbnail: None,
            }
        })
        .collect()
}

fn serialize_contact_data(data: &ContactData) -> String {
    let phones = data
        .phone_numbers
        .iter()
        .map(|p| p.number.as_str())
        .collect::<Vec<_>>()
        .join(",");
    let emails = data
        .email_addresses
        .iter()
        .map(|e| e.address.as_str())
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "{}\t{}\t{}\t{}\t{}\t{}\t{}",
        data.given_name.as_deref().unwrap_or(""),
        data.family_name.as_deref().unwrap_or(""),
        data.organization.as_deref().unwrap_or(""),
        phones,
        emails,
        data.birthday.as_deref().unwrap_or(""),
        data.note.as_deref().unwrap_or(""),
    )
}
