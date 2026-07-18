//! Engine-neutral protected-media metadata.

use crate::Error;

/// ISO/IEC 23001-7 sample-protection algorithm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CommonEncryptionScheme {
    /// AES-128 counter mode (`cenc`).
    Cenc,
    /// AES-128 pattern cipher-block chaining (`cbcs`).
    Cbcs,
}

/// Track-wide Common Encryption defaults from an ISO BMFF `tenc` box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackProtection {
    scheme: CommonEncryptionScheme,
    default_key_id: [u8; 16],
    per_sample_iv_size: u8,
    constant_iv: Option<Vec<u8>>,
    crypt_byte_block: u8,
    skip_byte_block: u8,
}

impl TrackProtection {
    /// Creates validated track protection defaults.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid per-sample or constant IV configuration.
    pub fn new(
        scheme: CommonEncryptionScheme,
        default_key_id: [u8; 16],
        per_sample_iv_size: u8,
        constant_iv: Option<Vec<u8>>,
        crypt_byte_block: u8,
        skip_byte_block: u8,
    ) -> Result<Self, Error> {
        if !matches!(per_sample_iv_size, 0 | 8 | 16) {
            return Err(Error::Container(format!(
                "CENC per-sample IV size must be 0, 8, or 16 bytes, got {per_sample_iv_size}"
            )));
        }
        match (per_sample_iv_size, constant_iv.as_ref()) {
            (0, Some(iv)) if matches!(iv.len(), 8 | 16) => {}
            (0, Some(iv)) => {
                return Err(Error::Container(format!(
                    "CENC constant IV must be 8 or 16 bytes, got {}",
                    iv.len()
                )));
            }
            (0, None) => {
                return Err(Error::Container(String::from(
                    "CENC track with no per-sample IV must declare a constant IV",
                )));
            }
            (_, None) => {}
            (_, Some(_)) => {
                return Err(Error::Container(String::from(
                    "CENC track cannot combine per-sample and constant IVs",
                )));
            }
        }
        Ok(Self {
            scheme,
            default_key_id,
            per_sample_iv_size,
            constant_iv,
            crypt_byte_block,
            skip_byte_block,
        })
    }

    /// Returns the sample-protection algorithm.
    #[must_use]
    pub const fn scheme(&self) -> CommonEncryptionScheme {
        self.scheme
    }

    /// Returns the default 16-byte content-key identifier.
    #[must_use]
    pub const fn default_key_id(&self) -> &[u8; 16] {
        &self.default_key_id
    }

    /// Returns the IV length carried by each sample, or zero for a constant IV.
    #[must_use]
    pub const fn per_sample_iv_size(&self) -> u8 {
        self.per_sample_iv_size
    }

    /// Returns the track-wide constant IV when samples do not carry their own.
    #[must_use]
    pub fn constant_iv(&self) -> Option<&[u8]> {
        self.constant_iv.as_deref()
    }

    /// Returns the number of encrypted 16-byte blocks in each `cbcs` pattern.
    #[must_use]
    pub const fn crypt_byte_block(&self) -> u8 {
        self.crypt_byte_block
    }

    /// Returns the number of clear 16-byte blocks in each `cbcs` pattern.
    #[must_use]
    pub const fn skip_byte_block(&self) -> u8 {
        self.skip_byte_block
    }
}

/// One clear/encrypted byte pair from a CENC subsample map.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EncryptionSubsample {
    clear_bytes: u16,
    encrypted_bytes: u32,
}

impl EncryptionSubsample {
    /// Creates one exact subsample range.
    #[must_use]
    pub const fn new(clear_bytes: u16, encrypted_bytes: u32) -> Self {
        Self {
            clear_bytes,
            encrypted_bytes,
        }
    }

    /// Returns the clear prefix length.
    #[must_use]
    pub const fn clear_bytes(self) -> u16 {
        self.clear_bytes
    }

    /// Returns the encrypted suffix length.
    #[must_use]
    pub const fn encrypted_bytes(self) -> u32 {
        self.encrypted_bytes
    }
}

/// Per-sample Common Encryption metadata from a `senc` box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SampleEncryption {
    initialization_vector: Vec<u8>,
    subsamples: Vec<EncryptionSubsample>,
}

impl SampleEncryption {
    /// Creates per-sample encryption metadata.
    #[must_use]
    pub const fn new(initialization_vector: Vec<u8>, subsamples: Vec<EncryptionSubsample>) -> Self {
        Self {
            initialization_vector,
            subsamples,
        }
    }

    /// Returns the sample IV. This is empty when the track declares a constant IV.
    #[must_use]
    pub fn initialization_vector(&self) -> &[u8] {
        &self.initialization_vector
    }

    /// Returns clear/encrypted byte ranges, or an empty slice when the whole sample is encrypted.
    #[must_use]
    pub fn subsamples(&self) -> &[EncryptionSubsample] {
        &self.subsamples
    }
}

/// DRM initialization data from an ISO BMFF `pssh` box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtectionInitData {
    system_id: [u8; 16],
    key_ids: Vec<[u8; 16]>,
    init_data: Vec<u8>,
    payload: Vec<u8>,
}

impl ProtectionInitData {
    /// Creates one platform-CDM initialization record.
    #[must_use]
    pub const fn new(
        system_id: [u8; 16],
        key_ids: Vec<[u8; 16]>,
        init_data: Vec<u8>,
        payload: Vec<u8>,
    ) -> Self {
        Self {
            system_id,
            key_ids,
            init_data,
            payload,
        }
    }

    /// Returns the DRM system UUID in network byte order.
    #[must_use]
    pub const fn system_id(&self) -> &[u8; 16] {
        &self.system_id
    }

    /// Returns content-key identifiers carried by a version-one `pssh` box.
    #[must_use]
    pub fn key_ids(&self) -> &[[u8; 16]] {
        &self.key_ids
    }

    /// Returns the complete serialized `pssh` box supplied to a platform CDM.
    #[must_use]
    pub fn init_data(&self) -> &[u8] {
        &self.init_data
    }

    /// Returns the DRM-system-specific `pssh` data field.
    #[must_use]
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }
}
