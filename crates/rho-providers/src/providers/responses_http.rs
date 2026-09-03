//! Shared Responses HTTP transport.
//!
//! Owns URL assembly, applied request headers, cancellation, and one
//! refresh-on-`401` retry. Credential policy (which headers to send, whether
//! and how to refresh) stays with the calling provider.

use std::future::Future;

use serde_json::Value;

use crate::{model::ModelError, provider_backend::cancel::cancel_aware};

/// Applied headers for one Responses HTTP request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ResponsesHttpAuth {
    bearer: Option<String>,
    user_agent: String,
    extra_headers: Vec<(String, String)>,
}

impl ResponsesHttpAuth {
    /// Keyless custom host: `User-Agent: rho` and no `Authorization`.
    pub(crate) fn keyless() -> Self {
        Self {
            bearer: None,
            user_agent: "rho".into(),
            extra_headers: Vec::new(),
        }
    }

    /// API-key host: bearer token plus `User-Agent: rho`.
    pub(crate) fn api_key(key: impl Into<String>) -> Self {
        Self {
            bearer: Some(key.into()),
            user_agent: "rho".into(),
            extra_headers: Vec::new(),
        }
    }

    /// Bearer token with a caller-chosen user agent (Codex, xAI).
    pub(crate) fn bearer(token: impl Into<String>, user_agent: impl Into<String>) -> Self {
        Self {
            bearer: Some(token.into()),
            user_agent: user_agent.into(),
            extra_headers: Vec::new(),
        }
    }

    /// Adds one extra request header (Codex originator, account id, beta).
    pub(crate) fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.extra_headers.push((name.into(), value.into()));
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResponsesEndpoint {
    Create,
    Compact,
}

impl ResponsesEndpoint {
    fn path(self) -> &'static str {
        match self {
            Self::Create => "responses",
            Self::Compact => "responses/compact",
        }
    }
}

/// Why a physical Responses HTTP attempt failed before an internal retry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResponsesFailedAttemptKind {
    Authentication,
}

impl ResponsesFailedAttemptKind {
    pub(crate) fn provider_error_kind(self) -> rho_sdk::ProviderErrorKind {
        match self {
            Self::Authentication => rho_sdk::ProviderErrorKind::Authentication,
        }
    }
}

/// One physical request that failed before the transport retried.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ResponsesFailedAttempt {
    pub(crate) kind: ResponsesFailedAttemptKind,
}

/// Responses HTTP post outcome plus any failed physical attempts.
///
/// `response` is `Ok` for a final HTTP response (including non-success status)
/// and `Err` when the transport fails before producing one. Failed auth attempts
/// that preceded a refresh/retry are retained on both success and error paths.
#[derive(Debug)]
pub(crate) struct ResponsesHttpResult {
    pub(crate) response: Result<reqwest::Response, ModelError>,
    pub(crate) failed_attempts: Vec<ResponsesFailedAttempt>,
}

impl ResponsesHttpResult {
    fn ok(response: reqwest::Response) -> Self {
        Self {
            response: Ok(response),
            failed_attempts: Vec::new(),
        }
    }

    pub(crate) fn err(error: ModelError) -> Self {
        Self {
            response: Err(error),
            failed_attempts: Vec::new(),
        }
    }

    fn with_failed_attempts(mut self, failed_attempts: Vec<ResponsesFailedAttempt>) -> Self {
        self.failed_attempts = failed_attempts;
        self
    }

    /// Converts transport failed attempts into the SDK native-compaction shape.
    pub(crate) fn native_failed_attempts(
        &self,
    ) -> Vec<rho_sdk::provider::NativeCompactionFailedAttempt> {
        self.failed_attempts
            .iter()
            .map(|attempt| {
                rho_sdk::provider::NativeCompactionFailedAttempt::new(
                    attempt.kind.provider_error_kind(),
                    crate::model::ModelUsage::default(),
                )
            })
            .collect()
    }
}

fn authentication_failed_attempt() -> Vec<ResponsesFailedAttempt> {
    vec![ResponsesFailedAttempt {
        kind: ResponsesFailedAttemptKind::Authentication,
    }]
}

/// After a completed POST, refresh once on `401` and retry.
///
/// `refresh` returning `Ok(None)` means the `401` is final and records no
/// failed attempt. `Ok(Some(auth))` records an authentication failed
/// attempt, runs `before_retry`, then retries once. `before_retry` errors
/// keep that attempt and skip the retry POST. `refresh` `Err` (including
/// cancel) keeps the attempt and does not retry.
pub(crate) async fn post_with_optional_refresh<Auth, F, Fut, N, R, RFut>(
    response: reqwest::Response,
    refresh: F,
    before_retry: N,
    retry: R,
    cancellation: Option<&rho_sdk::CancellationToken>,
) -> ResponsesHttpResult
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<Option<Auth>, ModelError>> + Send,
    N: FnOnce() -> Result<(), ModelError>,
    R: FnOnce(Auth) -> RFut,
    RFut: Future<Output = Result<reqwest::Response, ModelError>> + Send,
{
    if response.status() != reqwest::StatusCode::UNAUTHORIZED {
        return ResponsesHttpResult::ok(response);
    }

    match cancel_aware(cancellation, refresh()).await {
        Ok(None) => ResponsesHttpResult::ok(response),
        Ok(Some(refreshed)) => {
            let failed_attempts = authentication_failed_attempt();
            if let Err(error) = before_retry() {
                return ResponsesHttpResult::err(error).with_failed_attempts(failed_attempts);
            }
            match retry(refreshed).await {
                Ok(response) => {
                    ResponsesHttpResult::ok(response).with_failed_attempts(failed_attempts)
                }
                Err(error) => ResponsesHttpResult::err(error).with_failed_attempts(failed_attempts),
            }
        }
        Err(error) => {
            ResponsesHttpResult::err(error).with_failed_attempts(authentication_failed_attempt())
        }
    }
}

/// Shared Responses HTTP client used by API-key turns, Codex HTTP fallback,
/// xAI turns, and compact.
pub(crate) struct ResponsesHttpTransport<'a> {
    client: &'a reqwest::Client,
    api_base: &'a str,
}

impl<'a> ResponsesHttpTransport<'a> {
    pub(crate) fn new(client: &'a reqwest::Client, api_base: &'a str) -> Self {
        Self { client, api_base }
    }

    /// Posts JSON with no refresh. A `401` is the final response.
    pub(crate) async fn post_json(
        &self,
        auth: &ResponsesHttpAuth,
        endpoint: ResponsesEndpoint,
        body: &Value,
        cancellation: Option<&rho_sdk::CancellationToken>,
    ) -> ResponsesHttpResult {
        match self.send(endpoint, body, auth, cancellation).await {
            Ok(response) => ResponsesHttpResult::ok(response),
            Err(error) => ResponsesHttpResult::err(error),
        }
    }

    /// Posts JSON and, on `401`, invokes `refresh` once.
    ///
    /// See [`post_with_optional_refresh`] for the retry contract. Credential
    /// policy stays with the caller.
    pub(crate) async fn post_json_refreshing<F, Fut, N>(
        &self,
        auth: &ResponsesHttpAuth,
        refresh: F,
        before_retry: N,
        endpoint: ResponsesEndpoint,
        body: &Value,
        cancellation: Option<&rho_sdk::CancellationToken>,
    ) -> ResponsesHttpResult
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<Option<ResponsesHttpAuth>, ModelError>> + Send,
        N: FnOnce() -> Result<(), ModelError>,
    {
        let response = match self.send(endpoint, body, auth, cancellation).await {
            Ok(response) => response,
            Err(error) => return ResponsesHttpResult::err(error),
        };
        post_with_optional_refresh(
            response,
            refresh,
            before_retry,
            |refreshed| async move {
                self.post_json(&refreshed, endpoint, body, cancellation)
                    .await
                    .response
            },
            cancellation,
        )
        .await
    }

    fn build_request(
        &self,
        endpoint: ResponsesEndpoint,
        body: &Value,
        auth: &ResponsesHttpAuth,
    ) -> reqwest::RequestBuilder {
        let url = format!(
            "{}/{}",
            self.api_base.trim_end_matches('/'),
            endpoint.path()
        );
        let mut request = self
            .client
            .post(url)
            .json(body)
            .header("User-Agent", &auth.user_agent);
        if let Some(bearer) = &auth.bearer {
            request = request.bearer_auth(bearer);
        }
        for (name, value) in &auth.extra_headers {
            request = request.header(name, value);
        }
        request
    }

    async fn send(
        &self,
        endpoint: ResponsesEndpoint,
        body: &Value,
        auth: &ResponsesHttpAuth,
        cancellation: Option<&rho_sdk::CancellationToken>,
    ) -> Result<reqwest::Response, ModelError> {
        let request = self.build_request(endpoint, body, auth);
        cancel_aware(cancellation, async { Ok(request.send().await?) }).await
    }
}

#[cfg(test)]
#[path = "responses_http_tests.rs"]
mod tests;
