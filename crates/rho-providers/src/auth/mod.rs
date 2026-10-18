pub mod browser;
pub mod codex_oauth;
mod device_code;
pub mod github_copilot_device;
pub mod github_copilot_token;
pub mod kimi_oauth;
pub mod kimi_token;
pub mod login_dispatch;
pub mod login_prompt;
pub(crate) mod loopback;
pub mod meta_oauth;
pub mod meta_token;
pub mod ollama_device;
pub mod openrouter_oauth;
pub mod provider_credentials;
pub mod xai_oauth;
pub mod xai_token;

/// Fails with missing credentials for `auth` when an OAuth token endpoint
/// refused a refresh grant. RFC 6749 §5.2 answers an expired, revoked, or
/// foreign refresh token with 400 (`invalid_grant`) or 401, and a refresh
/// request is fixed-shape, so only signing in again recovers. Reporting it as
/// missing credentials tells hosts to ask for that. Other statuses, such as a
/// 403 from a proxy block, stay HTTP errors.
pub(crate) fn reject_refused_refresh(
    status: reqwest::StatusCode,
    auth: &str,
) -> Result<(), crate::model::ModelError> {
    match status.as_u16() {
        400 | 401 => Err(crate::model::registry::missing_credentials_error(auth)),
        _ => Ok(()),
    }
}
