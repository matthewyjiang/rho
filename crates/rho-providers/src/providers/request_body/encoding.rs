//! Buffered JSON request encoding, independent of network I/O.

use std::io::Write;

use flate2::{write::GzEncoder, Compression};
use reqwest::header::{CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_TYPE};

// A conservative large-payload cutoff, not a request-size limit. The benchmark
// sweeps 1 KiB–1 MiB and records CPU and paced-upload costs; see the docs report.
pub(super) const MIN_GZIP_BYTES: usize = 16 * 1024;

/// Compresses a buffered JSON body only at an endpoint known to decode gzip.
/// Existing content encodings and streaming bodies are left to their owner.
pub(crate) fn encode_request(request: &mut reqwest::Request) -> std::io::Result<()> {
    if !supports_gzip(request.url())
        || request.headers().contains_key(CONTENT_ENCODING)
        || request
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            != Some("application/json")
    {
        return Ok(());
    }
    let Some(bytes) = request.body().and_then(reqwest::Body::as_bytes) else {
        return Ok(());
    };
    if bytes.len() < MIN_GZIP_BYTES {
        return Ok(());
    }
    let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(bytes)?;
    let compressed = encoder.finish()?;
    if compressed.len() >= bytes.len() {
        return Ok(());
    }
    *request.body_mut() = Some(compressed.into());
    request.headers_mut().remove(CONTENT_LENGTH);
    request.headers_mut().insert(
        CONTENT_ENCODING,
        reqwest::header::HeaderValue::from_static("gzip"),
    );
    Ok(())
}

pub(super) fn supports_gzip(url: &url::Url) -> bool {
    if url.scheme() != "https" || url.port_or_known_default() != Some(443) {
        return false;
    }
    match url.host_str() {
        Some("api.anthropic.com") => url.path() == "/v1/messages",
        Some("api.typesafe.ai") => url.path() == "/v1/systemone",
        Some("api.x.ai") => matches!(
            url.path(),
            "/v1/responses" | "/v1/responses/compact" | "/v1/chat/completions"
        ),
        Some("generativelanguage.googleapis.com") => url
            .path()
            .strip_prefix("/v1beta/models/")
            .and_then(|path| path.rsplit_once(':'))
            .is_some_and(|(model, method)| {
                !model.is_empty()
                    && !model.contains('/')
                    && matches!(method, "generateContent" | "streamGenerateContent")
            }),
        _ => false,
    }
}
