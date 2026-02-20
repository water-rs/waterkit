//! Cross-platform passkey registration and authentication APIs.
//!
//! The crate provides two ergonomic entry points:
//! - [`PasskeyClient`] for native-SDK-like flows with reusable defaults
//! - top-level async functions for direct per-call usage

#![warn(missing_docs)]

mod error;
mod sys;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use sys::{PasskeyBackend, platform_backend};

pub use error::PasskeyError;

/// Base64url-encodes bytes without padding.
pub(crate) fn encode_base64url(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Decodes a base64url string without padding.
///
/// # Errors
/// Returns [`PasskeyError::OperationFailed`] when decoding fails.
pub(crate) fn decode_base64url(value: &str, field: &str) -> Result<Vec<u8>, PasskeyError> {
    URL_SAFE_NO_PAD.decode(value).map_err(|error| {
        PasskeyError::OperationFailed(format!("failed to decode `{field}` as base64url: {error}"))
    })
}

/// Decodes an optional base64url string.
///
/// # Errors
/// Returns [`PasskeyError::OperationFailed`] when decoding fails.
pub(crate) fn decode_optional_base64url(
    value: Option<String>,
    field: &str,
) -> Result<Option<Vec<u8>>, PasskeyError> {
    value.map_or(Ok(None), |encoded| {
        decode_base64url(&encoded, field).map(Some)
    })
}

/// A validated relying-party identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RpId(String);

impl RpId {
    /// Creates a new relying-party identifier.
    ///
    /// # Errors
    /// Returns [`PasskeyError::InvalidInput`] if the identifier is empty or malformed.
    pub fn new(value: impl Into<String>) -> Result<Self, PasskeyError> {
        let value = value.into();
        validate_non_empty("rp id", &value)?;
        if value.starts_with('.') || value.ends_with('.') {
            return Err(PasskeyError::InvalidInput(
                "rp id must not start or end with '.'".into(),
            ));
        }
        if value.contains("..") {
            return Err(PasskeyError::InvalidInput(
                "rp id must not contain '..'".into(),
            ));
        }
        Ok(Self(value))
    }

    /// Borrows the underlying RP ID string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A non-empty challenge payload.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Challenge(Vec<u8>);

impl Challenge {
    /// Creates a challenge from bytes.
    ///
    /// # Errors
    /// Returns [`PasskeyError::InvalidInput`] when bytes are empty.
    pub fn new(bytes: impl Into<Vec<u8>>) -> Result<Self, PasskeyError> {
        let bytes = bytes.into();
        if bytes.is_empty() {
            return Err(PasskeyError::InvalidInput(
                "challenge must not be empty".into(),
            ));
        }
        Ok(Self(bytes))
    }

    /// Borrows challenge bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

/// A non-empty user identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct UserId(Vec<u8>);

impl UserId {
    /// Creates a user identifier from bytes.
    ///
    /// # Errors
    /// Returns [`PasskeyError::InvalidInput`] when bytes are empty.
    pub fn new(bytes: impl Into<Vec<u8>>) -> Result<Self, PasskeyError> {
        let bytes = bytes.into();
        if bytes.is_empty() {
            return Err(PasskeyError::InvalidInput(
                "user id must not be empty".into(),
            ));
        }
        Ok(Self(bytes))
    }

    /// Borrows user identifier bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

/// A non-empty credential identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CredentialId(Vec<u8>);

impl CredentialId {
    /// Creates a credential identifier from bytes.
    ///
    /// # Errors
    /// Returns [`PasskeyError::InvalidInput`] when bytes are empty.
    pub fn new(bytes: impl Into<Vec<u8>>) -> Result<Self, PasskeyError> {
        let bytes = bytes.into();
        if bytes.is_empty() {
            return Err(PasskeyError::InvalidInput(
                "credential id must not be empty".into(),
            ));
        }
        Ok(Self(bytes))
    }

    /// Borrows credential identifier bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

/// Relying-party identity for a passkey ceremony.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelyingParty {
    id: RpId,
    name: String,
}

impl RelyingParty {
    /// Creates a relying-party descriptor.
    ///
    /// # Errors
    /// Returns [`PasskeyError::InvalidInput`] when fields are empty or malformed.
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Result<Self, PasskeyError> {
        let id = RpId::new(id)?;
        let name = name.into();
        validate_non_empty("rp name", &name)?;
        Ok(Self { id, name })
    }

    /// Borrows relying-party ID.
    #[must_use]
    pub const fn id(&self) -> &RpId {
        &self.id
    }

    /// Borrows relying-party display name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// User metadata for passkey registration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserEntity {
    id: UserId,
    name: String,
    display_name: String,
}

impl UserEntity {
    /// Creates a user entity.
    ///
    /// # Errors
    /// Returns [`PasskeyError::InvalidInput`] when `name` or `display_name` are empty.
    pub fn new(
        id: UserId,
        name: impl Into<String>,
        display_name: impl Into<String>,
    ) -> Result<Self, PasskeyError> {
        let name = name.into();
        let display_name = display_name.into();
        validate_non_empty("user name", &name)?;
        validate_non_empty("user display name", &display_name)?;

        Ok(Self {
            id,
            name,
            display_name,
        })
    }

    /// Borrows user ID.
    #[must_use]
    pub const fn id(&self) -> &UserId {
        &self.id
    }

    /// Borrows user name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Borrows user display name.
    #[must_use]
    pub fn display_name(&self) -> &str {
        &self.display_name
    }
}

/// Credential descriptor for allow/exclude lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialDescriptor {
    id: CredentialId,
}

impl CredentialDescriptor {
    /// Creates a credential descriptor.
    #[must_use]
    pub const fn new(id: CredentialId) -> Self {
        Self { id }
    }

    /// Borrows credential ID.
    #[must_use]
    pub const fn id(&self) -> &CredentialId {
        &self.id
    }
}

/// COSE algorithm identifiers used in `WebAuthn` registration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicKeyAlgorithm {
    /// ECDSA with SHA-256 (`-7`).
    Es256,
    /// RSASSA-PKCS1-v1_5 with SHA-256 (`-257`).
    Rs256,
    /// `EdDSA` (`-8`).
    EdDsa,
}

impl PublicKeyAlgorithm {
    pub(crate) const fn cose_id(self) -> i32 {
        match self {
            Self::Es256 => -7,
            Self::Rs256 => -257,
            Self::EdDsa => -8,
        }
    }
}

/// Attestation preference for registration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AttestationPreference {
    /// Do not request attestation statement.
    #[default]
    None,
    /// Indirect attestation if available.
    Indirect,
    /// Direct attestation.
    Direct,
    /// Enterprise attestation where permitted.
    Enterprise,
}

impl AttestationPreference {
    pub(crate) const fn as_wire(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Indirect => "indirect",
            Self::Direct => "direct",
            Self::Enterprise => "enterprise",
        }
    }
}

/// User verification requirement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UserVerificationRequirement {
    /// User verification is mandatory.
    #[default]
    Required,
    /// User verification is preferred.
    Preferred,
    /// User verification is discouraged.
    Discouraged,
}

impl UserVerificationRequirement {
    pub(crate) const fn as_wire(self) -> &'static str {
        match self {
            Self::Required => "required",
            Self::Preferred => "preferred",
            Self::Discouraged => "discouraged",
        }
    }
}

/// Platform passkey capability flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Availability {
    /// Whether passkeys are supported on this platform/runtime.
    pub is_platform_supported: bool,
    /// Whether user verification is available.
    pub supports_user_verification: bool,
    /// Whether discoverable credentials are available.
    pub supports_discoverable_credentials: bool,
}

impl Availability {
    pub(crate) const fn supported() -> Self {
        Self {
            is_platform_supported: true,
            supports_user_verification: true,
            supports_discoverable_credentials: true,
        }
    }

    pub(crate) const fn unavailable() -> Self {
        Self {
            is_platform_supported: false,
            supports_user_verification: false,
            supports_discoverable_credentials: false,
        }
    }
}

/// Registration ceremony options.
#[derive(Debug, Clone)]
pub struct RegisterOptions {
    rp: RelyingParty,
    user: UserEntity,
    challenge: Challenge,
    timeout_ms: Option<u32>,
    attestation: AttestationPreference,
    pub_key_algorithms: Vec<PublicKeyAlgorithm>,
    exclude_credentials: Vec<CredentialDescriptor>,
    user_verification: UserVerificationRequirement,
    discoverable: bool,
}

impl RegisterOptions {
    /// Creates registration options with secure ergonomic defaults.
    #[must_use]
    pub fn new(rp: RelyingParty, user: UserEntity, challenge: Challenge) -> Self {
        Self {
            rp,
            user,
            challenge,
            timeout_ms: None,
            attestation: AttestationPreference::None,
            pub_key_algorithms: vec![PublicKeyAlgorithm::Es256, PublicKeyAlgorithm::Rs256],
            exclude_credentials: Vec::new(),
            user_verification: UserVerificationRequirement::Required,
            discoverable: true,
        }
    }

    /// Sets operation timeout in milliseconds.
    #[must_use]
    pub const fn timeout_ms(mut self, timeout_ms: u32) -> Self {
        self.timeout_ms = Some(timeout_ms);
        self
    }

    /// Sets attestation preference.
    #[must_use]
    pub const fn attestation(mut self, preference: AttestationPreference) -> Self {
        self.attestation = preference;
        self
    }

    /// Sets user verification policy.
    #[must_use]
    pub const fn user_verification(mut self, requirement: UserVerificationRequirement) -> Self {
        self.user_verification = requirement;
        self
    }

    /// Sets discoverable credential behavior.
    #[must_use]
    pub const fn discoverable(mut self, discoverable: bool) -> Self {
        self.discoverable = discoverable;
        self
    }

    /// Adds an excluded credential ID.
    #[must_use]
    pub fn exclude_credential(mut self, credential: CredentialDescriptor) -> Self {
        self.exclude_credentials.push(credential);
        self
    }

    /// Replaces algorithm preferences.
    ///
    /// # Errors
    /// Returns [`PasskeyError::InvalidInput`] when no algorithms are provided.
    pub fn pub_key_algorithms(
        mut self,
        algorithms: Vec<PublicKeyAlgorithm>,
    ) -> Result<Self, PasskeyError> {
        if algorithms.is_empty() {
            return Err(PasskeyError::InvalidInput(
                "at least one public key algorithm is required".into(),
            ));
        }
        self.pub_key_algorithms = algorithms;
        Ok(self)
    }

    pub(crate) const fn rp(&self) -> &RelyingParty {
        &self.rp
    }

    pub(crate) const fn user(&self) -> &UserEntity {
        &self.user
    }

    pub(crate) const fn challenge(&self) -> &Challenge {
        &self.challenge
    }

    pub(crate) const fn timeout_ms_value(&self) -> Option<u32> {
        self.timeout_ms
    }

    pub(crate) const fn attestation_value(&self) -> AttestationPreference {
        self.attestation
    }

    pub(crate) fn pub_key_algorithms_ref(&self) -> &[PublicKeyAlgorithm] {
        &self.pub_key_algorithms
    }

    pub(crate) fn exclude_credentials_ref(&self) -> &[CredentialDescriptor] {
        &self.exclude_credentials
    }

    pub(crate) const fn user_verification_value(&self) -> UserVerificationRequirement {
        self.user_verification
    }

    pub(crate) const fn discoverable_value(&self) -> bool {
        self.discoverable
    }
}

/// Authentication ceremony options.
#[derive(Debug, Clone)]
pub struct AuthenticateOptions {
    rp_id: RpId,
    challenge: Challenge,
    timeout_ms: Option<u32>,
    allow_credentials: Vec<CredentialDescriptor>,
    user_verification: UserVerificationRequirement,
}

impl AuthenticateOptions {
    /// Creates authentication options with secure ergonomic defaults.
    #[must_use]
    pub const fn new(rp_id: RpId, challenge: Challenge) -> Self {
        Self {
            rp_id,
            challenge,
            timeout_ms: None,
            allow_credentials: Vec::new(),
            user_verification: UserVerificationRequirement::Required,
        }
    }

    /// Sets operation timeout in milliseconds.
    #[must_use]
    pub const fn timeout_ms(mut self, timeout_ms: u32) -> Self {
        self.timeout_ms = Some(timeout_ms);
        self
    }

    /// Adds an allowed credential ID.
    #[must_use]
    pub fn allow_credential(mut self, credential: CredentialDescriptor) -> Self {
        self.allow_credentials.push(credential);
        self
    }

    /// Sets user verification requirement.
    #[must_use]
    pub const fn user_verification(mut self, requirement: UserVerificationRequirement) -> Self {
        self.user_verification = requirement;
        self
    }

    pub(crate) const fn rp_id(&self) -> &RpId {
        &self.rp_id
    }

    pub(crate) const fn challenge(&self) -> &Challenge {
        &self.challenge
    }

    pub(crate) const fn timeout_ms_value(&self) -> Option<u32> {
        self.timeout_ms
    }

    pub(crate) fn allow_credentials_ref(&self) -> &[CredentialDescriptor] {
        &self.allow_credentials
    }

    pub(crate) const fn user_verification_value(&self) -> UserVerificationRequirement {
        self.user_verification
    }
}

/// Ergonomic passkey client with reusable defaults.
#[derive(Debug, Clone)]
pub struct PasskeyClient {
    relying_party: RelyingParty,
    timeout_ms: Option<u32>,
    attestation: AttestationPreference,
    pub_key_algorithms: Vec<PublicKeyAlgorithm>,
    user_verification: UserVerificationRequirement,
    discoverable: bool,
}

impl PasskeyClient {
    /// Creates a client builder.
    #[must_use]
    pub fn builder() -> PasskeyClientBuilder {
        PasskeyClientBuilder::new()
    }

    /// Creates a client from a relying-party descriptor.
    #[must_use]
    pub fn new(relying_party: RelyingParty) -> Self {
        Self {
            relying_party,
            timeout_ms: None,
            attestation: AttestationPreference::None,
            pub_key_algorithms: vec![PublicKeyAlgorithm::Es256, PublicKeyAlgorithm::Rs256],
            user_verification: UserVerificationRequirement::Required,
            discoverable: true,
        }
    }

    /// Queries passkey availability.
    ///
    /// # Errors
    /// Returns [`PasskeyError`] if the backend fails.
    pub async fn availability(&self) -> Result<Availability, PasskeyError> {
        let backend = platform_backend();
        backend.is_available().await
    }

    /// Registers a discoverable credential with client defaults.
    ///
    /// # Errors
    /// Returns [`PasskeyError`] if registration fails.
    pub async fn register(
        &self,
        user: UserEntity,
        challenge: Challenge,
    ) -> Result<RegistrationResult, PasskeyError> {
        let options = RegisterOptions::new(self.relying_party.clone(), user, challenge)
            .user_verification(self.user_verification)
            .attestation(self.attestation)
            .discoverable(self.discoverable)
            .pub_key_algorithms(self.pub_key_algorithms.clone())?
            .with_optional_timeout(self.timeout_ms);

        register(options).await
    }

    /// Registers a credential with explicit options.
    ///
    /// # Errors
    /// Returns [`PasskeyError`] if registration fails.
    pub async fn register_with(
        &self,
        options: RegisterOptions,
    ) -> Result<RegistrationResult, PasskeyError> {
        register(options).await
    }

    /// Authenticates with client defaults.
    ///
    /// # Errors
    /// Returns [`PasskeyError`] if authentication fails.
    pub async fn authenticate(
        &self,
        challenge: Challenge,
    ) -> Result<AuthenticationResult, PasskeyError> {
        let options = AuthenticateOptions::new(self.relying_party.id.clone(), challenge)
            .user_verification(self.user_verification)
            .with_optional_timeout(self.timeout_ms);

        authenticate(options).await
    }

    /// Authenticates with explicit options.
    ///
    /// # Errors
    /// Returns [`PasskeyError`] if authentication fails.
    pub async fn authenticate_with(
        &self,
        options: AuthenticateOptions,
    ) -> Result<AuthenticationResult, PasskeyError> {
        authenticate(options).await
    }

    /// Borrows configured relying-party defaults.
    #[must_use]
    pub const fn relying_party(&self) -> &RelyingParty {
        &self.relying_party
    }
}

/// Builder for [`PasskeyClient`].
#[derive(Debug, Clone)]
pub struct PasskeyClientBuilder {
    rp_id: Option<String>,
    rp_name: Option<String>,
    timeout_ms: Option<u32>,
    attestation: AttestationPreference,
    pub_key_algorithms: Vec<PublicKeyAlgorithm>,
    user_verification: UserVerificationRequirement,
    discoverable: bool,
}

impl PasskeyClientBuilder {
    /// Creates a new passkey client builder.
    #[must_use]
    pub fn new() -> Self {
        Self {
            rp_id: None,
            rp_name: None,
            timeout_ms: None,
            attestation: AttestationPreference::None,
            pub_key_algorithms: vec![PublicKeyAlgorithm::Es256, PublicKeyAlgorithm::Rs256],
            user_verification: UserVerificationRequirement::Required,
            discoverable: true,
        }
    }

    /// Sets relying-party ID.
    #[must_use]
    pub fn rp_id(mut self, rp_id: impl Into<String>) -> Self {
        self.rp_id = Some(rp_id.into());
        self
    }

    /// Sets relying-party display name.
    #[must_use]
    pub fn rp_name(mut self, rp_name: impl Into<String>) -> Self {
        self.rp_name = Some(rp_name.into());
        self
    }

    /// Sets relying-party values at once.
    #[must_use]
    pub fn relying_party(mut self, relying_party: RelyingParty) -> Self {
        let RelyingParty {
            id: RpId(rp_id),
            name: rp_name,
        } = relying_party;
        self.rp_id = Some(rp_id);
        self.rp_name = Some(rp_name);
        self
    }

    /// Sets default timeout in milliseconds.
    #[must_use]
    pub const fn timeout_ms(mut self, timeout_ms: u32) -> Self {
        self.timeout_ms = Some(timeout_ms);
        self
    }

    /// Sets default attestation preference.
    #[must_use]
    pub const fn attestation(mut self, preference: AttestationPreference) -> Self {
        self.attestation = preference;
        self
    }

    /// Sets default user verification behavior.
    #[must_use]
    pub const fn user_verification(mut self, requirement: UserVerificationRequirement) -> Self {
        self.user_verification = requirement;
        self
    }

    /// Sets default discoverable credential behavior.
    #[must_use]
    pub const fn discoverable(mut self, discoverable: bool) -> Self {
        self.discoverable = discoverable;
        self
    }

    /// Sets default public key algorithm preferences.
    ///
    /// # Errors
    /// Returns [`PasskeyError::InvalidInput`] when no algorithms are provided.
    pub fn pub_key_algorithms(
        mut self,
        algorithms: Vec<PublicKeyAlgorithm>,
    ) -> Result<Self, PasskeyError> {
        if algorithms.is_empty() {
            return Err(PasskeyError::InvalidInput(
                "at least one public key algorithm is required".into(),
            ));
        }
        self.pub_key_algorithms = algorithms;
        Ok(self)
    }

    /// Builds a [`PasskeyClient`].
    ///
    /// # Errors
    /// Returns [`PasskeyError::InvalidInput`] if relying-party fields are missing or malformed.
    pub fn build(self) -> Result<PasskeyClient, PasskeyError> {
        let rp_id = self
            .rp_id
            .ok_or_else(|| PasskeyError::InvalidInput("missing rp id".into()))?;
        let rp_name = self
            .rp_name
            .ok_or_else(|| PasskeyError::InvalidInput("missing rp name".into()))?;
        let relying_party = RelyingParty::new(rp_id, rp_name)?;

        Ok(PasskeyClient {
            relying_party,
            timeout_ms: self.timeout_ms,
            attestation: self.attestation,
            pub_key_algorithms: self.pub_key_algorithms,
            user_verification: self.user_verification,
            discoverable: self.discoverable,
        })
    }
}

impl Default for PasskeyClientBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl RegisterOptions {
    const fn with_optional_timeout(mut self, timeout_ms: Option<u32>) -> Self {
        self.timeout_ms = timeout_ms;
        self
    }
}

impl AuthenticateOptions {
    const fn with_optional_timeout(mut self, timeout_ms: Option<u32>) -> Self {
        self.timeout_ms = timeout_ms;
        self
    }
}

/// Registration ceremony result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrationResult {
    credential_id: CredentialId,
    attestation_object: Vec<u8>,
    client_data_json: Vec<u8>,
    authenticator_data: Option<Vec<u8>>,
    public_key_cose: Option<Vec<u8>>,
}

impl RegistrationResult {
    pub(crate) const fn new(
        credential_id: CredentialId,
        attestation_object: Vec<u8>,
        client_data_json: Vec<u8>,
        authenticator_data: Option<Vec<u8>>,
        public_key_cose: Option<Vec<u8>>,
    ) -> Self {
        Self {
            credential_id,
            attestation_object,
            client_data_json,
            authenticator_data,
            public_key_cose,
        }
    }

    /// Borrows credential ID.
    #[must_use]
    pub const fn credential_id(&self) -> &CredentialId {
        &self.credential_id
    }

    /// Borrows attestation object bytes.
    #[must_use]
    pub fn attestation_object(&self) -> &[u8] {
        &self.attestation_object
    }

    /// Borrows client data JSON bytes.
    #[must_use]
    pub fn client_data_json(&self) -> &[u8] {
        &self.client_data_json
    }

    /// Borrows optional authenticator data.
    #[must_use]
    pub fn authenticator_data(&self) -> Option<&[u8]> {
        self.authenticator_data.as_deref()
    }

    /// Borrows optional credential public key COSE bytes.
    #[must_use]
    pub fn public_key_cose(&self) -> Option<&[u8]> {
        self.public_key_cose.as_deref()
    }

    /// Converts to a `WebAuthn` JSON-ready payload.
    #[must_use]
    pub fn to_webauthn_json(&self) -> RegistrationWebAuthnJson {
        RegistrationWebAuthnJson {
            id: encode_base64url(self.credential_id.as_bytes()),
            raw_id: encode_base64url(self.credential_id.as_bytes()),
            credential_type: "public-key",
            response: RegistrationWebAuthnResponseJson {
                attestation_object: encode_base64url(&self.attestation_object),
                client_data_json: encode_base64url(&self.client_data_json),
                authenticator_data: self
                    .authenticator_data
                    .as_ref()
                    .map(|bytes| encode_base64url(bytes)),
                public_key_cose: self
                    .public_key_cose
                    .as_ref()
                    .map(|bytes| encode_base64url(bytes)),
            },
        }
    }
}

/// Authentication ceremony result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthenticationResult {
    credential_id: CredentialId,
    authenticator_data: Vec<u8>,
    client_data_json: Vec<u8>,
    signature: Vec<u8>,
    user_handle: Option<Vec<u8>>,
}

impl AuthenticationResult {
    pub(crate) const fn new(
        credential_id: CredentialId,
        authenticator_data: Vec<u8>,
        client_data_json: Vec<u8>,
        signature: Vec<u8>,
        user_handle: Option<Vec<u8>>,
    ) -> Self {
        Self {
            credential_id,
            authenticator_data,
            client_data_json,
            signature,
            user_handle,
        }
    }

    /// Borrows credential ID.
    #[must_use]
    pub const fn credential_id(&self) -> &CredentialId {
        &self.credential_id
    }

    /// Borrows authenticator data.
    #[must_use]
    pub fn authenticator_data(&self) -> &[u8] {
        &self.authenticator_data
    }

    /// Borrows client data JSON.
    #[must_use]
    pub fn client_data_json(&self) -> &[u8] {
        &self.client_data_json
    }

    /// Borrows signature.
    #[must_use]
    pub fn signature(&self) -> &[u8] {
        &self.signature
    }

    /// Borrows optional user handle.
    #[must_use]
    pub fn user_handle(&self) -> Option<&[u8]> {
        self.user_handle.as_deref()
    }

    /// Converts to a `WebAuthn` JSON-ready payload.
    #[must_use]
    pub fn to_webauthn_json(&self) -> AuthenticationWebAuthnJson {
        AuthenticationWebAuthnJson {
            id: encode_base64url(self.credential_id.as_bytes()),
            raw_id: encode_base64url(self.credential_id.as_bytes()),
            credential_type: "public-key",
            response: AuthenticationWebAuthnResponseJson {
                authenticator_data: encode_base64url(&self.authenticator_data),
                client_data_json: encode_base64url(&self.client_data_json),
                signature: encode_base64url(&self.signature),
                user_handle: self
                    .user_handle
                    .as_ref()
                    .map(|bytes| encode_base64url(bytes)),
            },
        }
    }
}

/// JSON-ready registration payload.
#[derive(Debug, Clone, Serialize)]
pub struct RegistrationWebAuthnJson {
    /// Credential id encoded as base64url.
    pub id: String,
    /// Raw credential id encoded as base64url.
    #[serde(rename = "rawId")]
    pub raw_id: String,
    /// Credential type (`public-key`).
    #[serde(rename = "type")]
    pub credential_type: &'static str,
    /// Registration response payload.
    pub response: RegistrationWebAuthnResponseJson,
}

/// Registration response payload in `WebAuthn` wire shape.
#[derive(Debug, Clone, Serialize)]
pub struct RegistrationWebAuthnResponseJson {
    /// Attestation object encoded as base64url.
    #[serde(rename = "attestationObject")]
    pub attestation_object: String,
    /// Client data JSON encoded as base64url.
    #[serde(rename = "clientDataJSON")]
    pub client_data_json: String,
    /// Optional authenticator data encoded as base64url.
    #[serde(rename = "authenticatorData", skip_serializing_if = "Option::is_none")]
    pub authenticator_data: Option<String>,
    /// Optional COSE credential public key encoded as base64url.
    #[serde(rename = "publicKey", skip_serializing_if = "Option::is_none")]
    pub public_key_cose: Option<String>,
}

/// JSON-ready authentication payload.
#[derive(Debug, Clone, Serialize)]
pub struct AuthenticationWebAuthnJson {
    /// Credential id encoded as base64url.
    pub id: String,
    /// Raw credential id encoded as base64url.
    #[serde(rename = "rawId")]
    pub raw_id: String,
    /// Credential type (`public-key`).
    #[serde(rename = "type")]
    pub credential_type: &'static str,
    /// Authentication response payload.
    pub response: AuthenticationWebAuthnResponseJson,
}

/// Authentication response payload in `WebAuthn` wire shape.
#[derive(Debug, Clone, Serialize)]
pub struct AuthenticationWebAuthnResponseJson {
    /// Authenticator data encoded as base64url.
    #[serde(rename = "authenticatorData")]
    pub authenticator_data: String,
    /// Client data JSON encoded as base64url.
    #[serde(rename = "clientDataJSON")]
    pub client_data_json: String,
    /// Signature encoded as base64url.
    pub signature: String,
    /// Optional user handle encoded as base64url.
    #[serde(rename = "userHandle", skip_serializing_if = "Option::is_none")]
    pub user_handle: Option<String>,
}

/// Queries passkey availability.
///
/// # Errors
/// Returns [`PasskeyError`] if the backend fails.
pub async fn is_available() -> Result<Availability, PasskeyError> {
    let backend = platform_backend();
    backend.is_available().await
}

/// Runs a registration ceremony.
///
/// # Errors
/// Returns [`PasskeyError`] if registration fails.
pub async fn register(options: RegisterOptions) -> Result<RegistrationResult, PasskeyError> {
    let backend = platform_backend();
    backend.register(&options).await
}

/// Runs an authentication ceremony.
///
/// # Errors
/// Returns [`PasskeyError`] if authentication fails.
pub async fn authenticate(
    options: AuthenticateOptions,
) -> Result<AuthenticationResult, PasskeyError> {
    let backend = platform_backend();
    backend.authenticate(&options).await
}

#[derive(Debug, Serialize)]
pub(crate) struct RegisterRequestWire {
    rp_id: String,
    rp_name: String,
    user_name: String,
    user_display_name: String,
    user_id_b64u: String,
    challenge_b64u: String,
    timeout_ms: Option<u32>,
    attestation: &'static str,
    user_verification: &'static str,
    discoverable: bool,
    algorithms: Vec<i32>,
    exclude_credentials: Vec<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct AuthenticateRequestWire {
    rp_id: String,
    challenge_b64u: String,
    timeout_ms: Option<u32>,
    user_verification: &'static str,
    allow_credentials: Vec<String>,
}

pub(crate) fn register_request_json(options: &RegisterOptions) -> Result<String, PasskeyError> {
    let wire = RegisterRequestWire {
        rp_id: options.rp().id().as_str().to_owned(),
        rp_name: options.rp().name().to_owned(),
        user_name: options.user().name().to_owned(),
        user_display_name: options.user().display_name().to_owned(),
        user_id_b64u: encode_base64url(options.user().id().as_bytes()),
        challenge_b64u: encode_base64url(options.challenge().as_bytes()),
        timeout_ms: options.timeout_ms_value(),
        attestation: options.attestation_value().as_wire(),
        user_verification: options.user_verification_value().as_wire(),
        discoverable: options.discoverable_value(),
        algorithms: options
            .pub_key_algorithms_ref()
            .iter()
            .map(|algorithm| algorithm.cose_id())
            .collect(),
        exclude_credentials: options
            .exclude_credentials_ref()
            .iter()
            .map(|credential| encode_base64url(credential.id().as_bytes()))
            .collect(),
    };

    serde_json::to_string(&wire).map_err(|error| {
        PasskeyError::OperationFailed(format!("failed to serialize register request: {error}"))
    })
}

pub(crate) fn authenticate_request_json(
    options: &AuthenticateOptions,
) -> Result<String, PasskeyError> {
    let wire = AuthenticateRequestWire {
        rp_id: options.rp_id().as_str().to_owned(),
        challenge_b64u: encode_base64url(options.challenge().as_bytes()),
        timeout_ms: options.timeout_ms_value(),
        user_verification: options.user_verification_value().as_wire(),
        allow_credentials: options
            .allow_credentials_ref()
            .iter()
            .map(|credential| encode_base64url(credential.id().as_bytes()))
            .collect(),
    };

    serde_json::to_string(&wire).map_err(|error| {
        PasskeyError::OperationFailed(format!("failed to serialize authenticate request: {error}"))
    })
}

#[derive(Debug, Deserialize)]
struct RegistrationResponseWire {
    #[serde(rename = "credential_id_b64u")]
    credential_id: String,
    #[serde(rename = "attestation_object_b64u")]
    attestation_object: String,
    #[serde(rename = "client_data_json_b64u")]
    client_data_json: String,
    #[serde(rename = "authenticator_data_b64u")]
    authenticator_data: Option<String>,
    #[serde(rename = "public_key_cose_b64u")]
    public_key_cose: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AuthenticationResponseWire {
    #[serde(rename = "credential_id_b64u")]
    credential_id: String,
    #[serde(rename = "authenticator_data_b64u")]
    authenticator_data: String,
    #[serde(rename = "client_data_json_b64u")]
    client_data_json: String,
    #[serde(rename = "signature_b64u")]
    signature: String,
    #[serde(rename = "user_handle_b64u")]
    user_handle: Option<String>,
}

pub(crate) fn parse_registration_response_json(
    response_json: &str,
) -> Result<RegistrationResult, PasskeyError> {
    let wire: RegistrationResponseWire = serde_json::from_str(response_json).map_err(|error| {
        PasskeyError::OperationFailed(format!(
            "failed to parse registration response JSON: {error}"
        ))
    })?;

    let credential_id =
        CredentialId::new(decode_base64url(&wire.credential_id, "credential_id_b64u")?)?;
    let attestation_object =
        decode_base64url(&wire.attestation_object, "attestation_object_b64u")?;
    let client_data_json = decode_base64url(&wire.client_data_json, "client_data_json_b64u")?;
    let authenticator_data =
        decode_optional_base64url(wire.authenticator_data, "authenticator_data_b64u")?;
    let public_key_cose = decode_optional_base64url(wire.public_key_cose, "public_key_cose_b64u")?;

    Ok(RegistrationResult::new(
        credential_id,
        attestation_object,
        client_data_json,
        authenticator_data,
        public_key_cose,
    ))
}

pub(crate) fn parse_authentication_response_json(
    response_json: &str,
) -> Result<AuthenticationResult, PasskeyError> {
    let wire: AuthenticationResponseWire =
        serde_json::from_str(response_json).map_err(|error| {
            PasskeyError::OperationFailed(format!(
                "failed to parse authentication response JSON: {error}"
            ))
        })?;

    let credential_id =
        CredentialId::new(decode_base64url(&wire.credential_id, "credential_id_b64u")?)?;
    let authenticator_data =
        decode_base64url(&wire.authenticator_data, "authenticator_data_b64u")?;
    let client_data_json = decode_base64url(&wire.client_data_json, "client_data_json_b64u")?;
    let signature = decode_base64url(&wire.signature, "signature_b64u")?;
    let user_handle = decode_optional_base64url(wire.user_handle, "user_handle_b64u")?;

    Ok(AuthenticationResult::new(
        credential_id,
        authenticator_data,
        client_data_json,
        signature,
        user_handle,
    ))
}

fn validate_non_empty(label: &str, value: &str) -> Result<(), PasskeyError> {
    if value.trim().is_empty() {
        return Err(PasskeyError::InvalidInput(format!(
            "{label} must not be empty"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn challenge_rejects_empty() {
        assert!(Challenge::new(Vec::new()).is_err());
    }

    #[test]
    fn rp_id_rejects_bad_dots() {
        assert!(RpId::new(".example.com").is_err());
        assert!(RpId::new("example..com").is_err());
    }

    #[test]
    fn registration_json_helper_contains_public_key_type() {
        let credential_id =
            CredentialId::new(vec![1, 2, 3]).expect("credential id should be valid");
        let result =
            RegistrationResult::new(credential_id, vec![4, 5, 6], vec![7, 8, 9], None, None);

        let payload = result.to_webauthn_json();
        assert_eq!(payload.credential_type, "public-key");
        assert!(!payload.id.is_empty());
    }
}
