use std::collections::BTreeMap;
use std::fmt;
use std::io::{Cursor, Read, Seek};
use std::str::FromStr;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::Deserialize;
use serde::de::IgnoredAny;
use zip::ZipArchive;
use zip::result::ZipError;

/// The JWT segment in which a validation error occurred.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JwtSegment {
    /// The JWT header.
    Header,
    /// The JWT claims.
    Claims,
    /// The JWT signature.
    Signature,
}

impl fmt::Display for JwtSegment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Header => formatter.write_str("header"),
            Self::Claims => formatter.write_str("claims"),
            Self::Signature => formatter.write_str("signature"),
        }
    }
}

/// A Google Wallet save-to-wallet JWT with validated structure and claims.
#[derive(Clone, PartialEq, Eq)]
pub struct GoogleWalletJwt(String);

impl fmt::Debug for GoogleWalletJwt {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("GoogleWalletJwt([REDACTED])")
    }
}

impl GoogleWalletJwt {
    /// Validates and stores a Google Wallet JWT.
    ///
    /// The signature is not verified; Google Wallet performs that verification.
    ///
    /// # Errors
    ///
    /// Returns [`JwtError`] when the token's encoding, header, or claims are
    /// invalid.
    pub fn new(token: impl Into<String>) -> Result<Self, JwtError> {
        let token = token.into();
        validate_jwt(&token)?;
        Ok(Self(token))
    }

    /// Borrows the encoded JWT.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for GoogleWalletJwt {
    type Err = JwtError;

    fn from_str(token: &str) -> Result<Self, Self::Err> {
        Self::new(token)
    }
}

/// Errors while validating a Google Wallet JWT.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum JwtError {
    /// The token did not contain exactly three segments.
    #[error("JWT must contain exactly three segments, found {found}")]
    Segments {
        /// The number of segments found.
        found: usize,
    },
    /// A segment was not valid base64url without padding.
    #[error("JWT {segment} segment is not valid base64url: {source}")]
    Encoding {
        /// The segment that failed decoding.
        segment: JwtSegment,
        /// The base64 decoding error.
        #[source]
        source: base64::DecodeError,
    },
    /// A decoded segment was empty.
    #[error("JWT {segment} segment must not be empty")]
    EmptySegment {
        /// The empty segment.
        segment: JwtSegment,
    },
    /// A decoded JSON segment could not be deserialized.
    #[error("JWT {segment} segment contains invalid JSON: {source}")]
    Json {
        /// The segment containing invalid JSON.
        segment: JwtSegment,
        /// The JSON error.
        #[source]
        source: serde_json::Error,
    },
    /// The header algorithm was not RS256.
    #[error("JWT algorithm must be RS256, found {found}")]
    Algorithm {
        /// The algorithm found in the header.
        found: String,
    },
    /// The claims audience was not `google`.
    #[error("JWT audience must be google, found {found}")]
    Audience {
        /// The audience found in the claims.
        found: String,
    },
    /// The claims type was not `savetowallet`.
    #[error("JWT type must be savetowallet, found {found}")]
    Type {
        /// The type found in the claims.
        found: String,
    },
    /// The issuer was empty.
    #[error("JWT issuer must not be empty")]
    Issuer,
    /// The payload did not contain any Google Wallet objects.
    #[error("JWT payload must contain at least one Google Wallet object")]
    NoObjects,
}

#[derive(Deserialize)]
struct JwtHeader {
    alg: String,
}

#[derive(Deserialize)]
struct JwtClaims {
    aud: String,
    typ: String,
    iss: String,
    payload: WalletPayload,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct WalletPayload {
    #[serde(rename = "genericObjects")]
    generic: Vec<IgnoredAny>,
    #[serde(rename = "eventTicketObjects")]
    event_tickets: Vec<IgnoredAny>,
    #[serde(rename = "flightObjects")]
    flights: Vec<IgnoredAny>,
    #[serde(rename = "giftCardObjects")]
    gift_cards: Vec<IgnoredAny>,
    #[serde(rename = "loyaltyObjects")]
    loyalty: Vec<IgnoredAny>,
    #[serde(rename = "offerObjects")]
    offers: Vec<IgnoredAny>,
    #[serde(rename = "transitObjects")]
    transit: Vec<IgnoredAny>,
}

impl WalletPayload {
    const fn has_objects(&self) -> bool {
        !self.generic.is_empty()
            || !self.event_tickets.is_empty()
            || !self.flights.is_empty()
            || !self.gift_cards.is_empty()
            || !self.loyalty.is_empty()
            || !self.offers.is_empty()
            || !self.transit.is_empty()
    }
}

fn validate_jwt(token: &str) -> Result<(), JwtError> {
    let segments = token.split('.').collect::<Vec<_>>();
    if segments.len() != 3 {
        return Err(JwtError::Segments {
            found: segments.len(),
        });
    }

    let [header, claims, signature] = segments.as_slice() else {
        unreachable!("segment count was validated");
    };
    let header = decode_segment(header, JwtSegment::Header)?;
    let claims = decode_segment(claims, JwtSegment::Claims)?;
    decode_segment(signature, JwtSegment::Signature)?;

    let header: JwtHeader = serde_json::from_slice(&header).map_err(|source| JwtError::Json {
        segment: JwtSegment::Header,
        source,
    })?;
    if header.alg != "RS256" {
        return Err(JwtError::Algorithm { found: header.alg });
    }

    let claims: JwtClaims = serde_json::from_slice(&claims).map_err(|source| JwtError::Json {
        segment: JwtSegment::Claims,
        source,
    })?;
    if claims.aud != "google" {
        return Err(JwtError::Audience { found: claims.aud });
    }
    if claims.typ != "savetowallet" {
        return Err(JwtError::Type { found: claims.typ });
    }
    if claims.iss.is_empty() {
        return Err(JwtError::Issuer);
    }
    if !claims.payload.has_objects() {
        return Err(JwtError::NoObjects);
    }
    Ok(())
}

fn decode_segment(segment: &str, name: JwtSegment) -> Result<Vec<u8>, JwtError> {
    let decoded = URL_SAFE_NO_PAD
        .decode(segment)
        .map_err(|source| JwtError::Encoding {
            segment: name,
            source,
        })?;
    if decoded.is_empty() {
        return Err(JwtError::EmptySegment { segment: name });
    }
    Ok(decoded)
}

/// A signed Apple Wallet pass archive that passed structural validation.
#[derive(Clone, PartialEq, Eq)]
pub struct ApplePass(Vec<u8>);

impl fmt::Debug for ApplePass {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ApplePass")
            .field("bytes", &self.0.len())
            .finish()
    }
}

impl ApplePass {
    /// Validates and stores a `.pkpass` archive.
    ///
    /// The archive signature and manifest hashes are not verified; `PassKit`
    /// performs those checks.
    ///
    /// # Errors
    ///
    /// Returns [`PkpassError`] when required archive entries or pass fields are
    /// missing or malformed.
    pub fn new(bytes: Vec<u8>) -> Result<Self, PkpassError> {
        validate_pkpass(&bytes)?;
        Ok(Self(bytes))
    }

    /// Borrows the original archive bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

/// One or more validated Apple Wallet passes.
#[derive(Clone, PartialEq, Eq)]
pub struct ApplePasses {
    first: ApplePass,
    rest: Vec<ApplePass>,
}

impl fmt::Debug for ApplePasses {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ApplePasses")
            .field("len", &self.len())
            .finish()
    }
}

impl ApplePasses {
    /// Constructs a non-empty collection of passes.
    #[must_use]
    pub const fn new(first: ApplePass, rest: Vec<ApplePass>) -> Self {
        Self { first, rest }
    }

    /// Returns the number of passes.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.rest.len() + 1
    }

    /// Returns whether the collection contains no passes.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        false
    }

    /// Returns an iterator over all passes in order.
    pub fn iter(&self) -> impl Iterator<Item = &ApplePass> {
        std::iter::once(&self.first).chain(&self.rest)
    }
}

impl From<ApplePass> for ApplePasses {
    fn from(first: ApplePass) -> Self {
        Self::new(first, Vec::new())
    }
}

impl TryFrom<Vec<ApplePass>> for ApplePasses {
    type Error = PkpassError;

    fn try_from(mut passes: Vec<ApplePass>) -> Result<Self, Self::Error> {
        if passes.is_empty() {
            return Err(PkpassError::NoPasses);
        }
        let first = passes.remove(0);
        Ok(Self::new(first, passes))
    }
}

/// Errors while validating an Apple Wallet pass archive.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PkpassError {
    /// The ZIP archive could not be read.
    #[error("invalid pkpass ZIP archive: {0}")]
    Archive(#[source] ZipError),
    /// A required root archive entry was missing.
    #[error("pkpass archive is missing required entry `{0}`")]
    MissingEntry(&'static str),
    /// A JSON archive entry could not be deserialized.
    #[error("pkpass entry `{entry}` contains invalid JSON: {source}")]
    Json {
        /// The JSON entry that failed.
        entry: &'static str,
        /// The JSON error.
        #[source]
        source: serde_json::Error,
    },
    /// The pass format version was not 1.
    #[error("pkpass formatVersion must be 1, found {found}")]
    FormatVersion {
        /// The version found in the pass.
        found: u32,
    },
    /// A required string field was empty.
    #[error("pkpass field `{0}` must not be empty")]
    EmptyField(&'static str),
    /// The manifest did not contain a `pass.json` entry.
    #[error("pkpass manifest does not contain `pass.json`")]
    ManifestMissingPassJson,
    /// No passes were provided.
    #[error("at least one Apple Wallet pass is required")]
    NoPasses,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PassJson {
    format_version: u32,
    pass_type_identifier: String,
    serial_number: String,
    team_identifier: String,
    organization_name: String,
    description: String,
}

fn validate_pkpass(bytes: &[u8]) -> Result<(), PkpassError> {
    let mut archive = ZipArchive::new(Cursor::new(bytes)).map_err(PkpassError::Archive)?;
    let pass = read_json_entry::<PassJson, _>(&mut archive, "pass.json")?;
    if pass.format_version != 1 {
        return Err(PkpassError::FormatVersion {
            found: pass.format_version,
        });
    }
    for (field, value) in [
        ("passTypeIdentifier", pass.pass_type_identifier),
        ("serialNumber", pass.serial_number),
        ("teamIdentifier", pass.team_identifier),
        ("organizationName", pass.organization_name),
        ("description", pass.description),
    ] {
        if value.is_empty() {
            return Err(PkpassError::EmptyField(field));
        }
    }

    let manifest: BTreeMap<String, String> = read_json_entry(&mut archive, "manifest.json")?;
    if !manifest.contains_key("pass.json") {
        return Err(PkpassError::ManifestMissingPassJson);
    }
    open_entry(&mut archive, "signature")?;
    Ok(())
}

fn read_json_entry<T: for<'de> Deserialize<'de>, R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    entry: &'static str,
) -> Result<T, PkpassError> {
    let mut file = open_entry(archive, entry)?;
    serde_json::from_reader(&mut file).map_err(|source| PkpassError::Json { entry, source })
}

fn open_entry<'archive, R: Read + Seek>(
    archive: &'archive mut ZipArchive<R>,
    entry: &'static str,
) -> Result<zip::read::ZipFile<'archive, R>, PkpassError> {
    archive.by_name(entry).map_err(|source| match source {
        ZipError::FileNotFound => PkpassError::MissingEntry(entry),
        source => PkpassError::Archive(source),
    })
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Write};

    use base64::Engine as _;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use serde_json::{Value, json};
    use zip::write::SimpleFileOptions;
    use zip::{CompressionMethod, ZipWriter};

    use super::{ApplePass, ApplePasses, GoogleWalletJwt, JwtError, PkpassError};

    fn jwt(header: &Value, claims: &Value, signature: &[u8]) -> String {
        format!(
            "{}.{}.{}",
            URL_SAFE_NO_PAD.encode(header.to_string()),
            URL_SAFE_NO_PAD.encode(claims.to_string()),
            URL_SAFE_NO_PAD.encode(signature)
        )
    }

    fn valid_claims() -> Value {
        json!({
            "aud": "google",
            "typ": "savetowallet",
            "iss": "issuer@example.com",
            "payload": { "genericObjects": [{ "id": "issuer.object" }] }
        })
    }

    fn valid_header() -> Value {
        json!({ "alg": "RS256" })
    }

    #[test]
    fn accepts_valid_google_wallet_jwt() {
        let encoded = jwt(&valid_header(), &valid_claims(), b"signature");
        let parsed = encoded
            .parse::<GoogleWalletJwt>()
            .expect("JWT should be valid");
        assert_eq!(parsed.as_str(), encoded);
    }

    #[test]
    fn rejects_wrong_jwt_segment_counts() {
        assert!(matches!(
            GoogleWalletJwt::new("one.two"),
            Err(JwtError::Segments { found: 2 })
        ));
        assert!(matches!(
            GoogleWalletJwt::new("one.two.three.four"),
            Err(JwtError::Segments { found: 4 })
        ));
    }

    #[test]
    fn rejects_invalid_base64url() {
        assert!(matches!(
            GoogleWalletJwt::new("%%.e30.c2ln"),
            Err(JwtError::Encoding { .. })
        ));
    }

    #[test]
    fn rejects_empty_signature() {
        let encoded = format!(
            "{}.{}.",
            URL_SAFE_NO_PAD.encode(valid_header().to_string()),
            URL_SAFE_NO_PAD.encode(valid_claims().to_string())
        );
        assert!(matches!(
            GoogleWalletJwt::new(encoded),
            Err(JwtError::Encoding { .. } | JwtError::EmptySegment { .. })
        ));
    }

    #[test]
    fn rejects_non_rs256_algorithm() {
        assert!(matches!(
            GoogleWalletJwt::new(jwt(&json!({ "alg": "HS256" }), &valid_claims(), b"sig")),
            Err(JwtError::Algorithm { .. })
        ));
    }

    #[test]
    fn rejects_wrong_audience_and_type() {
        let mut claims = valid_claims();
        claims["aud"] = json!("other");
        assert!(matches!(
            GoogleWalletJwt::new(jwt(&valid_header(), &claims, b"sig")),
            Err(JwtError::Audience { .. })
        ));

        let mut claims = valid_claims();
        claims["typ"] = json!("other");
        assert!(matches!(
            GoogleWalletJwt::new(jwt(&valid_header(), &claims, b"sig")),
            Err(JwtError::Type { .. })
        ));
    }

    #[test]
    fn rejects_missing_issuer() {
        let mut claims = valid_claims();
        claims.as_object_mut().expect("object").remove("iss");
        assert!(matches!(
            GoogleWalletJwt::new(jwt(&valid_header(), &claims, b"sig")),
            Err(JwtError::Json { .. })
        ));
    }

    #[test]
    fn rejects_empty_wallet_object_lists_and_classes_only() {
        let mut claims = valid_claims();
        claims["payload"] = json!({});
        assert!(matches!(
            GoogleWalletJwt::new(jwt(&valid_header(), &claims, b"sig")),
            Err(JwtError::NoObjects)
        ));

        let mut claims = valid_claims();
        claims["payload"] = json!({ "classes": [{ "id": "issuer.class" }] });
        assert!(matches!(
            GoogleWalletJwt::new(jwt(&valid_header(), &claims, b"sig")),
            Err(JwtError::NoObjects)
        ));
    }

    fn pass_json(format_version: u32, serial_number: &str) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "formatVersion": format_version,
            "passTypeIdentifier": "pass.example",
            "serialNumber": serial_number,
            "teamIdentifier": "TEAM123",
            "organizationName": "Example",
            "description": "Example pass"
        }))
        .expect("serialize pass.json")
    }

    fn archive(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        for (name, contents) in entries {
            writer.start_file(name, options).expect("start zip entry");
            writer.write_all(contents).expect("write zip entry");
        }
        writer.finish().expect("finish ZIP").into_inner()
    }

    fn valid_archive() -> Vec<u8> {
        archive(&[
            ("pass.json", pass_json(1, "serial")),
            ("manifest.json", br#"{"pass.json":"hash"}"#.to_vec()),
            ("signature", b"signature".to_vec()),
        ])
    }

    #[test]
    fn accepts_valid_pkpass_archive() {
        assert!(ApplePass::new(valid_archive()).is_ok());
    }

    #[test]
    fn rejects_non_zip_bytes() {
        assert!(matches!(
            ApplePass::new(b"not a zip".to_vec()),
            Err(PkpassError::Archive(_))
        ));
    }

    #[test]
    fn rejects_each_missing_required_archive_entry() {
        for entries in [
            vec![
                ("manifest.json", br#"{"pass.json":"hash"}"#.to_vec()),
                ("signature", b"sig".to_vec()),
            ],
            vec![
                ("pass.json", pass_json(1, "serial")),
                ("signature", b"sig".to_vec()),
            ],
            vec![
                ("pass.json", pass_json(1, "serial")),
                ("manifest.json", br#"{"pass.json":"hash"}"#.to_vec()),
            ],
        ] {
            assert!(matches!(
                ApplePass::new(archive(&entries)),
                Err(PkpassError::MissingEntry(_))
            ));
        }
    }

    #[test]
    fn rejects_wrong_format_version_and_empty_required_field() {
        let bytes = archive(&[
            ("pass.json", pass_json(2, "serial")),
            ("manifest.json", br#"{"pass.json":"hash"}"#.to_vec()),
            ("signature", b"sig".to_vec()),
        ]);
        assert!(matches!(
            ApplePass::new(bytes),
            Err(PkpassError::FormatVersion { found: 2 })
        ));

        let bytes = archive(&[
            ("pass.json", pass_json(1, "")),
            ("manifest.json", br#"{"pass.json":"hash"}"#.to_vec()),
            ("signature", b"sig".to_vec()),
        ]);
        assert!(matches!(
            ApplePass::new(bytes),
            Err(PkpassError::EmptyField("serialNumber"))
        ));
    }

    #[test]
    fn rejects_manifest_without_pass_json() {
        let bytes = archive(&[
            ("pass.json", pass_json(1, "serial")),
            ("manifest.json", br#"{"other.json":"hash"}"#.to_vec()),
            ("signature", b"sig".to_vec()),
        ]);
        assert!(matches!(
            ApplePass::new(bytes),
            Err(PkpassError::ManifestMissingPassJson)
        ));
    }

    #[test]
    fn apple_passes_are_nonempty_and_report_their_length() {
        assert!(matches!(
            ApplePasses::try_from(Vec::new()),
            Err(PkpassError::NoPasses)
        ));

        let pass = ApplePass::new(valid_archive()).expect("valid pass");
        let passes = ApplePasses::new(pass.clone(), vec![pass.clone(), pass]);
        assert_eq!(passes.len(), 3);
        assert_eq!(passes.iter().count(), 3);
    }
}
