use std::time::Duration;

use reqwest::header::{HeaderMap, RETRY_AFTER};

use crate::model::ModelError;

/// Maximum provider error payload retained for local diagnostics.
pub(crate) const MAX_ERROR_BODY_BYTES: usize = 16 * 1024;

/// Returns a status error with a bounded response body for local diagnostics.
pub(crate) async fn error_for_status(
    response: reqwest::Response,
) -> Result<reqwest::Response, ModelError> {
    if response.status().is_success() {
        return Ok(response);
    }
    Err(from_response(response).await)
}

pub(crate) async fn from_response(mut response: reqwest::Response) -> ModelError {
    let retry_after = parse_retry_after(response.headers());
    let status = response.status();
    let mut bytes = Vec::new();
    let mut truncated = false;
    let mut read_failed = false;
    loop {
        let chunk = match response.chunk().await {
            Ok(Some(chunk)) => chunk,
            Ok(None) => break,
            Err(_) => {
                read_failed = true;
                break;
            }
        };
        let remaining = MAX_ERROR_BODY_BYTES.saturating_sub(bytes.len());
        if chunk.len() > remaining {
            bytes.extend_from_slice(&chunk[..remaining]);
            truncated = true;
            break;
        }
        bytes.extend_from_slice(&chunk);
    }

    let mut body = String::from_utf8_lossy(&bytes).into_owned();
    if truncated {
        body.push_str("\n[response body truncated]");
    } else if read_failed {
        body.push_str("\n[response body read failed]");
    }
    ModelError::HttpStatus {
        status,
        body,
        retry_after,
    }
}

/// Parses `Retry-After` delay-seconds values.
///
/// HTTP-date forms are ignored: providers almost always send integer seconds on
/// 429 responses, and date parsing would pull a calendar dependency into this
/// crate solely for a rare wire shape.
pub(crate) fn parse_retry_after(headers: &HeaderMap) -> Option<Duration> {
    let raw = headers.get(RETRY_AFTER)?.to_str().ok()?.trim();
    if raw.is_empty() {
        return None;
    }
    raw.parse::<u64>().ok().map(Duration::from_secs)
}

#[cfg(test)]
#[path = "http_error_tests.rs"]
mod tests;
