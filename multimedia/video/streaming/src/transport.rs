use futures::StreamExt as _;
use std::num::NonZeroUsize;
use url::Url;
use waterkit_video_core::Error;
use zenwave::{
    Client as _, Method, StatusCode,
    header::{
        AUTHORIZATION, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_TYPE, COOKIE, HOST, HeaderMap,
        PROXY_AUTHORIZATION, TRANSFER_ENCODING,
    },
};

pub struct TransportRequest<'a> {
    pub method: Method,
    pub url: &'a Url,
    pub headers: &'a HeaderMap,
    pub body: Option<&'a [u8]>,
    pub maximum_redirects: u8,
    pub forward_cross_origin_credentials: bool,
}

pub async fn send(request: TransportRequest<'_>) -> Result<(Url, zenwave::Response), Error> {
    // WaterKit observes every redirect so its explicit redirect limit,
    // credential policy, and replayable license bodies remain authoritative.
    let mut client = zenwave::raw_client();
    let original_method = request.method.clone();
    let mut method = request.method;
    let mut effective_url = request.url.clone();
    let mut headers = request.headers.clone();
    let mut body = request.body;
    let mut redirects = 0_u8;

    loop {
        headers.remove(HOST);
        headers.remove(CONTENT_LENGTH);
        let mut builder = client
            .method(method.clone(), effective_url.as_str())
            .map_err(|error| Error::Streaming(error.to_string()))?;
        for (name, value) in &headers {
            builder = builder
                .header(name.clone(), value.clone())
                .map_err(|error| Error::Streaming(error.to_string()))?;
        }
        if let Some(bytes) = body {
            builder = builder
                .header(CONTENT_LENGTH, bytes.len().to_string())
                .map_err(|error| Error::Streaming(error.to_string()))?
                .bytes_body(bytes.to_vec());
        }
        let response = builder
            .await
            .map_err(|error| Error::Streaming(error.to_string()))?;
        if response.status() == StatusCode::NOT_MODIFIED || !response.status().is_redirection() {
            return Ok((effective_url, response));
        }
        if redirects == request.maximum_redirects {
            return Err(Error::Streaming(format!(
                "{original_method} {} exceeded {} redirects",
                request.url, request.maximum_redirects
            )));
        }

        let location = response
            .headers()
            .get("location")
            .ok_or_else(|| {
                Error::Streaming(format!(
                    "{method} {effective_url} returned redirect {} without Location",
                    response.status()
                ))
            })?
            .to_str()
            .map_err(|error| Error::Streaming(format!("invalid redirect Location: {error}")))?;
        let redirected_url = effective_url.join(location).map_err(|error| {
            Error::Streaming(format!("invalid redirect URL {location:?}: {error}"))
        })?;
        if !request.forward_cross_origin_credentials
            && !same_origin(&effective_url, &redirected_url)
        {
            strip_credentials(&mut headers);
        }

        let redirected_method = redirect_method(response.status(), &method);
        if redirected_method != method {
            method = redirected_method;
            body = None;
            strip_body_headers(&mut headers);
        }
        effective_url = redirected_url;
        redirects = redirects.saturating_add(1);
    }
}

pub async fn collect_bounded_body(
    response: zenwave::Response,
    maximum_response_bytes: NonZeroUsize,
) -> Result<Vec<u8>, Error> {
    let mut body = response.into_body();
    let mut bytes = Vec::new();
    while let Some(chunk) = body.next().await {
        let chunk = chunk.map_err(|error| Error::Streaming(error.to_string()))?;
        let next_len = bytes.len().checked_add(chunk.len()).ok_or_else(|| {
            Error::Streaming(String::from("response body length overflowed usize"))
        })?;
        if next_len > maximum_response_bytes.get() {
            return Err(Error::Streaming(format!(
                "response body exceeded the {maximum_response_bytes}-byte limit"
            )));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn redirect_method(status: StatusCode, current: &Method) -> Method {
    if status == StatusCode::SEE_OTHER
        || matches!(status, StatusCode::MOVED_PERMANENTLY | StatusCode::FOUND)
            && current != Method::GET
            && current != Method::HEAD
    {
        Method::GET
    } else {
        current.clone()
    }
}

pub fn same_origin(left: &Url, right: &Url) -> bool {
    left.scheme() == right.scheme()
        && left.host_str() == right.host_str()
        && left.port_or_known_default() == right.port_or_known_default()
}

pub fn strip_credentials(headers: &mut HeaderMap) {
    headers.remove(AUTHORIZATION);
    headers.remove(COOKIE);
    headers.remove(PROXY_AUTHORIZATION);
}

fn strip_body_headers(headers: &mut HeaderMap) {
    headers.remove(CONTENT_ENCODING);
    headers.remove(CONTENT_LENGTH);
    headers.remove(CONTENT_TYPE);
    headers.remove(TRANSFER_ENCODING);
}
