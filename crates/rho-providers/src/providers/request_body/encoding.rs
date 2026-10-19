//! Buffered JSON request encoding, independent of network I/O.

use std::io::Write;

use flate2::{write::GzEncoder, Compression};
use reqwest::header::{HeaderValue, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_TYPE};

use super::{EndpointCompression, RequestCompression};

// A conservative large-payload cutoff, not a request-size limit. The benchmark
// sweeps 1 KiB–1 MiB and records CPU and paced-upload costs; see the docs report.
pub(super) const MIN_COMPRESSED_BYTES: usize = 16 * 1024;

// zstd's own default level. On a 1 MB Rho session body, level 3 produced
// 153,718 bytes in 1.9 ms versus 169,343 bytes in 1.5 ms at level 1; the extra
// 0.4 ms of CPU is repaid by 15 KB less upload at any WAN bandwidth below
// ~300 Mbit/s.
const ZSTD_LEVEL: i32 = 3;

/// Compresses a buffered JSON body when `policy` names an encoding for its URL.
/// Existing content encodings and streaming bodies are left to their owner.
pub(crate) fn encode_request(
    request: &mut reqwest::Request,
    policy: EndpointCompression,
) -> std::io::Result<()> {
    let Some(encoding) = policy.encoding_for(request.url()) else {
        return Ok(());
    };
    if request.headers().contains_key(CONTENT_ENCODING)
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
    if bytes.len() < MIN_COMPRESSED_BYTES {
        return Ok(());
    }
    let compressed = match encoding {
        RequestCompression::Gzip => {
            let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
            encoder.write_all(bytes)?;
            encoder.finish()?
        }
        RequestCompression::Zstd => zstd::bulk::compress(bytes, ZSTD_LEVEL)?,
    };
    if compressed.len() >= bytes.len() {
        return Ok(());
    }
    *request.body_mut() = Some(compressed.into());
    request.headers_mut().remove(CONTENT_LENGTH);
    request.headers_mut().insert(
        CONTENT_ENCODING,
        HeaderValue::from_static(encoding.as_str()),
    );
    Ok(())
}

/// Built-in endpoints verified to decode gzip request bodies.
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
