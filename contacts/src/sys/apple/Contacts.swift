import Foundation
import Contacts

func contacts_fetch_all(callback: __private__RustFnOnceCallbackStringStringNoRet) {
    let store = CNContactStore()
    store.requestAccess(for: .contacts) { granted, error in
        if !granted {
            callback.call("", error?.localizedDescription ?? "Permission denied")
            return
        }
        let keys: [CNKeyDescriptor] = [
            CNContactIdentifierKey as CNKeyDescriptor,
            CNContactGivenNameKey as CNKeyDescriptor,
            CNContactFamilyNameKey as CNKeyDescriptor,
            CNContactOrganizationNameKey as CNKeyDescriptor,
            CNContactPhoneNumbersKey as CNKeyDescriptor,
            CNContactEmailAddressesKey as CNKeyDescriptor,
            CNContactBirthdayKey as CNKeyDescriptor,
            CNContactNoteKey as CNKeyDescriptor,
        ]
        let request = CNContactFetchRequest(keysToFetch: keys)
        var result = ""
        do {
            try store.enumerateContacts(with: request) { contact, _ in
                let phones = contact.phoneNumbers.map { $0.value.stringValue }.joined(separator: ",")
                let emails = contact.emailAddresses.map { $0.value as String }.joined(separator: ",")
                let birthday = contact.birthday.map { "\($0.year ?? 0)-\(String(format: "%02d", $0.month ?? 0))-\(String(format: "%02d", $0.day ?? 0))" } ?? ""
                let line = "\(contact.identifier)\t\(contact.givenName)\t\(contact.familyName)\t\(contact.organizationName)\t\(phones)\t\(emails)\t\(birthday)\t\(contact.note)"
                if !result.isEmpty { result += "\n" }
                result += line
            }
            callback.call(result, "")
        } catch {
            callback.call("", error.localizedDescription)
        }
    }
}

func contacts_search(query: RustStr, callback: __private__RustFnOnceCallbackStringStringNoRet) {
    let queryStr = query.toString()
    let store = CNContactStore()
    let keys: [CNKeyDescriptor] = [
        CNContactIdentifierKey as CNKeyDescriptor,
        CNContactGivenNameKey as CNKeyDescriptor,
        CNContactFamilyNameKey as CNKeyDescriptor,
        CNContactOrganizationNameKey as CNKeyDescriptor,
        CNContactPhoneNumbersKey as CNKeyDescriptor,
        CNContactEmailAddressesKey as CNKeyDescriptor,
    ]
    let predicate = CNContact.predicateForContacts(matchingName: queryStr)
    do {
        let contacts = try store.unifiedContacts(matching: predicate, keysToFetch: keys)
        var result = ""
        for contact in contacts {
            let phones = contact.phoneNumbers.map { $0.value.stringValue }.joined(separator: ",")
            let emails = contact.emailAddresses.map { $0.value as String }.joined(separator: ",")
            let line = "\(contact.identifier)\t\(contact.givenName)\t\(contact.familyName)\t\(contact.organizationName)\t\(phones)\t\(emails)\t\t"
            if !result.isEmpty { result += "\n" }
            result += line
        }
        callback.call(result, "")
    } catch {
        callback.call("", error.localizedDescription)
    }
}

func contacts_get(id: RustStr, callback: __private__RustFnOnceCallbackStringStringNoRet) {
    let idStr = id.toString()
    let store = CNContactStore()
    let keys: [CNKeyDescriptor] = [
        CNContactIdentifierKey as CNKeyDescriptor,
        CNContactGivenNameKey as CNKeyDescriptor,
        CNContactFamilyNameKey as CNKeyDescriptor,
        CNContactOrganizationNameKey as CNKeyDescriptor,
        CNContactPhoneNumbersKey as CNKeyDescriptor,
        CNContactEmailAddressesKey as CNKeyDescriptor,
        CNContactBirthdayKey as CNKeyDescriptor,
        CNContactNoteKey as CNKeyDescriptor,
    ]
    do {
        let contact = try store.unifiedContact(withIdentifier: idStr, keysToFetch: keys)
        let phones = contact.phoneNumbers.map { $0.value.stringValue }.joined(separator: ",")
        let emails = contact.emailAddresses.map { $0.value as String }.joined(separator: ",")
        let birthday = contact.birthday.map { "\($0.year ?? 0)-\(String(format: "%02d", $0.month ?? 0))-\(String(format: "%02d", $0.day ?? 0))" } ?? ""
        let line = "\(contact.identifier)\t\(contact.givenName)\t\(contact.familyName)\t\(contact.organizationName)\t\(phones)\t\(emails)\t\(birthday)\t\(contact.note)"
        callback.call(line, "")
    } catch {
        callback.call("", error.localizedDescription)
    }
}

func contacts_create(json: RustStr, callback: __private__RustFnOnceCallbackStringStringNoRet) {
    let jsonStr = json.toString()
    let parts = jsonStr.split(separator: "\t", omittingEmptySubsequences: false).map(String.init)
    let contact = CNMutableContact()
    if parts.count > 0 { contact.givenName = parts[0] }
    if parts.count > 1 { contact.familyName = parts[1] }
    if parts.count > 2 { contact.organizationName = parts[2] }
    if parts.count > 3 {
        contact.phoneNumbers = parts[3].split(separator: ",").map {
            CNLabeledValue(label: CNLabelPhoneNumberMobile, value: CNPhoneNumber(stringValue: String($0)))
        }
    }
    if parts.count > 4 {
        contact.emailAddresses = parts[4].split(separator: ",").map {
            CNLabeledValue(label: CNLabelHome, value: String($0) as NSString)
        }
    }
    if parts.count > 6 && !parts[6].isEmpty {
        contact.note = parts[6]
    }

    let store = CNContactStore()
    let saveRequest = CNSaveRequest()
    saveRequest.add(contact, toContainerWithIdentifier: nil)
    do {
        try store.execute(saveRequest)
        let line = "\(contact.identifier)\t\(contact.givenName)\t\(contact.familyName)\t\(contact.organizationName)\t\t\t\t"
        callback.call(line, "")
    } catch {
        callback.call("", error.localizedDescription)
    }
}

func contacts_delete(id: RustStr, callback: __private__RustFnOnceCallbackStringNoRet) {
    let idStr = id.toString()
    let store = CNContactStore()
    let keys: [CNKeyDescriptor] = [CNContactIdentifierKey as CNKeyDescriptor]
    do {
        let contact = try store.unifiedContact(withIdentifier: idStr, keysToFetch: keys)
        let mutable = contact.mutableCopy() as! CNMutableContact
        let saveRequest = CNSaveRequest()
        saveRequest.delete(mutable)
        try store.execute(saveRequest)
        callback.call("")
    } catch {
        callback.call(error.localizedDescription)
    }
}
