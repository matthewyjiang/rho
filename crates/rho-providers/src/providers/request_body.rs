//! Request-body compression for inference HTTP transports.
//!
//! Support belongs to the actual endpoint, not its JSON dialect or provider
//! identity. Unknown hosts and endpoint overrides stay byte-for-byte JSON; see
//! docs/providers/request-compression.md for the evidence behind this allowlist.

use crate::model::ModelError;

mod encoding;

pub(crate) use encoding::encode_request;

/// Sends the serialized request without changing response/SSE negotiation.
/// In particular, `Accept-Encoding` is not request compression.
pub(crate) async fn send(
    builder: reqwest::RequestBuilder,
) -> Result<reqwest::Response, ModelError> {
    let (client, request) = builder.build_split();
    let mut request = request?;
    encode_request(&mut request)?;
    Ok(client.execute(request).await?)
}

#[cfg(test)]
#[path = "request_body_tests.rs"]
mod tests;
