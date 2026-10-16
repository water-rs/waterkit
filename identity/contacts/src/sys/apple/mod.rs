//! Apple contacts implementation backed by the Contacts framework.

use std::cell::RefCell;
use std::ptr::NonNull;

use block2::RcBlock;
use futures::channel::oneshot;
use objc2::rc::Retained;
use objc2::runtime::{Bool, ProtocolObject};
use objc2::{AnyThread, msg_send};
use objc2_contacts::{
    CNContact, CNContactBirthdayKey, CNContactEmailAddressesKey, CNContactFamilyNameKey,
    CNContactFetchRequest, CNContactGivenNameKey, CNContactIdentifierKey, CNContactNoteKey,
    CNContactOrganizationNameKey, CNContactPhoneNumbersKey, CNContactStore, CNEntityType,
    CNKeyDescriptor, CNLabelHome, CNLabelPhoneNumberMobile, CNLabeledValue, CNMutableContact,
    CNPhoneNumber, CNSaveRequest,
};
use objc2_foundation::{NSArray, NSDateComponentUndefined, NSError, NSString};

use crate::{Contact, ContactData, ContactsError, EmailAddress, PhoneNumber};

type KeyDescriptors = NSArray<ProtocolObject<dyn CNKeyDescriptor>>;

pub async fn fetch_all() -> Result<Vec<Contact>, ContactsError> {
    request_contacts_access()
        .await
        .map_err(|_| ContactsError::Platform("contacts access callback dropped".into()))?
}

fn request_contacts_access() -> oneshot::Receiver<Result<Vec<Contact>, ContactsError>> {
    waterkit_core::apple::require_usage_description(
        "NSContactsUsageDescription",
        "contacts access",
    );
    // SAFETY: `new` is a convenience constructor with no invariants to uphold.
    let store = unsafe { CNContactStore::new() };
    let (sender, receiver) = oneshot::channel();
    let sender = RefCell::new(Some(sender));
    let block_store = store.clone();
    let block = RcBlock::new(move |granted: Bool, error: *mut NSError| {
        let result = if granted.as_bool() {
            fetch_all_inner(&block_store)
        } else {
            Err(ContactsError::Platform(access_error_message(error)))
        };
        if let Some(sender) = sender.borrow_mut().take() {
            let _ = sender.send(result);
        }
    });
    // SAFETY: `block` is a valid Objective-C block matching the documented
    // `granted:completionHandler:` signature; the completion handler is invoked
    // exactly once on a Contacts framework queue.
    unsafe { store.requestAccessForEntityType_completionHandler(CNEntityType::Contacts, &block) };
    receiver
}

pub async fn search(query: &str) -> Result<Vec<Contact>, ContactsError> {
    let query = query.to_owned();
    blocking::unblock(move || {
        // SAFETY: `new` is a convenience constructor with no invariants to uphold.
        let store = unsafe { CNContactStore::new() };
        let query = NSString::from_str(&query);
        // SAFETY: `predicateForContactsMatchingName:` is a documented factory for
        // `unifiedContactsMatchingPredicate:` predicates.
        let predicate = unsafe { CNContact::predicateForContactsMatchingName(&query) };
        // SAFETY: `store` and `predicate` are live and the key array only holds
        // contact key descriptors.
        unsafe {
            store.unifiedContactsMatchingPredicate_keysToFetch_error(&predicate, &fetch_keys(false))
        }
        .map_err(|error| platform_error(&error))
        .map(|contacts| {
            contacts
                .iter()
                .map(|contact| contact_from_objc(&contact, false))
                .collect()
        })
    })
    .await
}

pub async fn get(id: &str) -> Result<Contact, ContactsError> {
    let id = id.to_owned();
    blocking::unblock(move || {
        // SAFETY: `new` is a convenience constructor with no invariants to uphold.
        let store = unsafe { CNContactStore::new() };
        let id = NSString::from_str(&id);
        // SAFETY: `store` and `id` are live and the key array only holds contact
        // key descriptors.
        unsafe { store.unifiedContactWithIdentifier_keysToFetch_error(&id, &fetch_keys(true)) }
            .map_err(|error| platform_error(&error))
            .map(|contact| contact_from_objc(&contact, true))
    })
    .await
}

pub async fn create(data: ContactData) -> Result<Contact, ContactsError> {
    blocking::unblock(move || {
        // SAFETY: `new` is a convenience constructor with no invariants to
        // uphold; the values set below are freshly built NSObjects.
        let contact = unsafe { CNMutableContact::new() };
        // SAFETY: all setters take live values; `contact` is a valid
        // `CNMutableContact` for the entire block.
        unsafe {
            contact.setGivenName(&NSString::from_str(
                data.given_name.as_deref().unwrap_or_default(),
            ));
            contact.setFamilyName(&NSString::from_str(
                data.family_name.as_deref().unwrap_or_default(),
            ));
            contact.setOrganizationName(&NSString::from_str(
                data.organization.as_deref().unwrap_or_default(),
            ));
            let phone_numbers: Vec<Retained<CNLabeledValue<CNPhoneNumber>>> = data
                .phone_numbers
                .iter()
                .map(|phone| {
                    let value = CNPhoneNumber::initWithStringValue(
                        CNPhoneNumber::alloc(),
                        &NSString::from_str(&phone.number),
                    );
                    CNLabeledValue::initWithLabel_value(
                        CNLabeledValue::alloc(),
                        Some(CNLabelPhoneNumberMobile),
                        &*value,
                    )
                })
                .collect();
            contact.setPhoneNumbers(&NSArray::from_retained_slice(&phone_numbers));
            let email_addresses: Vec<Retained<CNLabeledValue<NSString>>> = data
                .email_addresses
                .iter()
                .map(|email| {
                    let address = NSString::from_str(&email.address);
                    CNLabeledValue::initWithLabel_value(
                        CNLabeledValue::alloc(),
                        Some(CNLabelHome),
                        &*address,
                    )
                })
                .collect();
            contact.setEmailAddresses(&NSArray::from_retained_slice(&email_addresses));
            if let Some(note) = data.note.filter(|note| !note.is_empty()) {
                contact.setNote(&NSString::from_str(&note));
            }
        }
        // SAFETY: `new` is a convenience constructor with no invariants to uphold.
        let store = unsafe { CNContactStore::new() };
        // SAFETY: `new` is a convenience constructor with no invariants to uphold.
        let save_request = unsafe { CNSaveRequest::new() };
        // SAFETY: `contact` was created above and is added to the default container.
        unsafe { save_request.addContact_toContainerWithIdentifier(&contact, None) };
        // SAFETY: `save_request` holds the freshly created contact.
        unsafe { store.executeSaveRequest_error(&save_request) }
            .map_err(|error| platform_error(&error))?;
        Ok(created_contact(&contact))
    })
    .await
}

pub async fn delete(id: &str) -> Result<(), ContactsError> {
    let id = id.to_owned();
    blocking::unblock(move || {
        // SAFETY: `new` is a convenience constructor with no invariants to uphold.
        let store = unsafe { CNContactStore::new() };
        let id = NSString::from_str(&id);
        // SAFETY: `CNContactIdentifierKey` is an immutable extern NSString
        // static exposed by the Contacts framework.
        let keys =
            NSArray::from_slice(&[unsafe { ProtocolObject::from_ref(CNContactIdentifierKey) }]);
        // SAFETY: `store` and `id` are live and the key array only holds contact
        // key descriptors.
        let contact = unsafe { store.unifiedContactWithIdentifier_keysToFetch_error(&id, &keys) }
            .map_err(|error| platform_error(&error))?;
        // SAFETY: `mutableCopy` on a `CNContact` returns a retained
        // `CNMutableContact` copy per `NSMutableCopying`'s contract.
        let mutable: Retained<CNMutableContact> = unsafe { msg_send![&contact, mutableCopy] };
        // SAFETY: `new` is a convenience constructor with no invariants to uphold.
        let save_request = unsafe { CNSaveRequest::new() };
        // SAFETY: `mutable` refers to the fetched contact marked for deletion.
        unsafe { save_request.deleteContact(&mutable) };
        // SAFETY: `save_request` holds the deletion mutation above.
        unsafe { store.executeSaveRequest_error(&save_request) }
            .map_err(|error| platform_error(&error))
    })
    .await
}

fn fetch_all_inner(store: &CNContactStore) -> Result<Vec<Contact>, ContactsError> {
    // SAFETY: `initWithKeysToFetch:` is the designated initializer for a fetch
    // request; the key array only holds contact key descriptors.
    let request = unsafe {
        CNContactFetchRequest::initWithKeysToFetch(
            CNContactFetchRequest::alloc(),
            &fetch_keys(true),
        )
    };
    let contacts = RefCell::new(Vec::new());
    let block = RcBlock::new(|contact: NonNull<CNContact>, _stop: NonNull<Bool>| {
        // SAFETY: Contacts invokes the block with a valid `CNContact` pointer for
        // the duration of the call.
        contacts
            .borrow_mut()
            .push(contact_from_objc(unsafe { contact.as_ref() }, true));
    });
    let mut error = None;
    // SAFETY: `error` is a live out-parameter for the duration of this
    // synchronous call and `block` only borrows `contacts`, which outlives it.
    let ok = unsafe {
        store.enumerateContactsWithFetchRequest_error_usingBlock(&request, Some(&mut error), &block)
    };
    if !ok {
        return Err(ContactsError::Platform(error.map_or_else(
            || "failed to fetch contacts".into(),
            |error| error.localizedDescription().to_string(),
        )));
    }
    drop(block);
    Ok(contacts.into_inner())
}

fn fetch_keys(personal_details: bool) -> Retained<KeyDescriptors> {
    // SAFETY: the `CNContact*Key` constants are immutable extern NSString
    // statics exposed by the Contacts framework.
    let keys = unsafe {
        let mut keys: Vec<&ProtocolObject<dyn CNKeyDescriptor>> = vec![
            ProtocolObject::from_ref(CNContactIdentifierKey),
            ProtocolObject::from_ref(CNContactGivenNameKey),
            ProtocolObject::from_ref(CNContactFamilyNameKey),
            ProtocolObject::from_ref(CNContactOrganizationNameKey),
            ProtocolObject::from_ref(CNContactPhoneNumbersKey),
            ProtocolObject::from_ref(CNContactEmailAddressesKey),
        ];
        if personal_details {
            keys.push(ProtocolObject::from_ref(CNContactBirthdayKey));
            keys.push(ProtocolObject::from_ref(CNContactNoteKey));
        }
        keys
    };
    NSArray::from_slice(&keys)
}

fn contact_from_objc(contact: &CNContact, personal_details: bool) -> Contact {
    // SAFETY: `contact` is a live `CNContact` whose keys were fetched above.
    let id = unsafe { contact.identifier() }.to_string();
    // SAFETY: `contact` is a live `CNContact` whose keys were fetched above.
    let given_name = unsafe { contact.givenName() }.to_string();
    // SAFETY: `contact` is a live `CNContact` whose keys were fetched above.
    let family_name = unsafe { contact.familyName() }.to_string();
    // SAFETY: `contact` is a live `CNContact` whose keys were fetched above.
    let organization = unsafe { contact.organizationName() }.to_string();
    // SAFETY: `contact` is a live `CNContact` whose phone numbers were fetched.
    let phone_numbers = unsafe { contact.phoneNumbers() }
        .iter()
        .map(|labeled| {
            // SAFETY: `labeled` is a live `CNLabeledValue<CNPhoneNumber>` element.
            let number = unsafe { labeled.value().stringValue().to_string() };
            // SAFETY: `labeled` is a live `CNLabeledValue<CNPhoneNumber>` element.
            let label = unsafe { labeled.label() }.map(|label| label.to_string());
            PhoneNumber { number, label }
        })
        .collect();
    // SAFETY: `contact` is a live `CNContact` whose email addresses were fetched.
    let email_addresses = unsafe { contact.emailAddresses() }
        .iter()
        .map(|labeled| {
            // SAFETY: `labeled` is a live `CNLabeledValue<NSString>` element.
            let address = unsafe { labeled.value().to_string() };
            // SAFETY: `labeled` is a live `CNLabeledValue<NSString>` element.
            let label = unsafe { labeled.label() }.map(|label| label.to_string());
            EmailAddress { address, label }
        })
        .collect();
    let (birthday, note) = if personal_details {
        // SAFETY: `contact` is a live `CNContact` whose birthday and note were
        // fetched.
        let birthday = unsafe { contact.birthday() }.map(|components| {
            let part = |value| {
                if value == NSDateComponentUndefined {
                    0
                } else {
                    value
                }
            };
            format!(
                "{}-{:02}-{:02}",
                part(components.year()),
                part(components.month()),
                part(components.day())
            )
            .parse::<crate::Date>()
            .ok()
        });
        // SAFETY: `contact` is a live `CNContact` whose note was fetched.
        let note = unsafe { contact.note() }.to_string();
        (birthday.flatten(), Some(note))
    } else {
        (None, None)
    };
    Contact {
        id,
        given_name: non_empty(given_name),
        family_name: non_empty(family_name),
        organization: non_empty(organization),
        phone_numbers,
        email_addresses,
        postal_addresses: Vec::new(),
        birthday,
        note: note.and_then(non_empty),
        thumbnail: None,
    }
}

fn created_contact(contact: &CNMutableContact) -> Contact {
    // SAFETY: `contact` is a live `CNMutableContact` saved by the request above.
    let id = unsafe { contact.identifier() }.to_string();
    // SAFETY: `contact` is a live `CNMutableContact` saved by the request above.
    let given_name = unsafe { contact.givenName() }.to_string();
    // SAFETY: `contact` is a live `CNMutableContact` saved by the request above.
    let family_name = unsafe { contact.familyName() }.to_string();
    // SAFETY: `contact` is a live `CNMutableContact` saved by the request above.
    let organization = unsafe { contact.organizationName() }.to_string();
    Contact {
        id,
        given_name: non_empty(given_name),
        family_name: non_empty(family_name),
        organization: non_empty(organization),
        phone_numbers: Vec::new(),
        email_addresses: Vec::new(),
        postal_addresses: Vec::new(),
        birthday: None,
        note: None,
        thumbnail: None,
    }
}

fn access_error_message(error: *mut NSError) -> String {
    // SAFETY: the completion handler may pass a nil error pointer; dereference
    // only when it is non-null, in which case the `NSError` is live.
    unsafe { error.as_ref() }.map_or_else(
        || "Permission denied".into(),
        |error| error.localizedDescription().to_string(),
    )
}

fn platform_error(error: &NSError) -> ContactsError {
    ContactsError::Platform(error.localizedDescription().to_string())
}

fn non_empty(value: String) -> Option<String> {
    (!value.is_empty()).then_some(value)
}
