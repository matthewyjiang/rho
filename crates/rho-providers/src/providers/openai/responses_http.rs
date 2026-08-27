//! Credential-aware HTTP transport for OpenAI Responses create/compact.

use serde_json::Value;

use crate::{credentials::CodexTokens, model::ModelError, provider_backend::cancel::cancel_aware};

use super::auth::{refresh_codex_token_at, Auth, CodexAuthSource};

const DEFAULT_CODEX_REFRESH_URL: &str = "https://auth.openai.com/oauth/token";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ResponsesEndpoint {
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
pub(super) enum ResponsesFailedAttemptKind {
    Authentication,
}

/// One physical request that failed before the transport retried.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ResponsesFailedAttempt {
    pub(super) kind: ResponsesFailedAttemptKind,
}

/// Responses HTTP post outcome plus any failed physical attempts.
///
/// `response` is `Ok` for a final HTTP response (including non-success status)
/// and `Err` when the transport fails before producing one. Failed auth attempts
/// that preceded a refresh/retry are retained on both success and error paths.
#[derive(Debug)]
pub(super) struct ResponsesHttpResult {
    pub(super) response: Result<reqwest::Response, ModelError>,
    pub(super) failed_attempts: Vec<ResponsesFailedAttempt>,
}

impl ResponsesHttpResult {
    fn ok(response: reqwest::Response) -> Self {
        Self {
            response: Ok(response),
            failed_attempts: Vec::new(),
        }
    }

    fn err(error: ModelError) -> Self {
        Self {
            response: Err(error),
            failed_attempts: Vec::new(),
        }
    }

    fn with_failed_attempts(mut self, failed_attempts: Vec<ResponsesFailedAttempt>) -> Self {
        self.failed_attempts = failed_attempts;
        self
    }
}

/// Shared Responses HTTP client used by API-key turns, Codex HTTP fallback, and compact.
pub(super) struct ResponsesHttpTransport<'a> {
    client: &'a reqwest::Client,
    api_base: &'a str,
    codex_refresh_url: &'a str,
}

impl<'a> ResponsesHttpTransport<'a> {
    pub(super) fn new(client: &'a reqwest::Client, api_base: &'a str) -> Self {
        Self {
            client,
            api_base,
            codex_refresh_url: DEFAULT_CODEX_REFRESH_URL,
        }
    }

    #[cfg(test)]
    pub(super) fn with_codex_refresh_url(mut self, url: &'a str) -> Self {
        self.codex_refresh_url = url;
        self
    }

    /// Posts JSON and, for Codex credentials, refreshes once on `401`.
    ///
    /// Failed physical auth attempts are reported in the typed result so callers
    /// can account for them without an out-of-band retry callback. Once a `401`
    /// is eligible for refresh, the authentication failed attempt is recorded
    /// immediately and survives refresh failure, cancellation, and retry-send
    /// failure as well as a successful retry response.
    pub(super) async fn post_json(
        &self,
        auth: Option<&Auth>,
        endpoint: ResponsesEndpoint,
        body: &Value,
        cancellation: Option<&rho_sdk::CancellationToken>,
    ) -> ResponsesHttpResult {
        match auth {
            None => {
                let request = self.build_request(endpoint, body, ResponsesHttpAuth::Keyless);
                match self.send(request, cancellation).await {
                    Ok(response) => ResponsesHttpResult::ok(response),
                    Err(error) => ResponsesHttpResult::err(error),
                }
            }
            Some(Auth::ApiKey(key)) => {
                let request = self.build_request(endpoint, body, ResponsesHttpAuth::ApiKey { key });
                match self.send(request, cancellation).await {
                    Ok(response) => ResponsesHttpResult::ok(response),
                    Err(error) => ResponsesHttpResult::err(error),
                }
            }
            Some(auth @ Auth::Codex { source, .. }) => {
                let source = *source;
                let tokens = match auth.codex_tokens_for_request() {
                    Ok(tokens) => tokens,
                    Err(error) => return ResponsesHttpResult::err(error),
                };
                let response = match self
                    .send(
                        self.build_request(
                            endpoint,
                            body,
                            ResponsesHttpAuth::Codex {
                                access_token: &tokens.access_token,
                                account_id: tokens.account_id.as_deref(),
                            },
                        ),
                        cancellation,
                    )
                    .await
                {
                    Ok(response) => response,
                    // Initial send failure has no preceding retry metadata.
                    Err(error) => return ResponsesHttpResult::err(error),
                };
                if response.status() != reqwest::StatusCode::UNAUTHORIZED {
                    return ResponsesHttpResult::ok(response);
                }
                // No-refresh 401 remains a final response with no prior failed attempt.
                let Some(refresh_token) = tokens.refresh_token.as_deref() else {
                    return ResponsesHttpResult::ok(response);
                };

                // 401 is retry-eligible: record the auth failure before refresh/retry.
                let failed_attempts = vec![ResponsesFailedAttempt {
                    kind: ResponsesFailedAttemptKind::Authentication,
                }];
                let refreshed = match self
                    .refresh_codex_tokens(auth, refresh_token, source, &tokens, cancellation)
                    .await
                {
                    Ok(tokens) => tokens,
                    Err(error) => {
                        return ResponsesHttpResult::err(error)
                            .with_failed_attempts(failed_attempts);
                    }
                };
                auth.remember_refreshed_codex_tokens(refreshed.clone());
                match self
                    .send(
                        self.build_request(
                            endpoint,
                            body,
                            ResponsesHttpAuth::Codex {
                                access_token: &refreshed.access_token,
                                account_id: refreshed.account_id.as_deref(),
                            },
                        ),
                        cancellation,
                    )
                    .await
                {
                    Ok(response) => {
                        ResponsesHttpResult::ok(response).with_failed_attempts(failed_attempts)
                    }
                    Err(error) => {
                        ResponsesHttpResult::err(error).with_failed_attempts(failed_attempts)
                    }
                }
            }
        }
    }

    async fn refresh_codex_tokens(
        &self,
        auth: &Auth,
        refresh_token: &str,
        source: CodexAuthSource,
        previous: &CodexTokens,
        cancellation: Option<&rho_sdk::CancellationToken>,
    ) -> Result<CodexTokens, ModelError> {
        let Auth::Codex { refresh_store, .. } = auth else {
            return Err(ModelError::InvalidResponse(
                "Codex tokens requested for non-Codex auth".into(),
            ));
        };
        let refresh = refresh_codex_token_at(
            self.client,
            refresh_store.as_ref(),
            refresh_token,
            source,
            previous,
            self.codex_refresh_url,
        );
        cancel_aware(cancellation, refresh).await
    }

    fn build_request(
        &self,
        endpoint: ResponsesEndpoint,
        body: &Value,
        auth: ResponsesHttpAuth<'_>,
    ) -> reqwest::RequestBuilder {
        let url = format!(
            "{}/{}",
            self.api_base.trim_end_matches('/'),
            endpoint.path()
        );
        let mut request = self.client.post(url).json(body);
        match auth {
            ResponsesHttpAuth::Keyless => {
                request = request.header("User-Agent", "rho");
            }
            ResponsesHttpAuth::ApiKey { key } => {
                request = request.bearer_auth(key).header("User-Agent", "rho");
            }
            ResponsesHttpAuth::Codex {
                access_token,
                account_id,
            } => {
                request = request
                    .bearer_auth(access_token)
                    .header("User-Agent", "codex-cli")
                    .header("originator", "codex_cli_rs")
                    .header("OpenAI-Beta", "responses=experimental");
                if let Some(account_id) = account_id {
                    request = request.header("ChatGPT-Account-ID", account_id);
                }
            }
        }
        request
    }

    async fn send(
        &self,
        request: reqwest::RequestBuilder,
        cancellation: Option<&rho_sdk::CancellationToken>,
    ) -> Result<reqwest::Response, ModelError> {
        cancel_aware(cancellation, async { Ok(request.send().await?) }).await
    }
}

#[derive(Clone, Copy, Debug)]
enum ResponsesHttpAuth<'a> {
    Keyless,
    ApiKey {
        key: &'a str,
    },
    Codex {
        access_token: &'a str,
        account_id: Option<&'a str>,
    },
}

#[cfg(test)]
#[path = "responses_http_tests.rs"]
mod tests;
