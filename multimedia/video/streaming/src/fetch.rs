use std::{num::NonZeroUsize, time::Duration};

use bytes::Bytes;
use futures::StreamExt as _;
use url::Url;
use waterkit_video_core::Error;
use zenwave::{
    Method, StatusCode,
    header::{HeaderMap, HeaderName, HeaderValue},
};

use crate::transport::{TransportRequest, same_origin, send, strip_credentials};

/// One half-open HTTP byte range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MediaByteRange {
    start: u64,
    end_exclusive: u64,
}

impl MediaByteRange {
    /// Creates a non-empty half-open byte range.
    ///
    /// # Errors
    ///
    /// Returns an error when `start >= end_exclusive`.
    pub fn new(start: u64, end_exclusive: u64) -> Result<Self, Error> {
        if start >= end_exclusive {
            return Err(Error::Streaming(format!(
                "media byte range start {start} must be below end {end_exclusive}"
            )));
        }
        Ok(Self {
            start,
            end_exclusive,
        })
    }

    /// Returns the number of requested bytes.
    #[must_use]
    pub const fn len(self) -> u64 {
        self.end_exclusive - self.start
    }

    /// Returns whether the validated range is empty.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        false
    }

    /// Returns the first byte included in the request.
    #[must_use]
    pub const fn start(self) -> u64 {
        self.start
    }

    /// Returns the first byte excluded from the request.
    #[must_use]
    pub const fn end_exclusive(self) -> u64 {
        self.end_exclusive
    }
}

/// One bounded Zenwave media request.
#[derive(Debug, Clone)]
pub struct MediaRequest {
    url: Url,
    headers: HeaderMap,
    byte_range: Option<MediaByteRange>,
    maximum_response_bytes: NonZeroUsize,
    maximum_redirects: u8,
    forward_cross_origin_credentials: bool,
}

impl MediaRequest {
    /// Creates a bounded request for a complete resource.
    #[must_use]
    pub fn new(url: Url, maximum_response_bytes: NonZeroUsize) -> Self {
        Self {
            url,
            headers: HeaderMap::new(),
            byte_range: None,
            maximum_response_bytes,
            maximum_redirects: 10,
            forward_cross_origin_credentials: false,
        }
    }

    /// Creates a request whose response must exactly match a byte range.
    ///
    /// # Errors
    ///
    /// Returns an error when the requested length cannot fit in memory on the
    /// current architecture.
    pub fn ranged(url: Url, byte_range: MediaByteRange) -> Result<Self, Error> {
        let maximum_response_bytes = usize::try_from(byte_range.len())
            .ok()
            .and_then(NonZeroUsize::new)
            .ok_or_else(|| {
                Error::Streaming(String::from(
                    "media byte range length does not fit the current architecture",
                ))
            })?;
        Ok(Self {
            url,
            headers: HeaderMap::new(),
            byte_range: Some(byte_range),
            maximum_response_bytes,
            maximum_redirects: 10,
            forward_cross_origin_credentials: false,
        })
    }

    /// Returns the requested URL.
    #[must_use]
    pub const fn url(&self) -> &Url {
        &self.url
    }

    /// Adds an already validated HTTP header.
    #[must_use]
    pub fn with_header(mut self, name: HeaderName, value: HeaderValue) -> Self {
        self.headers.insert(name, value);
        self
    }

    /// Changes the maximum number of redirects followed by the request.
    #[must_use]
    pub const fn with_maximum_redirects(mut self, maximum_redirects: u8) -> Self {
        self.maximum_redirects = maximum_redirects;
        self
    }

    /// Changes the maximum accepted response size for a complete resource.
    ///
    /// Exact range requests retain their range-derived size invariant.
    #[must_use]
    pub const fn with_maximum_response_bytes(
        mut self,
        maximum_response_bytes: NonZeroUsize,
    ) -> Self {
        if self.byte_range.is_none() {
            self.maximum_response_bytes = maximum_response_bytes;
        }
        self
    }

    /// Explicitly permits credential headers to cross origin boundaries.
    ///
    /// By default, `Authorization`, `Cookie`, and `Proxy-Authorization` are
    /// removed when a related request or redirect changes scheme, host, or port.
    #[must_use]
    pub const fn with_cross_origin_credentials(mut self, enabled: bool) -> Self {
        self.forward_cross_origin_credentials = enabled;
        self
    }

    /// Creates a bounded request for a related resource while preserving safe context.
    ///
    /// Credential headers are retained only for the same origin unless explicitly
    /// enabled through [`Self::with_cross_origin_credentials`].
    #[must_use]
    pub fn related(&self, url: Url) -> Self {
        let mut request = Self {
            url,
            headers: self.headers.clone(),
            byte_range: None,
            maximum_response_bytes: self.maximum_response_bytes,
            maximum_redirects: self.maximum_redirects,
            forward_cross_origin_credentials: self.forward_cross_origin_credentials,
        };
        if !request.forward_cross_origin_credentials && !same_origin(&self.url, &request.url) {
            strip_credentials(&mut request.headers);
        }
        request
    }

    /// Returns the requested byte range.
    #[must_use]
    pub const fn byte_range(&self) -> Option<MediaByteRange> {
        self.byte_range
    }

    pub(crate) fn related_resource(
        &self,
        url: Url,
        byte_range: Option<MediaByteRange>,
        maximum_response_bytes: NonZeroUsize,
    ) -> Result<Self, Error> {
        let mut request = self.related(url);
        request.byte_range = byte_range;
        request.maximum_response_bytes = match byte_range {
            Some(range) => usize::try_from(range.len())
                .ok()
                .and_then(NonZeroUsize::new)
                .ok_or_else(|| {
                    Error::Streaming(String::from(
                        "media byte range length does not fit the current architecture",
                    ))
                })?,
            None => maximum_response_bytes,
        };
        Ok(request)
    }
}

/// Bytes and transfer metadata returned by [`fetch_media`].
#[derive(Debug, Clone)]
pub struct MediaResponse {
    effective_url: Url,
    bytes: Vec<u8>,
    elapsed: Duration,
    total_resource_bytes: Option<u64>,
    validator: MediaValidator,
}

/// Incremental bounded response body opened through Zenwave.
pub struct MediaStream {
    effective_url: Url,
    body: zenwave::Body,
    started: std::time::Instant,
    maximum_response_bytes: NonZeroUsize,
    expected_range_bytes: Option<usize>,
    received_bytes: usize,
    total_resource_bytes: Option<u64>,
    validator: MediaValidator,
    completed: bool,
}

impl std::fmt::Debug for MediaStream {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MediaStream")
            .field("effective_url", &self.effective_url)
            .field("maximum_response_bytes", &self.maximum_response_bytes)
            .field("expected_range_bytes", &self.expected_range_bytes)
            .field("received_bytes", &self.received_bytes)
            .field("total_resource_bytes", &self.total_resource_bytes)
            .field("validator", &self.validator)
            .field("completed", &self.completed)
            .finish_non_exhaustive()
    }
}

/// Metadata for one fully consumed incremental media response.
#[derive(Debug, Clone)]
pub struct MediaStreamReceipt {
    effective_url: Url,
    elapsed: Duration,
    received_bytes: usize,
    total_resource_bytes: Option<u64>,
    validator: MediaValidator,
}

/// Origin validators identifying one revision of a media resource.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MediaValidator {
    etag: Option<String>,
    last_modified: Option<String>,
}

impl MediaValidator {
    /// Creates an unvalidated identity for origins that provide no validators.
    #[must_use]
    pub const fn none() -> Self {
        Self {
            etag: None,
            last_modified: None,
        }
    }

    /// Returns the entity tag exactly as supplied by the origin.
    #[must_use]
    pub fn etag(&self) -> Option<&str> {
        self.etag.as_deref()
    }

    /// Returns the `Last-Modified` value exactly as supplied by the origin.
    #[must_use]
    pub fn last_modified(&self) -> Option<&str> {
        self.last_modified.as_deref()
    }

    /// Returns whether the origin supplied neither validator.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.etag.is_none() && self.last_modified.is_none()
    }

    fn from_headers(headers: &HeaderMap) -> Result<Self, Error> {
        Ok(Self {
            etag: optional_header_text(headers, "etag")?,
            last_modified: optional_header_text(headers, "last-modified")?,
        })
    }
}

/// Result of conditionally revalidating a cached media resource.
#[derive(Debug, Clone)]
pub enum MediaRevalidation {
    /// The origin returned HTTP 304 and cached bytes remain authoritative.
    NotModified {
        /// Effective URL after any redirects performed during revalidation.
        effective_url: Url,
        /// Current validators, updated from the 304 response when supplied.
        validator: MediaValidator,
    },
    /// The origin returned a new complete or ranged representation.
    Modified(MediaResponse),
}

impl MediaRevalidation {
    /// Returns the effective URL after redirects.
    #[must_use]
    pub const fn effective_url(&self) -> &Url {
        match self {
            Self::NotModified { effective_url, .. } => effective_url,
            Self::Modified(response) => response.effective_url(),
        }
    }

    /// Returns the validators that identify the current origin revision.
    #[must_use]
    pub const fn validator(&self) -> &MediaValidator {
        match self {
            Self::NotModified { validator, .. } => validator,
            Self::Modified(response) => response.validator(),
        }
    }
}

impl MediaResponse {
    /// Returns the effective URL after redirects.
    #[must_use]
    pub const fn effective_url(&self) -> &Url {
        &self.effective_url
    }

    /// Returns downloaded response bytes.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Consumes the response and returns downloaded bytes.
    #[must_use]
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    /// Returns body transfer elapsed time for bandwidth estimation.
    #[must_use]
    pub const fn elapsed(&self) -> Duration {
        self.elapsed
    }

    /// Returns total resource size from `Content-Range` or `Content-Length`.
    #[must_use]
    pub const fn total_resource_bytes(&self) -> Option<u64> {
        self.total_resource_bytes
    }

    /// Returns origin validators for persistent cache identity and revalidation.
    #[must_use]
    pub const fn validator(&self) -> &MediaValidator {
        &self.validator
    }
}

impl MediaStream {
    fn from_response(
        request: &MediaRequest,
        effective_url: Url,
        response: zenwave::Response,
    ) -> Result<Self, Error> {
        let total_resource_bytes = validate_response_headers(request, &effective_url, &response)?;
        let validator = MediaValidator::from_headers(response.headers())?;
        let expected_range_bytes = request
            .byte_range
            .map(|range| {
                usize::try_from(range.len()).map_err(|_| {
                    Error::Streaming(String::from(
                        "validated media byte range no longer fits the current architecture",
                    ))
                })
            })
            .transpose()?;
        Ok(Self {
            effective_url,
            body: response.into_body(),
            started: std::time::Instant::now(),
            maximum_response_bytes: request.maximum_response_bytes,
            expected_range_bytes,
            received_bytes: 0,
            total_resource_bytes,
            validator,
            completed: false,
        })
    }

    /// Returns the final URL after redirects.
    #[must_use]
    pub const fn effective_url(&self) -> &Url {
        &self.effective_url
    }

    /// Reads the next HTTP body chunk without waiting for the full response.
    ///
    /// # Errors
    ///
    /// Returns a streaming error for body failures or when cumulative bytes
    /// exceed the request's explicit response bound.
    pub async fn next_chunk(&mut self) -> Result<Option<Bytes>, Error> {
        if self.completed {
            return Ok(None);
        }
        let Some(chunk) = self.body.next().await else {
            if let Some(expected_range_bytes) = self.expected_range_bytes
                && expected_range_bytes != self.received_bytes
            {
                return Err(Error::Streaming(format!(
                    "GET {} returned {} bytes for a {}-byte range",
                    self.effective_url, self.received_bytes, expected_range_bytes
                )));
            }
            self.completed = true;
            return Ok(None);
        };
        let chunk = chunk.map_err(|error| Error::Streaming(error.to_string()))?;
        let received_bytes = self
            .received_bytes
            .checked_add(chunk.len())
            .ok_or_else(|| {
                Error::Streaming(String::from("media response length overflowed usize"))
            })?;
        if received_bytes > self.maximum_response_bytes.get() {
            return Err(Error::Streaming(format!(
                "GET {} exceeded the {}-byte response limit",
                self.effective_url, self.maximum_response_bytes
            )));
        }
        self.received_bytes = received_bytes;
        Ok(Some(chunk))
    }

    /// Consumes a body that has reached end-of-stream and returns transfer metadata.
    ///
    /// # Errors
    ///
    /// Returns a streaming error if the caller has not consumed the body to its
    /// explicit completion signal.
    pub fn finish(self) -> Result<MediaStreamReceipt, Error> {
        if !self.completed {
            return Err(Error::Streaming(format!(
                "GET {} media stream was finished before end-of-response",
                self.effective_url
            )));
        }
        Ok(MediaStreamReceipt {
            effective_url: self.effective_url,
            elapsed: self.started.elapsed(),
            received_bytes: self.received_bytes,
            total_resource_bytes: self.total_resource_bytes,
            validator: self.validator,
        })
    }
}

impl MediaStreamReceipt {
    /// Returns the final URL after redirects.
    #[must_use]
    pub const fn effective_url(&self) -> &Url {
        &self.effective_url
    }

    /// Returns body-transfer elapsed time.
    #[must_use]
    pub const fn elapsed(&self) -> Duration {
        self.elapsed
    }

    /// Returns the number of response body bytes consumed.
    #[must_use]
    pub const fn received_bytes(&self) -> usize {
        self.received_bytes
    }

    /// Returns total origin resource size when declared.
    #[must_use]
    pub const fn total_resource_bytes(&self) -> Option<u64> {
        self.total_resource_bytes
    }

    /// Returns origin validators for the consumed revision.
    #[must_use]
    pub const fn validator(&self) -> &MediaValidator {
        &self.validator
    }
}

/// Fetches one complete resource or exact byte range exclusively through Zenwave.
///
/// Redirects are followed explicitly so relative manifest URLs can be resolved
/// against the final effective URL. Range requests require HTTP 206 and a
/// matching `Content-Range`; an origin that silently ignores `Range` fails.
/// Every body is bounded before and during streaming.
///
/// # Errors
///
/// Returns a streaming error for redirects, status/header mismatches, body
/// errors, or a response that exceeds its declared memory limit.
pub async fn fetch_media(request: MediaRequest) -> Result<MediaResponse, Error> {
    let (effective_url, response) = send_media_request(&request).await?;
    read_media_response(&request, effective_url, response).await
}

/// Opens one complete resource or exact byte range as a bounded Zenwave stream.
///
/// Redirect, credential, status, range, and declared-size validation completes
/// before the returned stream yields its first body chunk.
///
/// # Errors
///
/// Returns a streaming error for redirects, status/header mismatches, invalid
/// ranges, or declared response sizes above the request bound.
pub async fn open_media_stream(request: MediaRequest) -> Result<MediaStream, Error> {
    let (effective_url, response) = send_media_request(&request).await?;
    MediaStream::from_response(&request, effective_url, response)
}

/// Conditionally revalidates a cached media revision through Zenwave.
///
/// `ETag` takes precedence through `If-None-Match`; `Last-Modified` is also sent
/// when available. An unvalidated cache identity performs an ordinary bounded
/// fetch and therefore always returns [`MediaRevalidation::Modified`].
///
/// # Errors
///
/// Returns a streaming error for invalid validator values, redirects,
/// status/header mismatches, body errors, or response-size violations.
pub async fn revalidate_media(
    mut request: MediaRequest,
    cached: &MediaValidator,
) -> Result<MediaRevalidation, Error> {
    if cached.is_empty() {
        return fetch_media(request).await.map(MediaRevalidation::Modified);
    }
    if let Some(etag) = cached.etag() {
        request = request.with_header(
            HeaderName::from_static("if-none-match"),
            HeaderValue::from_str(etag).map_err(|error| {
                Error::Streaming(format!("invalid cached ETag for revalidation: {error}"))
            })?,
        );
    }
    if let Some(last_modified) = cached.last_modified() {
        request = request.with_header(
            HeaderName::from_static("if-modified-since"),
            HeaderValue::from_str(last_modified).map_err(|error| {
                Error::Streaming(format!(
                    "invalid cached Last-Modified for revalidation: {error}"
                ))
            })?,
        );
    }
    let (effective_url, response) = send_media_request(&request).await?;
    if response.status() == StatusCode::NOT_MODIFIED {
        let returned = MediaValidator::from_headers(response.headers())?;
        let validator = MediaValidator {
            etag: returned.etag.or_else(|| cached.etag.clone()),
            last_modified: returned
                .last_modified
                .or_else(|| cached.last_modified.clone()),
        };
        return Ok(MediaRevalidation::NotModified {
            effective_url,
            validator,
        });
    }
    read_media_response(&request, effective_url, response)
        .await
        .map(MediaRevalidation::Modified)
}

async fn send_media_request(request: &MediaRequest) -> Result<(Url, zenwave::Response), Error> {
    let mut headers = request.headers.clone();
    if let Some(range) = request.byte_range {
        headers.insert(
            "range",
            HeaderValue::from_str(&format!(
                "bytes={}-{}",
                range.start(),
                range.end_exclusive() - 1
            ))
            .map_err(|error| Error::Streaming(format!("invalid media Range header: {error}")))?,
        );
    }
    send(TransportRequest {
        method: Method::GET,
        url: &request.url,
        headers: &headers,
        body: None,
        maximum_redirects: request.maximum_redirects,
        forward_cross_origin_credentials: request.forward_cross_origin_credentials,
    })
    .await
}

async fn read_media_response(
    request: &MediaRequest,
    effective_url: Url,
    response: zenwave::Response,
) -> Result<MediaResponse, Error> {
    let mut stream = MediaStream::from_response(request, effective_url, response)?;
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next_chunk().await? {
        bytes.extend_from_slice(&chunk);
    }
    let receipt = stream.finish()?;

    Ok(MediaResponse {
        effective_url: receipt.effective_url,
        bytes,
        elapsed: receipt.elapsed,
        total_resource_bytes: receipt.total_resource_bytes,
        validator: receipt.validator,
    })
}

fn optional_header_text(headers: &HeaderMap, name: &str) -> Result<Option<String>, Error> {
    headers
        .get(name)
        .map(|value| {
            value
                .to_str()
                .map(str::to_owned)
                .map_err(|error| Error::Streaming(format!("invalid {name} header: {error}")))
        })
        .transpose()
}

fn validate_response_headers(
    request: &MediaRequest,
    effective_url: &Url,
    response: &zenwave::Response,
) -> Result<Option<u64>, Error> {
    if let Some(range) = request.byte_range {
        if response.status() != StatusCode::PARTIAL_CONTENT {
            return Err(Error::Streaming(format!(
                "GET {effective_url} returned HTTP {} for a byte-range request; expected 206",
                response.status()
            )));
        }
        let content_range = response
            .headers()
            .get("content-range")
            .ok_or_else(|| {
                Error::Streaming(format!(
                    "GET {effective_url} returned 206 without Content-Range"
                ))
            })?
            .to_str()
            .map_err(|error| Error::Streaming(format!("invalid Content-Range: {error}")))?;
        let parsed = parse_content_range(content_range)?;
        if parsed.start != range.start() || parsed.end_exclusive != range.end_exclusive() {
            return Err(Error::Streaming(format!(
                "GET {effective_url} returned Content-Range {content_range:?}, expected bytes {}-{}/...",
                range.start(),
                range.end_exclusive() - 1
            )));
        }
        return Ok(parsed.total);
    }

    if !response.status().is_success() {
        return Err(Error::Streaming(format!(
            "GET {effective_url} returned HTTP {}",
            response.status()
        )));
    }
    let content_length = response
        .headers()
        .get("content-length")
        .map(|value| {
            value
                .to_str()
                .map_err(|error| Error::Streaming(format!("invalid Content-Length: {error}")))?
                .parse::<u64>()
                .map_err(|error| Error::Streaming(format!("invalid Content-Length: {error}")))
        })
        .transpose()?;
    let maximum_response_bytes =
        u64::try_from(request.maximum_response_bytes.get()).map_err(|_| {
            Error::Streaming(String::from(
                "media response limit exceeds HTTP u64 length range",
            ))
        })?;
    if content_length.is_some_and(|length| length > maximum_response_bytes) {
        return Err(Error::Streaming(format!(
            "GET {effective_url} Content-Length exceeds the {}-byte response limit",
            request.maximum_response_bytes
        )));
    }
    Ok(content_length)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ParsedContentRange {
    start: u64,
    end_exclusive: u64,
    total: Option<u64>,
}

fn parse_content_range(value: &str) -> Result<ParsedContentRange, Error> {
    let value = value
        .strip_prefix("bytes ")
        .ok_or_else(|| Error::Streaming(format!("unsupported Content-Range unit in {value:?}")))?;
    let (range, total) = value
        .split_once('/')
        .ok_or_else(|| Error::Streaming(format!("malformed Content-Range {value:?}")))?;
    let (start, inclusive_end) = range
        .split_once('-')
        .ok_or_else(|| Error::Streaming(format!("malformed Content-Range {value:?}")))?;
    let start = start
        .parse::<u64>()
        .map_err(|error| Error::Streaming(format!("invalid Content-Range start: {error}")))?;
    let inclusive_end = inclusive_end
        .parse::<u64>()
        .map_err(|error| Error::Streaming(format!("invalid Content-Range end: {error}")))?;
    let end_exclusive = inclusive_end.checked_add(1).ok_or_else(|| {
        Error::Streaming(String::from("Content-Range inclusive end overflowed u64"))
    })?;
    if start >= end_exclusive {
        return Err(Error::Streaming(format!(
            "Content-Range start {start} is above end {inclusive_end}"
        )));
    }
    let total =
        match total {
            "*" => None,
            total => Some(total.parse::<u64>().map_err(|error| {
                Error::Streaming(format!("invalid Content-Range total: {error}"))
            })?),
        };
    if let Some(total) = total
        && end_exclusive > total
    {
        return Err(Error::Streaming(format!(
            "Content-Range end {inclusive_end} exceeds total {total}"
        )));
    }
    Ok(ParsedContentRange {
        start,
        end_exclusive,
        total,
    })
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read as _, Write as _},
        net::TcpListener,
        num::NonZeroUsize,
        sync::mpsc,
        thread,
        time::Duration,
    };

    use url::Url;
    use zenwave::header::{HeaderName, HeaderValue};

    use super::{
        MediaByteRange, MediaRequest, MediaRevalidation, MediaValidator, ParsedContentRange,
        fetch_media, open_media_stream, parse_content_range, revalidate_media,
    };

    #[test]
    fn byte_ranges_are_non_empty() {
        assert!(MediaByteRange::new(10, 10).is_err());
        assert_eq!(
            MediaByteRange::new(10, 20)
                .expect("range must be valid")
                .len(),
            10
        );
    }

    #[test]
    fn content_range_is_parsed_as_half_open() {
        assert_eq!(
            parse_content_range("bytes 100-199/1000").expect("header must be valid"),
            ParsedContentRange {
                start: 100,
                end_exclusive: 200,
                total: Some(1_000),
            }
        );
        assert!(parse_content_range("items 100-199/1000").is_err());
        assert!(parse_content_range("bytes 100-1000/1000").is_err());
    }

    #[test]
    fn related_requests_strip_cross_origin_credentials_unless_explicitly_enabled() {
        let origin =
            Url::parse("http://127.0.0.1:41001/manifest.mpd").expect("test origin URL must parse");
        let other_origin =
            Url::parse("http://127.0.0.1:41002/segment.m4s").expect("test related URL must parse");
        let request = authenticated_request(origin);

        let stripped = request.related(other_origin.clone());
        assert!(stripped.headers.get("authorization").is_none());
        assert!(stripped.headers.get("cookie").is_none());
        let forwarded = request
            .with_cross_origin_credentials(true)
            .related(other_origin);
        assert!(forwarded.headers.get("authorization").is_some());
        assert!(forwarded.headers.get("cookie").is_some());
    }

    #[test]
    fn redirects_strip_credentials_before_contacting_another_origin() {
        let destination =
            TcpListener::bind(("127.0.0.1", 0)).expect("test destination server must bind");
        let destination_address = destination
            .local_addr()
            .expect("test destination address must exist");
        let (request_sender, request_receiver) = mpsc::sync_channel(1);
        let destination_worker = thread::spawn(move || {
            let (mut socket, _) = destination
                .accept()
                .expect("redirected request must arrive");
            let request = read_http_request(&mut socket);
            request_sender
                .send(request)
                .expect("captured request must send");
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
                .expect("destination response must write");
        });

        let redirect = TcpListener::bind(("127.0.0.1", 0)).expect("test redirect server must bind");
        let redirect_address = redirect
            .local_addr()
            .expect("test redirect address must exist");
        let redirect_worker = thread::spawn(move || {
            let (mut socket, _) = redirect.accept().expect("initial request must arrive");
            let _request = read_http_request(&mut socket);
            let response = format!(
                "HTTP/1.1 302 Found\r\nLocation: http://{destination_address}/media\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
            socket
                .write_all(response.as_bytes())
                .expect("redirect response must write");
        });

        let response = futures::executor::block_on(fetch_media(authenticated_request(
            Url::parse(&format!("http://{redirect_address}/manifest"))
                .expect("test redirect URL must parse"),
        )))
        .expect("redirected media request must succeed");
        assert_eq!(response.bytes(), b"ok");
        let forwarded_request = request_receiver
            .recv()
            .expect("redirected request must be captured")
            .to_ascii_lowercase();
        assert!(!forwarded_request.contains("authorization:"));
        assert!(!forwarded_request.contains("cookie:"));

        redirect_worker.join().expect("redirect server must finish");
        destination_worker
            .join()
            .expect("destination server must finish");
    }

    #[test]
    fn response_exposes_origin_validators_for_cache_identity() {
        let server = TcpListener::bind(("127.0.0.1", 0)).expect("test server must bind");
        let address = server.local_addr().expect("test address must exist");
        let worker = thread::spawn(move || {
            let (mut socket, _) = server.accept().expect("test request must arrive");
            let _request = read_http_request(&mut socket);
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nETag: \"revision-7\"\r\nLast-Modified: Wed, 15 Jul 2026 10:00:00 GMT\r\nConnection: close\r\n\r\nok",
                )
                .expect("test response must write");
        });
        let response = futures::executor::block_on(fetch_media(MediaRequest::new(
            Url::parse(&format!("http://{address}/media")).expect("test URL must be valid"),
            NonZeroUsize::new(16).expect("test response bound must be non-zero"),
        )))
        .expect("test response must succeed");
        assert_eq!(response.validator().etag(), Some("\"revision-7\""));
        assert_eq!(
            response.validator().last_modified(),
            Some("Wed, 15 Jul 2026 10:00:00 GMT"),
        );
        worker.join().expect("test server must finish");
    }

    #[test]
    fn incremental_response_opens_before_its_body_completes() {
        let server = TcpListener::bind(("127.0.0.1", 0)).expect("test server must bind");
        let address = server.local_addr().expect("test address must exist");
        let (first_written_tx, first_written_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        let worker = thread::spawn(move || {
            let (mut socket, _) = server.accept().expect("test request must arrive");
            let _request = read_http_request(&mut socket);
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\nConnection: close\r\n\r\nhello ",
                )
                .expect("first response bytes must write");
            socket.flush().expect("first response bytes must flush");
            first_written_tx
                .send(())
                .expect("first-write signal must send");
            release_rx
                .recv()
                .expect("response tail must be explicitly released");
            socket
                .write_all(b"world")
                .expect("response tail must write");
        });
        let (stream_tx, stream_rx) = mpsc::sync_channel(1);
        let client = thread::spawn(move || {
            let result = futures::executor::block_on(open_media_stream(MediaRequest::new(
                Url::parse(&format!("http://{address}/media")).expect("test URL must be valid"),
                NonZeroUsize::new(16).expect("test response bound must be non-zero"),
            )));
            stream_tx
                .send(result)
                .expect("opened media stream must send");
        });
        first_written_rx
            .recv()
            .expect("server must write the first response bytes");
        let mut stream = match stream_rx.recv_timeout(std::time::Duration::from_secs(5)) {
            Ok(result) => result.expect("incremental media response must open"),
            Err(error) => {
                release_tx
                    .send(())
                    .expect("timed-out test must release its server");
                worker.join().expect("test server must finish");
                client.join().expect("test client must finish");
                panic!("media response headers were buffered until body completion: {error}");
            }
        };
        let first = futures::executor::block_on(stream.next_chunk())
            .expect("first body read must succeed")
            .expect("first body chunk must exist");
        assert_eq!(first.as_ref(), b"hello ");
        release_tx
            .send(())
            .expect("test must release the response tail");
        let second = futures::executor::block_on(stream.next_chunk())
            .expect("second body read must succeed")
            .expect("second body chunk must exist");
        assert_eq!(second.as_ref(), b"world");
        assert!(
            futures::executor::block_on(stream.next_chunk())
                .expect("body completion must succeed")
                .is_none()
        );
        stream.finish().expect("completed media stream must finish");
        worker.join().expect("test server must finish");
        client.join().expect("test client must finish");
    }

    #[test]
    fn chunked_transfer_delivers_a_complete_http_chunk_without_successor_data() {
        let first_payload = vec![0x5a_u8; 512];
        let second_payload = vec![0xa5_u8; 64];
        let server_first = first_payload.clone();
        let server_second = second_payload.clone();
        let server = TcpListener::bind(("127.0.0.1", 0)).expect("test server must bind");
        let address = server.local_addr().expect("test address must exist");
        let (first_written_tx, first_written_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        let worker = thread::spawn(move || {
            let (mut socket, _) = server.accept().expect("test request must arrive");
            let _request = read_http_request(&mut socket);
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
                )
                .expect("chunked response header must write");
            write_http_chunk(&mut socket, &server_first);
            socket.flush().expect("first HTTP chunk must flush");
            first_written_tx
                .send(())
                .expect("first-write signal must send");
            release_rx
                .recv()
                .expect("second HTTP chunk must be explicitly released");
            write_http_chunk(&mut socket, &server_second);
            socket
                .write_all(b"0\r\n\r\n")
                .expect("chunked response terminator must write");
        });
        let stream = futures::executor::block_on(open_media_stream(MediaRequest::new(
            Url::parse(&format!("http://{address}/media")).expect("test URL must be valid"),
            NonZeroUsize::new(1_024).expect("test response bound must be non-zero"),
        )))
        .expect("chunked media response must open");
        let (first_body_tx, first_body_rx) = mpsc::sync_channel(1);
        let body_worker = thread::spawn(move || {
            let mut stream = stream;
            let mut received = Vec::new();
            while received.len() < 512 {
                let chunk = futures::executor::block_on(stream.next_chunk())
                    .expect("HTTP chunk read must succeed")
                    .expect("HTTP body must not end inside its first chunk");
                received.extend_from_slice(&chunk);
            }
            first_body_tx
                .send((stream, received))
                .expect("first body data must send");
        });
        first_written_rx
            .recv()
            .expect("server must write the first HTTP chunk");
        let (mut stream, received) = match first_body_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(result) => result,
            Err(error) => {
                release_tx
                    .send(())
                    .expect("timed-out test must release the second chunk");
                let result = first_body_rx
                    .recv()
                    .expect("released body reader must finish");
                body_worker.join().expect("body reader must finish");
                worker.join().expect("test server must finish");
                panic!(
                    "first HTTP chunk required successor data ({error}); received {} bytes after release",
                    result.1.len()
                );
            }
        };
        assert_eq!(received, first_payload);
        release_tx
            .send(())
            .expect("test must release the second HTTP chunk");
        let second = futures::executor::block_on(stream.next_chunk())
            .expect("second HTTP chunk read must succeed")
            .expect("second HTTP chunk must exist");
        assert_eq!(second.as_ref(), second_payload);
        assert!(
            futures::executor::block_on(stream.next_chunk())
                .expect("chunked body completion must succeed")
                .is_none()
        );
        stream.finish().expect("chunked media stream must finish");
        body_worker.join().expect("body reader must finish");
        worker.join().expect("test server must finish");
    }

    #[test]
    fn conditional_revalidation_preserves_cached_revision_on_http_304() {
        let server = TcpListener::bind(("127.0.0.1", 0)).expect("test server must bind");
        let address = server.local_addr().expect("test address must exist");
        let (request_sender, request_receiver) = mpsc::sync_channel(1);
        let worker = thread::spawn(move || {
            let (mut socket, _) = server.accept().expect("test request must arrive");
            let request = read_http_request(&mut socket);
            request_sender
                .send(request)
                .expect("test request capture must send");
            socket
                .write_all(
                    b"HTTP/1.1 304 Not Modified\r\nETag: \"revision-7\"\r\nConnection: close\r\n\r\n",
                )
                .expect("test response must write");
        });
        let cached = MediaValidator {
            etag: Some(String::from("\"revision-7\"")),
            last_modified: Some(String::from("Wed, 15 Jul 2026 10:00:00 GMT")),
        };
        let outcome = futures::executor::block_on(revalidate_media(
            MediaRequest::new(
                Url::parse(&format!("http://{address}/media")).expect("test URL must be valid"),
                NonZeroUsize::new(16).expect("test response bound must be non-zero"),
            ),
            &cached,
        ))
        .expect("test revalidation must succeed");
        assert!(matches!(outcome, MediaRevalidation::NotModified { .. }));
        assert_eq!(outcome.validator().etag(), cached.etag());
        assert_eq!(outcome.validator().last_modified(), cached.last_modified());
        let request = request_receiver
            .recv()
            .expect("test request must be captured")
            .to_ascii_lowercase();
        assert!(request.contains("if-none-match: \"revision-7\""));
        assert!(request.contains("if-modified-since: wed, 15 jul 2026 10:00:00 gmt"));
        worker.join().expect("test server must finish");
    }

    fn authenticated_request(url: Url) -> MediaRequest {
        MediaRequest::new(
            url,
            NonZeroUsize::new(1_024).expect("test response bound is non-zero"),
        )
        .with_header(
            HeaderName::from_static("authorization"),
            HeaderValue::from_static("Bearer waterkit-test-token"),
        )
        .with_header(
            HeaderName::from_static("cookie"),
            HeaderValue::from_static("waterkit_session=test"),
        )
    }

    fn read_http_request(socket: &mut std::net::TcpStream) -> String {
        let mut bytes = [0_u8; 4_096];
        let mut filled = 0_usize;
        loop {
            let read = socket
                .read(&mut bytes[filled..])
                .expect("test request must read");
            assert_ne!(read, 0, "test request ended before its HTTP header");
            filled += read;
            if bytes[..filled]
                .windows(4)
                .any(|window| window == b"\r\n\r\n")
            {
                break;
            }
            assert!(
                filled < bytes.len(),
                "test request exceeded its explicit header bound"
            );
        }
        String::from_utf8(bytes[..filled].to_vec()).expect("test request must be UTF-8")
    }

    fn write_http_chunk(socket: &mut std::net::TcpStream, body: &[u8]) {
        let header = format!("{:X}\r\n", body.len());
        socket
            .write_all(header.as_bytes())
            .expect("HTTP chunk header must write");
        socket.write_all(body).expect("HTTP chunk body must write");
        socket
            .write_all(b"\r\n")
            .expect("HTTP chunk delimiter must write");
    }
}
