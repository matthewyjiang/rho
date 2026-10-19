//! Request-body compression for inference HTTP transports.
//!
//! Support belongs to the actual endpoint, not its JSON dialect or provider
//! identity. Unknown hosts and endpoint overrides stay byte-for-byte JSON
//! unless the user configures an encoding for their own host; see
//! docs/providers/request-compression.md for the evidence behind the allowlist.

use crate::model::ModelError;

mod encoding;

pub(crate) use encoding::encode_request;

/// A `Content-Encoding` an endpoint decodes on large JSON request bodies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum RequestCompression {
    Gzip,
    Zstd,
}

impl RequestCompression {
    /// The `Content-Encoding` token, also the config spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Gzip => "gzip",
            Self::Zstd => "zstd",
        }
    }
}

impl std::str::FromStr for RequestCompression {
    type Err = RequestCompressionParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim() {
            "gzip" => Ok(Self::Gzip),
            "zstd" => Ok(Self::Zstd),
            _ => Err(RequestCompressionParseError),
        }
    }
}

/// Unknown request compression name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RequestCompressionParseError;

impl std::fmt::Display for RequestCompressionParseError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("must be gzip or zstd")
    }
}

impl std::error::Error for RequestCompressionParseError {}

/// Who decides whether a transport's requests are compressed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum EndpointCompression {
    /// Gzip only on built-in endpoints verified to decode it.
    #[default]
    Verified,
    /// The user asserts their own endpoint decodes this encoding.
    Configured(RequestCompression),
}

impl EndpointCompression {
    fn encoding_for(self, url: &url::Url) -> Option<RequestCompression> {
        match self {
            Self::Verified => encoding::supports_gzip(url).then_some(RequestCompression::Gzip),
            Self::Configured(encoding) => Some(encoding),
        }
    }
}

/// Sends the serialized request without changing response/SSE negotiation.
/// In particular, `Accept-Encoding` is not request compression.
pub(crate) async fn send(
    builder: reqwest::RequestBuilder,
    policy: EndpointCompression,
) -> Result<reqwest::Response, ModelError> {
    let (client, request) = builder.build_split();
    let mut request = request?;
    encode_request(&mut request, policy)?;
    Ok(client.execute(request).await?)
}

#[cfg(test)]
#[path = "request_body_tests.rs"]
mod tests;
