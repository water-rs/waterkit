use std::{
    num::{NonZeroU64, NonZeroUsize},
    str::FromStr as _,
    time::Duration,
};

use bytes::Bytes;
use url::Url;
use waterkit_video_core::Error;
use zenwave::header::{HeaderMap, HeaderName, HeaderValue};

use crate::{
    BandwidthEstimator, DashInitialization, DashSegmentReference, HlsInitializationSegment,
    HlsPartialSegment, HlsSegment, MediaByteRange, MediaRequest, MediaStream, fetch_media,
    open_media_stream,
};

/// One bounded segment resource with ordered CDN candidates.
#[derive(Debug, Clone)]
pub struct SegmentResource {
    candidates: Vec<Url>,
    byte_range: Option<MediaByteRange>,
    maximum_response_bytes: NonZeroUsize,
    headers: HeaderMap,
    request_context: Option<MediaRequest>,
}

impl SegmentResource {
    /// Creates a bounded whole-resource request.
    ///
    /// # Errors
    ///
    /// Returns an error when no candidate URL is supplied.
    pub fn new(
        candidates: impl IntoIterator<Item = Url>,
        maximum_response_bytes: NonZeroUsize,
    ) -> Result<Self, Error> {
        let candidates = candidates.into_iter().collect::<Vec<_>>();
        if candidates.is_empty() {
            return Err(Error::Streaming(String::from(
                "segment resource requires at least one candidate URL",
            )));
        }
        Ok(Self {
            candidates,
            byte_range: None,
            maximum_response_bytes,
            headers: HeaderMap::new(),
            request_context: None,
        })
    }

    /// Creates an exact byte-range segment request.
    ///
    /// # Errors
    ///
    /// Returns an error when no candidate is supplied or the requested length
    /// cannot fit in memory on the current architecture.
    pub fn ranged(
        candidates: impl IntoIterator<Item = Url>,
        byte_range: MediaByteRange,
    ) -> Result<Self, Error> {
        let maximum_response_bytes = usize::try_from(byte_range.len())
            .ok()
            .and_then(NonZeroUsize::new)
            .ok_or_else(|| {
                Error::Streaming(String::from(
                    "segment byte range length does not fit the current architecture",
                ))
            })?;
        let mut resource = Self::new(candidates, maximum_response_bytes)?;
        resource.byte_range = Some(byte_range);
        Ok(resource)
    }

    /// Creates a resource from one HLS media segment.
    ///
    /// # Errors
    ///
    /// Returns an error when the whole-resource bound is invalid for the
    /// segment's addressing mode.
    pub fn for_hls(
        segment: &HlsSegment,
        maximum_response_bytes: NonZeroUsize,
    ) -> Result<Self, Error> {
        resource_for_url(
            segment.url.clone(),
            segment.byte_range,
            maximum_response_bytes,
        )
    }

    /// Creates a resource from one Low-Latency HLS partial segment.
    ///
    /// # Errors
    ///
    /// Returns an error when the whole-resource bound is invalid for the
    /// part's addressing mode.
    pub fn for_hls_partial(
        part: &HlsPartialSegment,
        maximum_response_bytes: NonZeroUsize,
    ) -> Result<Self, Error> {
        resource_for_url(part.url.clone(), part.byte_range, maximum_response_bytes)
    }

    /// Creates a resource from an HLS Media Initialization Section.
    ///
    /// # Errors
    ///
    /// Returns an error when the whole-resource bound is invalid for the
    /// initialization section's addressing mode.
    pub fn for_hls_initialization(
        initialization: &HlsInitializationSegment,
        maximum_response_bytes: NonZeroUsize,
    ) -> Result<Self, Error> {
        resource_for_url(
            initialization.url.clone(),
            initialization.byte_range,
            maximum_response_bytes,
        )
    }

    /// Creates a resource from a DASH media reference.
    ///
    /// # Errors
    ///
    /// Returns an error when the reference has no candidates or its byte range
    /// cannot fit in memory.
    pub fn for_dash(
        segment: &DashSegmentReference,
        maximum_response_bytes: NonZeroUsize,
    ) -> Result<Self, Error> {
        resource_for_candidates(
            segment.urls.clone(),
            segment.byte_range,
            maximum_response_bytes,
        )
    }

    /// Creates a resource from a DASH initialization reference.
    ///
    /// # Errors
    ///
    /// Returns an error when the reference has no candidates or its byte range
    /// cannot fit in memory.
    pub fn for_dash_initialization(
        initialization: &DashInitialization,
        maximum_response_bytes: NonZeroUsize,
    ) -> Result<Self, Error> {
        resource_for_candidates(
            initialization.urls.clone(),
            initialization.byte_range,
            maximum_response_bytes,
        )
    }

    /// Adds one validated HTTP request header to every CDN candidate.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid header name or value.
    pub fn with_header(mut self, name: &str, value: &str) -> Result<Self, Error> {
        let name = HeaderName::from_str(name)
            .map_err(|error| Error::Streaming(format!("invalid media header name: {error}")))?;
        let value = HeaderValue::from_str(value)
            .map_err(|error| Error::Streaming(format!("invalid media header value: {error}")))?;
        self.headers.insert(name, value);
        Ok(self)
    }

    /// Inherits authentication, redirect, and related-resource policy from a manifest request.
    #[must_use]
    pub fn with_request_context(mut self, request: &MediaRequest) -> Self {
        self.request_context = Some(request.clone());
        self
    }

    /// Returns ordered candidate URLs.
    #[must_use]
    pub fn candidates(&self) -> &[Url] {
        &self.candidates
    }

    /// Returns the exact requested byte range.
    #[must_use]
    pub const fn byte_range(&self) -> Option<MediaByteRange> {
        self.byte_range
    }

    fn request_for(&self, url: Url) -> Result<MediaRequest, Error> {
        let mut request = match &self.request_context {
            Some(context) => {
                context.related_resource(url, self.byte_range, self.maximum_response_bytes)?
            }
            None => match self.byte_range {
                Some(range) => MediaRequest::ranged(url, range)?,
                None => MediaRequest::new(url, self.maximum_response_bytes),
            },
        };
        for (name, value) in &self.headers {
            request = request.with_header(name.clone(), value.clone());
        }
        Ok(request)
    }
}

/// Completed segment bytes and transport measurements.
#[derive(Debug, Clone)]
pub struct FetchedSegment {
    effective_url: Url,
    bytes: Bytes,
    elapsed: Duration,
    total_resource_bytes: Option<u64>,
    estimated_bits_per_second: NonZeroU64,
}

/// Transfer metadata for one incrementally consumed segment response.
#[derive(Debug, Clone)]
pub struct StreamedSegmentReceipt {
    effective_url: Url,
    elapsed: Duration,
    received_bytes: usize,
    total_resource_bytes: Option<u64>,
    estimated_bits_per_second: NonZeroU64,
}

/// One validated segment response whose body can be consumed incrementally.
#[derive(Debug)]
pub struct SegmentStream {
    candidate: Url,
    stream: MediaStream,
}

impl SegmentStream {
    /// Reads the next Zenwave response-body chunk.
    ///
    /// # Errors
    ///
    /// Returns a streaming error when the selected CDN response fails after
    /// header validation or violates its bounded response contract.
    pub async fn next_chunk(&mut self) -> Result<Option<Bytes>, Error> {
        self.stream.next_chunk().await.map_err(|error| {
            Error::Streaming(format!(
                "streaming segment {} failed: {error}",
                self.candidate
            ))
        })
    }
}

impl FetchedSegment {
    /// Returns the final URL after redirects.
    #[must_use]
    pub const fn effective_url(&self) -> &Url {
        &self.effective_url
    }

    /// Returns the segment bytes.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Consumes the response and returns its bytes.
    #[must_use]
    pub fn into_bytes(self) -> Bytes {
        self.bytes
    }

    /// Returns transfer duration used for bandwidth estimation.
    #[must_use]
    pub const fn elapsed(&self) -> Duration {
        self.elapsed
    }

    /// Returns the total origin resource size when declared.
    #[must_use]
    pub const fn total_resource_bytes(&self) -> Option<u64> {
        self.total_resource_bytes
    }

    /// Returns the conservative estimate after this transfer.
    #[must_use]
    pub const fn estimated_bits_per_second(&self) -> NonZeroU64 {
        self.estimated_bits_per_second
    }
}

impl StreamedSegmentReceipt {
    /// Returns the final URL after redirects.
    #[must_use]
    pub const fn effective_url(&self) -> &Url {
        &self.effective_url
    }

    /// Returns transfer elapsed time.
    #[must_use]
    pub const fn elapsed(&self) -> Duration {
        self.elapsed
    }

    /// Returns the number of body bytes delivered to the consumer.
    #[must_use]
    pub const fn received_bytes(&self) -> usize {
        self.received_bytes
    }

    /// Returns total origin resource size when declared.
    #[must_use]
    pub const fn total_resource_bytes(&self) -> Option<u64> {
        self.total_resource_bytes
    }

    /// Returns the conservative estimate after this transfer.
    #[must_use]
    pub const fn estimated_bits_per_second(&self) -> NonZeroU64 {
        self.estimated_bits_per_second
    }
}

/// Stateful Zenwave segment loader and throughput estimator.
#[derive(Debug)]
pub struct SegmentLoader {
    bandwidth: BandwidthEstimator,
}

impl SegmentLoader {
    /// Creates a loader with a conservative initial throughput estimate.
    #[must_use]
    pub const fn new(initial_bits_per_second: NonZeroU64) -> Self {
        Self {
            bandwidth: BandwidthEstimator::new(initial_bits_per_second),
        }
    }

    /// Loads a segment, trying only the distinct CDN candidates supplied by
    /// the manifest and feeding a successful transfer into the ABR estimator.
    ///
    /// # Errors
    ///
    /// Returns an aggregate streaming error after every declared candidate
    /// fails its request, status, range, or body validation.
    pub async fn load(&mut self, resource: &SegmentResource) -> Result<FetchedSegment, Error> {
        let mut failures = Vec::new();
        for candidate in resource.candidates() {
            let request = resource.request_for(candidate.clone())?;
            match fetch_media(request).await {
                Ok(response) => {
                    let elapsed = response.elapsed();
                    if let Some(transferred) = u64::try_from(response.bytes().len())
                        .ok()
                        .and_then(NonZeroU64::new)
                        && !elapsed.is_zero()
                    {
                        self.bandwidth.add_sample(transferred, elapsed)?;
                    }
                    let effective_url = response.effective_url().clone();
                    let total_resource_bytes = response.total_resource_bytes();
                    return Ok(FetchedSegment {
                        effective_url,
                        bytes: Bytes::from(response.into_bytes()),
                        elapsed,
                        total_resource_bytes,
                        estimated_bits_per_second: self.bandwidth.estimate(),
                    });
                }
                Err(error) => failures.push(format!("{candidate}: {error}")),
            }
        }
        Err(Error::Streaming(format!(
            "all segment candidates failed: {}",
            failures.join("; ")
        )))
    }

    /// Streams a segment to `on_chunk` as Zenwave receives body bytes.
    ///
    /// CDN candidates are tried only until a response passes status and header
    /// validation. Once bytes have been delivered, a body failure is surfaced
    /// directly because replaying another candidate would duplicate decoder state.
    ///
    /// # Errors
    ///
    /// Returns an aggregate error when every candidate fails before body
    /// delivery, or a direct body/consumer error after streaming begins.
    pub async fn stream(
        &mut self,
        resource: &SegmentResource,
        mut on_chunk: impl FnMut(&[u8]) -> Result<(), Error>,
    ) -> Result<StreamedSegmentReceipt, Error> {
        let mut stream = self.open_stream(resource).await?;
        while let Some(chunk) = stream.next_chunk().await? {
            on_chunk(&chunk)?;
        }
        self.finish_stream(stream)
    }

    /// Opens a validated segment response without consuming its body.
    ///
    /// CDN candidates are tried until status, range, and response headers pass
    /// validation. Once returned, the selected response remains authoritative;
    /// body failures never restart on another CDN and duplicate decoder state.
    ///
    /// # Errors
    ///
    /// Returns an aggregate streaming error when every declared CDN candidate
    /// fails before body delivery begins.
    pub async fn open_stream(&self, resource: &SegmentResource) -> Result<SegmentStream, Error> {
        let mut failures = Vec::new();
        loop {
            let Some(candidate) = resource.candidates().get(failures.len()) else {
                return Err(Error::Streaming(format!(
                    "all segment candidates failed before body delivery: {}",
                    failures.join("; ")
                )));
            };
            let request = resource.request_for(candidate.clone())?;
            match open_media_stream(request).await {
                Ok(stream) => {
                    return Ok(SegmentStream {
                        candidate: candidate.clone(),
                        stream,
                    });
                }
                Err(error) => failures.push(format!("{candidate}: {error}")),
            }
        }
    }

    /// Completes a fully consumed incremental segment and records its throughput.
    ///
    /// # Errors
    ///
    /// Returns a streaming error when the body has not reached its explicit end
    /// signal or its transfer measurement cannot update the bandwidth estimator.
    pub fn finish_stream(
        &mut self,
        stream: SegmentStream,
    ) -> Result<StreamedSegmentReceipt, Error> {
        let receipt = stream.stream.finish()?;
        let elapsed = receipt.elapsed();
        if let Some(transferred) = u64::try_from(receipt.received_bytes())
            .ok()
            .and_then(NonZeroU64::new)
            && !elapsed.is_zero()
        {
            self.bandwidth.add_sample(transferred, elapsed)?;
        }
        Ok(StreamedSegmentReceipt {
            effective_url: receipt.effective_url().clone(),
            elapsed,
            received_bytes: receipt.received_bytes(),
            total_resource_bytes: receipt.total_resource_bytes(),
            estimated_bits_per_second: self.bandwidth.estimate(),
        })
    }

    /// Returns the current conservative throughput estimate.
    #[must_use]
    pub fn estimated_bits_per_second(&self) -> NonZeroU64 {
        self.bandwidth.estimate()
    }
}

fn resource_for_url(
    url: Url,
    byte_range: Option<MediaByteRange>,
    maximum_response_bytes: NonZeroUsize,
) -> Result<SegmentResource, Error> {
    resource_for_candidates([url], byte_range, maximum_response_bytes)
}

fn resource_for_candidates(
    candidates: impl IntoIterator<Item = Url>,
    byte_range: Option<MediaByteRange>,
    maximum_response_bytes: NonZeroUsize,
) -> Result<SegmentResource, Error> {
    match byte_range {
        Some(range) => SegmentResource::ranged(candidates, range),
        None => SegmentResource::new(candidates, maximum_response_bytes),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read as _, Write as _},
        net::TcpListener,
        num::{NonZeroU64, NonZeroUsize},
        thread,
    };

    use url::Url;

    use super::{SegmentLoader, SegmentResource};

    #[test]
    fn loader_uses_declared_cdn_failover_and_updates_bandwidth() {
        let server = TcpListener::bind(("127.0.0.1", 0)).expect("test server must bind");
        let address = server.local_addr().expect("test address must exist");
        let worker = thread::spawn(move || {
            for index in 0..2 {
                let (mut stream, _) = server.accept().expect("test request must arrive");
                let mut request = [0_u8; 4_096];
                let read = stream.read(&mut request).expect("test request must read");
                assert!(read > 0);
                let response = if index == 0 {
                    String::from(
                        "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    )
                } else {
                    String::from(
                        "HTTP/1.1 200 OK\r\nContent-Length: 12\r\nConnection: close\r\n\r\nsegment-data",
                    )
                };
                stream
                    .write_all(response.as_bytes())
                    .expect("test response must write");
            }
        });
        let failing =
            Url::parse(&format!("http://{address}/failing")).expect("test URL must be valid");
        let working =
            Url::parse(&format!("http://{address}/working")).expect("test URL must be valid");
        let resource = SegmentResource::new(
            [failing, working.clone()],
            NonZeroUsize::new(1024).expect("test bound must be non-zero"),
        )
        .expect("test resource must be valid");
        let mut loader = SegmentLoader::new(
            NonZeroU64::new(1_000_000).expect("test bandwidth must be non-zero"),
        );
        let segment = futures::executor::block_on(loader.load(&resource))
            .expect("declared failover must succeed");

        worker.join().expect("test server must finish");
        assert_eq!(segment.effective_url(), &working);
        assert_eq!(segment.bytes(), b"segment-data");
    }
}
