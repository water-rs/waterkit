use async_trait::async_trait;
use serde::Serialize;
use windows::Win32::Foundation::HWND;
use windows::Win32::Networking::WindowsWebServices::{
    WEBAUTHN_ATTESTATION_CONVEYANCE_PREFERENCE_DIRECT,
    WEBAUTHN_ATTESTATION_CONVEYANCE_PREFERENCE_INDIRECT,
    WEBAUTHN_ATTESTATION_CONVEYANCE_PREFERENCE_NONE, WEBAUTHN_AUTHENTICATOR_ATTACHMENT_PLATFORM,
    WEBAUTHN_AUTHENTICATOR_GET_ASSERTION_OPTIONS,
    WEBAUTHN_AUTHENTICATOR_GET_ASSERTION_OPTIONS_VERSION_7,
    WEBAUTHN_AUTHENTICATOR_MAKE_CREDENTIAL_OPTIONS,
    WEBAUTHN_AUTHENTICATOR_MAKE_CREDENTIAL_OPTIONS_VERSION_7, WEBAUTHN_CLIENT_DATA,
    WEBAUTHN_CLIENT_DATA_CURRENT_VERSION, WEBAUTHN_COSE_CREDENTIAL_PARAMETER,
    WEBAUTHN_COSE_CREDENTIAL_PARAMETER_CURRENT_VERSION, WEBAUTHN_COSE_CREDENTIAL_PARAMETERS,
    WEBAUTHN_CREDENTIAL_EX, WEBAUTHN_CREDENTIAL_EX_CURRENT_VERSION, WEBAUTHN_CREDENTIAL_LIST,
    WEBAUTHN_CREDENTIAL_TYPE_PUBLIC_KEY, WEBAUTHN_CREDENTIALS,
    WEBAUTHN_ENTERPRISE_ATTESTATION_NONE, WEBAUTHN_ENTERPRISE_ATTESTATION_VENDOR_FACILITATED,
    WEBAUTHN_EXTENSIONS, WEBAUTHN_HASH_ALGORITHM_SHA_256, WEBAUTHN_RP_ENTITY_INFORMATION,
    WEBAUTHN_RP_ENTITY_INFORMATION_CURRENT_VERSION, WEBAUTHN_USER_ENTITY_INFORMATION,
    WEBAUTHN_USER_ENTITY_INFORMATION_CURRENT_VERSION,
    WEBAUTHN_USER_VERIFICATION_REQUIREMENT_DISCOURAGED,
    WEBAUTHN_USER_VERIFICATION_REQUIREMENT_PREFERRED,
    WEBAUTHN_USER_VERIFICATION_REQUIREMENT_REQUIRED, WebAuthNAuthenticatorGetAssertion,
    WebAuthNAuthenticatorMakeCredential, WebAuthNFreeAssertion, WebAuthNFreeCredentialAttestation,
    WebAuthNGetApiVersionNumber, WebAuthNGetErrorName,
    WebAuthNIsUserVerifyingPlatformAuthenticatorAvailable,
};
use windows::core::{BOOL, HSTRING, PCWSTR};

use crate::{
    AttestationPreference, AuthenticateOptions, AuthenticationResult, Availability, CredentialId,
    PasskeyError, RegisterOptions, RegistrationResult, UserVerificationRequirement,
    encode_base64url,
};

use super::PasskeyBackend;

pub struct PlatformBackend;

#[async_trait]
impl PasskeyBackend for PlatformBackend {
    async fn is_available(&self) -> Result<Availability, PasskeyError> {
        let api_version = unsafe {
            // SAFETY: This function has no preconditions and simply reports the OS WebAuthn API
            // version from `webauthn.dll`.
            WebAuthNGetApiVersionNumber()
        };
        if api_version == 0 {
            return Ok(Availability::unavailable());
        }

        let uvpa_available = unsafe {
            // SAFETY: This function has no caller preconditions besides running on supported
            // Windows versions. We guard with API version above.
            WebAuthNIsUserVerifyingPlatformAuthenticatorAvailable()
        };

        if matches!(uvpa_available, Ok(value) if value.as_bool()) {
            Ok(Availability::supported())
        } else {
            Ok(Availability::unavailable())
        }
    }

    #[allow(clippy::too_many_lines)]
    async fn register(
        &self,
        options: &RegisterOptions,
    ) -> Result<RegistrationResult, PasskeyError> {
        let rp_id = HSTRING::from(options.rp().id().as_str());
        let rp_name = HSTRING::from(options.rp().name());
        let user_name = HSTRING::from(options.user().name());
        let user_display_name = HSTRING::from(options.user().display_name());

        let mut user_id = options.user().id().as_bytes().to_vec();
        let user_id_len = to_u32_len(user_id.len(), "user id")?;

        let rp_information = WEBAUTHN_RP_ENTITY_INFORMATION {
            dwVersion: WEBAUTHN_RP_ENTITY_INFORMATION_CURRENT_VERSION,
            pwszId: PCWSTR::from_raw(rp_id.as_ptr()),
            pwszName: PCWSTR::from_raw(rp_name.as_ptr()),
            pwszIcon: PCWSTR::null(),
        };

        let user_information = WEBAUTHN_USER_ENTITY_INFORMATION {
            dwVersion: WEBAUTHN_USER_ENTITY_INFORMATION_CURRENT_VERSION,
            cbId: user_id_len,
            pbId: user_id.as_mut_ptr(),
            pwszName: PCWSTR::from_raw(user_name.as_ptr()),
            pwszIcon: PCWSTR::null(),
            pwszDisplayName: PCWSTR::from_raw(user_display_name.as_ptr()),
        };

        let mut cose_algorithms: Vec<WEBAUTHN_COSE_CREDENTIAL_PARAMETER> = options
            .pub_key_algorithms_ref()
            .iter()
            .map(|algorithm| WEBAUTHN_COSE_CREDENTIAL_PARAMETER {
                dwVersion: WEBAUTHN_COSE_CREDENTIAL_PARAMETER_CURRENT_VERSION,
                pwszCredentialType: WEBAUTHN_CREDENTIAL_TYPE_PUBLIC_KEY,
                lAlg: algorithm.cose_id(),
            })
            .collect();

        let cose_algorithms_len = to_u32_len(cose_algorithms.len(), "COSE algorithm list")?;
        let cose_parameters = WEBAUTHN_COSE_CREDENTIAL_PARAMETERS {
            cCredentialParameters: cose_algorithms_len,
            pCredentialParameters: cose_algorithms.as_mut_ptr(),
        };

        let mut excluded_credential_ids: Vec<Vec<u8>> = options
            .exclude_credentials_ref()
            .iter()
            .map(|credential| credential.id().as_bytes().to_vec())
            .collect();

        let mut excluded_credentials: Vec<WEBAUTHN_CREDENTIAL_EX> = excluded_credential_ids
            .iter_mut()
            .map(|credential_id| {
                let id_len = to_u32_len(credential_id.len(), "excluded credential id")?;
                Ok(WEBAUTHN_CREDENTIAL_EX {
                    dwVersion: WEBAUTHN_CREDENTIAL_EX_CURRENT_VERSION,
                    cbId: id_len,
                    pbId: credential_id.as_mut_ptr(),
                    pwszCredentialType: WEBAUTHN_CREDENTIAL_TYPE_PUBLIC_KEY,
                    dwTransports: 0,
                })
            })
            .collect::<Result<_, PasskeyError>>()?;

        let mut excluded_credential_ptrs: Vec<*mut WEBAUTHN_CREDENTIAL_EX> = excluded_credentials
            .iter_mut()
            .map(std::ptr::from_mut)
            .collect();

        let mut excluded_credential_list = WEBAUTHN_CREDENTIAL_LIST {
            cCredentials: to_u32_len(excluded_credential_ptrs.len(), "excluded credential list")?,
            ppCredentials: excluded_credential_ptrs.as_mut_ptr(),
        };

        let mut client_data_json = serialize_client_data_json(
            "webauthn.create",
            options.challenge().as_bytes(),
            options.rp().id().as_str(),
        )?;

        let client_data = WEBAUTHN_CLIENT_DATA {
            dwVersion: WEBAUTHN_CLIENT_DATA_CURRENT_VERSION,
            cbClientDataJSON: to_u32_len(client_data_json.len(), "clientDataJSON")?,
            pbClientDataJSON: client_data_json.as_mut_ptr(),
            pwszHashAlgId: WEBAUTHN_HASH_ALGORITHM_SHA_256,
        };

        let make_options = WEBAUTHN_AUTHENTICATOR_MAKE_CREDENTIAL_OPTIONS {
            dwVersion: WEBAUTHN_AUTHENTICATOR_MAKE_CREDENTIAL_OPTIONS_VERSION_7,
            dwTimeoutMilliseconds: options.timeout_ms_value().unwrap_or_default(),
            CredentialList: WEBAUTHN_CREDENTIALS::default(),
            Extensions: WEBAUTHN_EXTENSIONS::default(),
            dwAuthenticatorAttachment: WEBAUTHN_AUTHENTICATOR_ATTACHMENT_PLATFORM,
            bRequireResidentKey: bool_to_win(options.discoverable_value()),
            dwUserVerificationRequirement: map_user_verification(options.user_verification_value()),
            dwAttestationConveyancePreference: map_attestation(options.attestation_value()),
            dwFlags: 0,
            pCancellationId: std::ptr::null_mut(),
            pExcludeCredentialList: if excluded_credentials.is_empty() {
                std::ptr::null_mut()
            } else {
                std::ptr::from_mut(&mut excluded_credential_list)
            },
            dwEnterpriseAttestation: map_enterprise_attestation(options.attestation_value()),
            dwLargeBlobSupport: 0,
            bPreferResidentKey: bool_to_win(options.discoverable_value()),
            bBrowserInPrivateMode: BOOL(0),
            bEnablePrf: BOOL(0),
            pLinkedDevice: std::ptr::null_mut(),
            cbJsonExt: 0,
            pbJsonExt: std::ptr::null_mut(),
        };

        let attestation = unsafe {
            // SAFETY: All pointers in the argument structures reference memory owned in this
            // scope and live across the FFI call.
            WebAuthNAuthenticatorMakeCredential(
                HWND::default(),
                &raw const rp_information,
                &raw const user_information,
                &raw const cose_parameters,
                &raw const client_data,
                Some(&raw const make_options),
            )
        }
        .map_err(|error| map_windows_error(&error))?;

        let _attestation_guard = CredentialAttestationGuard(attestation.cast_const());
        let attestation_ref = unsafe {
            // SAFETY: The API returned a non-null pointer on success.
            attestation.as_ref()
        }
        .ok_or_else(|| {
            PasskeyError::Platform("windows returned null attestation pointer".into())
        })?;

        let credential_id = CredentialId::new(copy_required_bytes(
            attestation_ref.pbCredentialId,
            attestation_ref.cbCredentialId,
            "credential id",
        )?)?;
        let attestation_object = copy_required_bytes(
            attestation_ref.pbAttestationObject,
            attestation_ref.cbAttestationObject,
            "attestation object",
        )?;
        let authenticator_data = copy_optional_bytes(
            attestation_ref.pbAuthenticatorData,
            attestation_ref.cbAuthenticatorData,
            "authenticator data",
        )?;

        Ok(RegistrationResult::new(
            credential_id,
            attestation_object,
            client_data_json,
            authenticator_data,
            None,
        ))
    }

    #[allow(clippy::too_many_lines)]
    async fn authenticate(
        &self,
        options: &AuthenticateOptions,
    ) -> Result<AuthenticationResult, PasskeyError> {
        let rp_id = HSTRING::from(options.rp_id().as_str());

        let mut allowed_credential_ids: Vec<Vec<u8>> = options
            .allow_credentials_ref()
            .iter()
            .map(|credential| credential.id().as_bytes().to_vec())
            .collect();

        let mut allowed_credentials: Vec<WEBAUTHN_CREDENTIAL_EX> = allowed_credential_ids
            .iter_mut()
            .map(|credential_id| {
                let id_len = to_u32_len(credential_id.len(), "allowed credential id")?;
                Ok(WEBAUTHN_CREDENTIAL_EX {
                    dwVersion: WEBAUTHN_CREDENTIAL_EX_CURRENT_VERSION,
                    cbId: id_len,
                    pbId: credential_id.as_mut_ptr(),
                    pwszCredentialType: WEBAUTHN_CREDENTIAL_TYPE_PUBLIC_KEY,
                    dwTransports: 0,
                })
            })
            .collect::<Result<_, PasskeyError>>()?;

        let mut allowed_credential_ptrs: Vec<*mut WEBAUTHN_CREDENTIAL_EX> = allowed_credentials
            .iter_mut()
            .map(std::ptr::from_mut)
            .collect();

        let mut allowed_credential_list = WEBAUTHN_CREDENTIAL_LIST {
            cCredentials: to_u32_len(allowed_credential_ptrs.len(), "allowed credential list")?,
            ppCredentials: allowed_credential_ptrs.as_mut_ptr(),
        };

        let mut client_data_json = serialize_client_data_json(
            "webauthn.get",
            options.challenge().as_bytes(),
            options.rp_id().as_str(),
        )?;

        let client_data = WEBAUTHN_CLIENT_DATA {
            dwVersion: WEBAUTHN_CLIENT_DATA_CURRENT_VERSION,
            cbClientDataJSON: to_u32_len(client_data_json.len(), "clientDataJSON")?,
            pbClientDataJSON: client_data_json.as_mut_ptr(),
            pwszHashAlgId: WEBAUTHN_HASH_ALGORITHM_SHA_256,
        };

        let assertion_options = WEBAUTHN_AUTHENTICATOR_GET_ASSERTION_OPTIONS {
            dwVersion: WEBAUTHN_AUTHENTICATOR_GET_ASSERTION_OPTIONS_VERSION_7,
            dwTimeoutMilliseconds: options.timeout_ms_value().unwrap_or_default(),
            CredentialList: WEBAUTHN_CREDENTIALS::default(),
            Extensions: WEBAUTHN_EXTENSIONS::default(),
            dwAuthenticatorAttachment: WEBAUTHN_AUTHENTICATOR_ATTACHMENT_PLATFORM,
            dwUserVerificationRequirement: map_user_verification(options.user_verification_value()),
            dwFlags: 0,
            pwszU2fAppId: PCWSTR::null(),
            pbU2fAppId: std::ptr::null_mut(),
            pCancellationId: std::ptr::null_mut(),
            pAllowCredentialList: if allowed_credentials.is_empty() {
                std::ptr::null_mut()
            } else {
                std::ptr::from_mut(&mut allowed_credential_list)
            },
            dwCredLargeBlobOperation: 0,
            cbCredLargeBlob: 0,
            pbCredLargeBlob: std::ptr::null_mut(),
            pHmacSecretSaltValues: std::ptr::null_mut(),
            bBrowserInPrivateMode: BOOL(0),
            pLinkedDevice: std::ptr::null_mut(),
            bAutoFill: BOOL(0),
            cbJsonExt: 0,
            pbJsonExt: std::ptr::null_mut(),
        };

        let assertion = unsafe {
            // SAFETY: All pointers in the argument structures reference memory owned in this
            // scope and live across the FFI call.
            WebAuthNAuthenticatorGetAssertion(
                HWND::default(),
                &rp_id,
                &raw const client_data,
                Some(&raw const assertion_options),
            )
        }
        .map_err(|error| map_windows_error(&error))?;

        let _assertion_guard = AssertionGuard(assertion.cast_const());
        let assertion_ref = unsafe {
            // SAFETY: The API returned a non-null pointer on success.
            assertion.as_ref()
        }
        .ok_or_else(|| PasskeyError::Platform("windows returned null assertion pointer".into()))?;

        let credential_id = CredentialId::new(copy_required_bytes(
            assertion_ref.Credential.pbId,
            assertion_ref.Credential.cbId,
            "credential id",
        )?)?;
        let authenticator_data = copy_required_bytes(
            assertion_ref.pbAuthenticatorData,
            assertion_ref.cbAuthenticatorData,
            "authenticator data",
        )?;
        let signature = copy_required_bytes(
            assertion_ref.pbSignature,
            assertion_ref.cbSignature,
            "signature",
        )?;
        let user_handle = copy_optional_bytes(
            assertion_ref.pbUserId,
            assertion_ref.cbUserId,
            "user handle",
        )?;

        Ok(AuthenticationResult::new(
            credential_id,
            authenticator_data,
            client_data_json,
            signature,
            user_handle,
        ))
    }
}

struct CredentialAttestationGuard(
    *const windows::Win32::Networking::WindowsWebServices::WEBAUTHN_CREDENTIAL_ATTESTATION,
);

impl Drop for CredentialAttestationGuard {
    fn drop(&mut self) {
        unsafe {
            // SAFETY: Pointer comes from `WebAuthNAuthenticatorMakeCredential`.
            WebAuthNFreeCredentialAttestation(Some(self.0));
        }
    }
}

struct AssertionGuard(*const windows::Win32::Networking::WindowsWebServices::WEBAUTHN_ASSERTION);

impl Drop for AssertionGuard {
    fn drop(&mut self) {
        unsafe {
            // SAFETY: Pointer comes from `WebAuthNAuthenticatorGetAssertion`.
            WebAuthNFreeAssertion(self.0);
        }
    }
}

fn to_u32_len(len: usize, what: &str) -> Result<u32, PasskeyError> {
    u32::try_from(len)
        .map_err(|_| PasskeyError::OperationFailed(format!("{what} length exceeds u32 range")))
}

fn bool_to_win(value: bool) -> BOOL {
    BOOL(i32::from(value))
}

const fn map_attestation(attestation: AttestationPreference) -> u32 {
    match attestation {
        AttestationPreference::None => WEBAUTHN_ATTESTATION_CONVEYANCE_PREFERENCE_NONE,
        AttestationPreference::Indirect => WEBAUTHN_ATTESTATION_CONVEYANCE_PREFERENCE_INDIRECT,
        AttestationPreference::Direct | AttestationPreference::Enterprise => {
            WEBAUTHN_ATTESTATION_CONVEYANCE_PREFERENCE_DIRECT
        }
    }
}

const fn map_enterprise_attestation(attestation: AttestationPreference) -> u32 {
    match attestation {
        AttestationPreference::Enterprise => WEBAUTHN_ENTERPRISE_ATTESTATION_VENDOR_FACILITATED,
        _ => WEBAUTHN_ENTERPRISE_ATTESTATION_NONE,
    }
}

const fn map_user_verification(user_verification: UserVerificationRequirement) -> u32 {
    match user_verification {
        UserVerificationRequirement::Required => WEBAUTHN_USER_VERIFICATION_REQUIREMENT_REQUIRED,
        UserVerificationRequirement::Preferred => WEBAUTHN_USER_VERIFICATION_REQUIREMENT_PREFERRED,
        UserVerificationRequirement::Discouraged => {
            WEBAUTHN_USER_VERIFICATION_REQUIREMENT_DISCOURAGED
        }
    }
}

fn copy_required_bytes(pointer: *mut u8, length: u32, what: &str) -> Result<Vec<u8>, PasskeyError> {
    let bytes = copy_optional_bytes(pointer, length, what)?.ok_or_else(|| {
        PasskeyError::Platform(format!("windows returned missing `{what}` bytes"))
    })?;
    if bytes.is_empty() {
        return Err(PasskeyError::Platform(format!(
            "windows returned empty `{what}` bytes"
        )));
    }
    Ok(bytes)
}

fn copy_optional_bytes(
    pointer: *mut u8,
    length: u32,
    what: &str,
) -> Result<Option<Vec<u8>>, PasskeyError> {
    if length == 0 {
        return Ok(None);
    }
    if pointer.is_null() {
        return Err(PasskeyError::Platform(format!(
            "windows returned null pointer for `{what}` with non-zero length"
        )));
    }

    let length = usize::try_from(length)
        .map_err(|_| PasskeyError::OperationFailed(format!("{what} length exceeds usize range")))?;

    let bytes = unsafe {
        // SAFETY: The pointer and length were validated by the caller from API-provided values.
        std::slice::from_raw_parts(pointer.cast_const(), length)
    };
    Ok(Some(bytes.to_vec()))
}

fn map_windows_error(error: &windows::core::Error) -> PasskeyError {
    let hr = error.code();
    let error_name = unsafe {
        // SAFETY: This converts an HRESULT to a static name string.
        WebAuthNGetErrorName(hr)
    };

    let error_name = if error_name.is_null() {
        "unknown".to_owned()
    } else {
        unsafe {
            // SAFETY: `WebAuthNGetErrorName` returns a null-terminated UTF-16 string.
            error_name
                .to_string()
                .unwrap_or_else(|_| "unknown".to_owned())
        }
    };

    let hr_u32 = u32::from_ne_bytes(hr.0.to_ne_bytes());
    PasskeyError::from_platform_error(format!("{error_name} ({hr_u32:#010x}): {error}"))
}

#[derive(Serialize)]
struct ClientDataWire {
    #[serde(rename = "type")]
    operation_type: &'static str,
    challenge: String,
    origin: String,
    #[serde(rename = "crossOrigin")]
    cross_origin: bool,
}

fn serialize_client_data_json(
    operation_type: &'static str,
    challenge: &[u8],
    rp_id: &str,
) -> Result<Vec<u8>, PasskeyError> {
    let payload = ClientDataWire {
        operation_type,
        challenge: encode_base64url(challenge),
        origin: format!("https://{rp_id}"),
        cross_origin: false,
    };

    serde_json::to_vec(&payload).map_err(|error| {
        PasskeyError::OperationFailed(format!(
            "failed to serialize windows clientDataJSON: {error}"
        ))
    })
}
