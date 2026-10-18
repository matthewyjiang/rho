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
/// refused a refresh grant, and otherwise hands the response back. RFC 6749
/// §5.2 answers an expired, revoked, or foreign refresh token with 400
/// `invalid_grant`; a 401 to these fixed-shape public-client requests means
/// the same (Codex reports `refresh_token_expired` and the like there). Only
/// signing in again recovers, and missing credentials tell hosts to ask for
/// that. Any other 400 is a malformed request and fails as an HTTP error; the
/// remaining statuses, such as a 403 from a proxy block, are left to the
/// caller.
pub(crate) async fn reject_refused_refresh(
    response: reqwest::Response,
    auth: &str,
) -> Result<reqwest::Response, crate::model::ModelError> {
    #[derive(serde::Deserialize)]
    struct OAuthError {
        error: Option<String>,
    }
    let status = response.status();
    match status.as_u16() {
        401 => Err(crate::model::registry::missing_credentials_error(auth)),
        400 => {
            let body = response.text().await.unwrap_or_default();
            let code = serde_json::from_str::<OAuthError>(&body)
                .ok()
                .and_then(|error| error.error);
            match code.as_deref() {
                Some("invalid_grant") => {
                    Err(crate::model::registry::missing_credentials_error(auth))
                }
                _ => Err(crate::model::ModelError::HttpStatus {
                    status,
                    body,
                    retry_after: None,
                }),
            }
        }
        _ => Ok(response),
    }
}
