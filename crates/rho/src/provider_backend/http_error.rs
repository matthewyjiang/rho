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
    let status = response.status();
    let mut bytes = Vec::new();
    let mut truncated = false;
    loop {
        let chunk = match response.chunk().await {
            Ok(Some(chunk)) => chunk,
            Ok(None) => break,
            Err(error) => return ModelError::Request(error),
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
    }
    ModelError::HttpStatus { status, body }
}

#[cfg(test)]
#[path = "http_error_tests.rs"]
mod tests;
