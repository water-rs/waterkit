use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use zbus::names::BusName;
use zbus::zvariant::{OwnedValue, Value};

use crate::{
    AttestationPreference, AuthenticateOptions, AuthenticationResult, Availability, CredentialId,
    PasskeyError, RegisterOptions, RegistrationResult, UserVerificationRequirement,
    decode_base64url, decode_optional_base64url, encode_base64url,
};

use super::PasskeyBackend;

const CREDENTIALSD_BUS_NAME: &str = "xyz.iinuwa.credentialsd.Credentials";
const CREDENTIALSD_PATH: &str = "/xyz/iinuwa/credentialsd/Credentials";
const CREDENTIALSD_INTERFACE: &str = "xyz.iinuwa.credentialsd.Credentials1";

pub struct PlatformBackend;

#[async_trait]
impl PasskeyBackend for PlatformBackend {
    async fn is_available(&self) -> Result<Availability, PasskeyError> {
        let connection = zbus::Connection::session().await.map_err(|error| {
            PasskeyError::Platform(format!("failed to connect to session bus: {error}"))
        })?;

        let dbus = zbus::fdo::DBusProxy::new(&connection)
            .await
            .map_err(|error| {
                PasskeyError::Platform(format!("failed to create D-Bus proxy: {error}"))
            })?;

        let service_name = BusName::try_from(CREDENTIALSD_BUS_NAME).map_err(|error| {
            PasskeyError::Platform(format!(
                "invalid credentials service bus name `{CREDENTIALSD_BUS_NAME}`: {error}"
            ))
        })?;
        let has_portal = dbus.name_has_owner(service_name).await.map_err(|error| {
            PasskeyError::Platform(format!(
                "failed to query credentials service ownership: {error}"
            ))
        })?;

        if has_portal {
            Ok(Availability::supported())
        } else {
            Ok(Availability::unavailable())
        }
    }

    async fn register(
        &self,
        options: &RegisterOptions,
    ) -> Result<RegistrationResult, PasskeyError> {
        let connection = connect_session_bus().await?;
        let request = build_create_credential_request(options)?;
        let response = call_gateway_method(&connection, "CreateCredential", request).await?;
        parse_registration_gateway_response(response)
    }

    async fn authenticate(
        &self,
        options: &AuthenticateOptions,
    ) -> Result<AuthenticationResult, PasskeyError> {
        let connection = connect_session_bus().await?;
        let request = build_get_credential_request(options)?;
        let response = call_gateway_method(&connection, "GetCredential", request).await?;
        parse_authentication_gateway_response(response)
    }
}

async fn connect_session_bus() -> Result<zbus::Connection, PasskeyError> {
    zbus::Connection::session().await.map_err(|error| {
        PasskeyError::Platform(format!("failed to connect to session bus: {error}"))
    })
}

fn origin_from_rp_id(rp_id: &str) -> String {
    format!("https://{rp_id}")
}

fn build_create_credential_request(
    options: &RegisterOptions,
) -> Result<HashMap<String, OwnedValue>, PasskeyError> {
    let request_json = serde_json::to_string(&PublicKeyCredentialCreationOptionsWire::from(
        options,
    ))
    .map_err(|error| {
        PasskeyError::OperationFailed(format!(
            "failed to serialize linux create credential request JSON: {error}"
        ))
    })?;

    let mut public_key = HashMap::new();
    public_key.insert(
        "request_json".to_owned(),
        owned_string_value(&request_json)?,
    );

    let mut request = HashMap::new();
    request.insert("type".to_owned(), owned_string_value("publicKey")?);
    request.insert(
        "origin".to_owned(),
        owned_string_value(origin_from_rp_id(options.rp().id().as_str()))?,
    );
    request.insert("is_same_origin".to_owned(), OwnedValue::from(true));
    request.insert("publicKey".to_owned(), OwnedValue::from(public_key));
    Ok(request)
}

fn build_get_credential_request(
    options: &AuthenticateOptions,
) -> Result<HashMap<String, OwnedValue>, PasskeyError> {
    let request_json = serde_json::to_string(&PublicKeyCredentialRequestOptionsWire::from(options))
        .map_err(|error| {
            PasskeyError::OperationFailed(format!(
                "failed to serialize linux get credential request JSON: {error}"
            ))
        })?;

    let mut public_key = HashMap::new();
    public_key.insert(
        "request_json".to_owned(),
        owned_string_value(&request_json)?,
    );

    let mut request = HashMap::new();
    request.insert("type".to_owned(), owned_string_value("publicKey")?);
    request.insert(
        "origin".to_owned(),
        owned_string_value(origin_from_rp_id(options.rp_id().as_str()))?,
    );
    request.insert("is_same_origin".to_owned(), OwnedValue::from(true));
    request.insert("publicKey".to_owned(), OwnedValue::from(public_key));
    Ok(request)
}

async fn call_gateway_method(
    connection: &zbus::Connection,
    method: &str,
    request: HashMap<String, OwnedValue>,
) -> Result<HashMap<String, OwnedValue>, PasskeyError> {
    let proxy = zbus::Proxy::new(
        connection,
        CREDENTIALSD_BUS_NAME,
        CREDENTIALSD_PATH,
        CREDENTIALSD_INTERFACE,
    )
    .await
    .map_err(|error| map_dbus_error(&error))?;

    let message = proxy
        .call_method(method, &("", request))
        .await
        .map_err(|error| map_dbus_error(&error))?;

    message.body().deserialize().map_err(|error| {
        PasskeyError::Platform(format!(
            "failed to deserialize credentials service response: {error}"
        ))
    })
}

fn parse_registration_gateway_response(
    mut response: HashMap<String, OwnedValue>,
) -> Result<RegistrationResult, PasskeyError> {
    validate_response_type(&mut response)?;
    let mut public_key = extract_map(&mut response, "public_key")?;
    let registration_json = extract_string(&mut public_key, "registration_response_json")?;

    let wire: RegistrationCredentialResponseWire = serde_json::from_str(&registration_json)
        .map_err(|error| {
            PasskeyError::OperationFailed(format!(
                "failed to parse registration response JSON from linux gateway: {error}"
            ))
        })?;

    let credential_id = CredentialId::new(decode_base64url(&wire.raw_id, "rawId")?)?;
    let attestation_object = decode_base64url(
        &wire.response.attestation_object,
        "response.attestationObject",
    )?;
    let client_data_json =
        decode_base64url(&wire.response.client_data_json, "response.clientDataJSON")?;
    let authenticator_data = decode_optional_base64url(
        wire.response.authenticator_data,
        "response.authenticatorData",
    )?;
    let public_key_cose =
        decode_optional_base64url(wire.response.public_key, "response.publicKey")?;

    Ok(RegistrationResult::new(
        credential_id,
        attestation_object,
        client_data_json,
        authenticator_data,
        public_key_cose,
    ))
}

fn parse_authentication_gateway_response(
    mut response: HashMap<String, OwnedValue>,
) -> Result<AuthenticationResult, PasskeyError> {
    validate_response_type(&mut response)?;
    let mut public_key = extract_map(&mut response, "public_key")?;
    let authentication_json = extract_string(&mut public_key, "authentication_response_json")?;

    let wire: AuthenticationCredentialResponseWire = serde_json::from_str(&authentication_json)
        .map_err(|error| {
            PasskeyError::OperationFailed(format!(
                "failed to parse authentication response JSON from linux gateway: {error}"
            ))
        })?;

    let credential_id = CredentialId::new(decode_base64url(&wire.raw_id, "rawId")?)?;
    let authenticator_data = decode_base64url(
        &wire.response.authenticator_data,
        "response.authenticatorData",
    )?;
    let client_data_json =
        decode_base64url(&wire.response.client_data_json, "response.clientDataJSON")?;
    let signature = decode_base64url(&wire.response.signature, "response.signature")?;
    let user_handle = decode_optional_base64url(wire.response.user_handle, "response.userHandle")?;

    Ok(AuthenticationResult::new(
        credential_id,
        authenticator_data,
        client_data_json,
        signature,
        user_handle,
    ))
}

fn validate_response_type(response: &mut HashMap<String, OwnedValue>) -> Result<(), PasskeyError> {
    let credential_type = extract_string(response, "type")?;
    if credential_type != "public-key" {
        return Err(PasskeyError::Platform(format!(
            "unexpected credential type from linux gateway: {credential_type}"
        )));
    }
    Ok(())
}

fn extract_string(
    values: &mut HashMap<String, OwnedValue>,
    field: &str,
) -> Result<String, PasskeyError> {
    let value = values.remove(field).ok_or_else(|| {
        PasskeyError::Platform(format!(
            "missing `{field}` field in linux credentials gateway response"
        ))
    })?;

    let value = unwrap_variant(Value::from(value));
    String::try_from(value).map_err(|error| {
        PasskeyError::Platform(format!(
            "failed to decode `{field}` as string in linux credentials gateway response: {error}"
        ))
    })
}

fn extract_map(
    values: &mut HashMap<String, OwnedValue>,
    field: &str,
) -> Result<HashMap<String, OwnedValue>, PasskeyError> {
    let value = values.remove(field).ok_or_else(|| {
        PasskeyError::Platform(format!(
            "missing `{field}` field in linux credentials gateway response"
        ))
    })?;

    let value = unwrap_variant(Value::from(value));
    HashMap::<String, OwnedValue>::try_from(value).map_err(|error| {
        PasskeyError::Platform(format!(
            "failed to decode `{field}` as map in linux credentials gateway response: {error}"
        ))
    })
}

fn unwrap_variant(value: Value<'static>) -> Value<'static> {
    if let Value::Value(inner) = value {
        *inner
    } else {
        value
    }
}

fn map_dbus_error(error: &zbus::Error) -> PasskeyError {
    if let zbus::Error::MethodError(name, detail, _) = error {
        let error_name = name.as_str();
        if error_name.ends_with(".AbortError") {
            return PasskeyError::Cancelled;
        }
        if error_name.ends_with(".NotSupportedError") {
            return PasskeyError::NotAvailable;
        }
        if error_name.ends_with(".TypeError") {
            return PasskeyError::InvalidInput(detail.clone().unwrap_or_else(|| {
                "invalid request passed to linux credentials service".to_owned()
            }));
        }
        if error_name.ends_with(".NotAllowedError") {
            return PasskeyError::Cancelled;
        }
    }

    PasskeyError::from_platform_error(format!(
        "linux credentials service D-Bus call failed: {error}"
    ))
}

fn owned_string_value(value: impl AsRef<str>) -> Result<OwnedValue, PasskeyError> {
    Value::from(value.as_ref()).try_into().map_err(|error| {
        PasskeyError::Platform(format!("failed to encode D-Bus string value: {error}"))
    })
}

#[derive(Debug, Serialize)]
struct PublicKeyCredentialCreationOptionsWire {
    challenge: String,
    rp: CreationRelyingPartyWire,
    user: CreationUserWire,
    #[serde(rename = "pubKeyCredParams")]
    pub_key_cred_params: Vec<CreationPublicKeyParamWire>,
    #[serde(skip_serializing_if = "Option::is_none")]
    timeout: Option<u32>,
    attestation: &'static str,
    #[serde(rename = "excludeCredentials")]
    exclude_credentials: Vec<CreationCredentialDescriptorWire>,
    #[serde(rename = "authenticatorSelection")]
    authenticator_selection: AuthenticatorSelectionWire,
}

#[derive(Debug, Serialize)]
struct CreationRelyingPartyWire {
    id: String,
    name: String,
}

#[derive(Debug, Serialize)]
struct CreationUserWire {
    id: String,
    name: String,
    #[serde(rename = "displayName")]
    display_name: String,
}

#[derive(Debug, Serialize)]
struct CreationPublicKeyParamWire {
    #[serde(rename = "type")]
    credential_type: &'static str,
    alg: i32,
}

#[derive(Debug, Serialize)]
struct CreationCredentialDescriptorWire {
    #[serde(rename = "type")]
    credential_type: &'static str,
    id: String,
}

#[derive(Debug, Serialize)]
struct AuthenticatorSelectionWire {
    #[serde(rename = "residentKey")]
    resident_key: &'static str,
    #[serde(rename = "requireResidentKey")]
    require_resident_key: bool,
    #[serde(rename = "userVerification")]
    user_verification: &'static str,
}

impl From<&RegisterOptions> for PublicKeyCredentialCreationOptionsWire {
    fn from(options: &RegisterOptions) -> Self {
        let pub_key_cred_params = options
            .pub_key_algorithms_ref()
            .iter()
            .map(|algorithm| CreationPublicKeyParamWire {
                credential_type: "public-key",
                alg: algorithm.cose_id(),
            })
            .collect();

        let exclude_credentials = options
            .exclude_credentials_ref()
            .iter()
            .map(|credential| CreationCredentialDescriptorWire {
                credential_type: "public-key",
                id: encode_base64url(credential.id().as_bytes()),
            })
            .collect();

        Self {
            challenge: encode_base64url(options.challenge().as_bytes()),
            rp: CreationRelyingPartyWire {
                id: options.rp().id().as_str().to_owned(),
                name: options.rp().name().to_owned(),
            },
            user: CreationUserWire {
                id: encode_base64url(options.user().id().as_bytes()),
                name: options.user().name().to_owned(),
                display_name: options.user().display_name().to_owned(),
            },
            pub_key_cred_params,
            timeout: options.timeout_ms_value(),
            attestation: map_attestation(options.attestation_value()),
            exclude_credentials,
            authenticator_selection: AuthenticatorSelectionWire {
                resident_key: if options.discoverable_value() {
                    "required"
                } else {
                    "discouraged"
                },
                require_resident_key: options.discoverable_value(),
                user_verification: map_user_verification(options.user_verification_value()),
            },
        }
    }
}

#[derive(Debug, Serialize)]
struct PublicKeyCredentialRequestOptionsWire {
    challenge: String,
    #[serde(rename = "rpId")]
    rp_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    timeout: Option<u32>,
    #[serde(rename = "userVerification")]
    user_verification: &'static str,
    #[serde(rename = "allowCredentials")]
    allow_credentials: Vec<RequestCredentialDescriptorWire>,
}

#[derive(Debug, Serialize)]
struct RequestCredentialDescriptorWire {
    #[serde(rename = "type")]
    credential_type: &'static str,
    id: String,
}

impl From<&AuthenticateOptions> for PublicKeyCredentialRequestOptionsWire {
    fn from(options: &AuthenticateOptions) -> Self {
        let allow_credentials = options
            .allow_credentials_ref()
            .iter()
            .map(|credential| RequestCredentialDescriptorWire {
                credential_type: "public-key",
                id: encode_base64url(credential.id().as_bytes()),
            })
            .collect();

        Self {
            challenge: encode_base64url(options.challenge().as_bytes()),
            rp_id: options.rp_id().as_str().to_owned(),
            timeout: options.timeout_ms_value(),
            user_verification: map_user_verification(options.user_verification_value()),
            allow_credentials,
        }
    }
}

const fn map_attestation(attestation: AttestationPreference) -> &'static str {
    match attestation {
        AttestationPreference::None => "none",
        AttestationPreference::Indirect => "indirect",
        AttestationPreference::Direct => "direct",
        AttestationPreference::Enterprise => "enterprise",
    }
}

const fn map_user_verification(user_verification: UserVerificationRequirement) -> &'static str {
    match user_verification {
        UserVerificationRequirement::Required => "required",
        UserVerificationRequirement::Preferred => "preferred",
        UserVerificationRequirement::Discouraged => "discouraged",
    }
}

#[derive(Debug, Deserialize)]
struct RegistrationCredentialResponseWire {
    #[serde(rename = "rawId")]
    raw_id: String,
    response: RegistrationCredentialResponseBodyWire,
}

#[derive(Debug, Deserialize)]
struct RegistrationCredentialResponseBodyWire {
    #[serde(rename = "attestationObject")]
    attestation_object: String,
    #[serde(rename = "clientDataJSON")]
    client_data_json: String,
    #[serde(rename = "authenticatorData")]
    authenticator_data: Option<String>,
    #[serde(rename = "publicKey")]
    public_key: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AuthenticationCredentialResponseWire {
    #[serde(rename = "rawId")]
    raw_id: String,
    response: AuthenticationCredentialResponseBodyWire,
}

#[derive(Debug, Deserialize)]
struct AuthenticationCredentialResponseBodyWire {
    #[serde(rename = "authenticatorData")]
    authenticator_data: String,
    #[serde(rename = "clientDataJSON")]
    client_data_json: String,
    signature: String,
    #[serde(rename = "userHandle")]
    user_handle: Option<String>,
}
