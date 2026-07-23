//! Credential-aware HTTP transport for OpenAI Responses create/compact.

use std::sync::Mutex;

use serde_json::Value;

use crate::{
    credentials::{load_codex_tokens, CodexTokens, CredentialStore},
    model::ModelError,
};

use super::{
    auth::{refresh_codex_token_at, Auth, CodexAuthSource},
    codex_request::ResponsesProfile,
};

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
    profile: &'a ResponsesProfile,
    credential_store: &'a dyn CredentialStore,
    refreshed_codex_tokens: &'a Mutex<Option<CodexTokens>>,
    codex_refresh_url: &'a str,
}

impl<'a> ResponsesHttpTransport<'a> {
    pub(super) fn new(
        client: &'a reqwest::Client,
        api_base: &'a str,
        profile: &'a ResponsesProfile,
        credential_store: &'a dyn CredentialStore,
        refreshed_codex_tokens: &'a Mutex<Option<CodexTokens>>,
    ) -> Self {
        Self {
            client,
            api_base,
            profile,
            credential_store,
            refreshed_codex_tokens,
            codex_refresh_url: DEFAULT_CODEX_REFRESH_URL,
        }
    }

    #[cfg(test)]
    pub(super) fn with_codex_refresh_url(mut self, url: &'a str) -> Self {
        self.codex_refresh_url = url;
        self
    }

    /// Resolves the Codex tokens that should be used for the next request.
    pub(super) fn codex_tokens_for_auth(&self, auth: &Auth) -> Result<CodexTokens, ModelError> {
        let Auth::Codex { tokens, source } = auth else {
            return Err(ModelError::InvalidResponse(
                "Codex tokens requested for non-Codex auth".into(),
            ));
        };
        Ok(self.codex_turn_tokens(tokens, *source))
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
        auth: &Auth,
        endpoint: ResponsesEndpoint,
        body: &Value,
        cancellation: Option<&rho_sdk::CancellationToken>,
    ) -> ResponsesHttpResult {
        match auth {
            Auth::ApiKey(key) => {
                let request = self.build_request(endpoint, body, ResponsesHttpAuth::ApiKey { key });
                match self.send(request, cancellation).await {
                    Ok(response) => ResponsesHttpResult::ok(response),
                    Err(error) => ResponsesHttpResult::err(error),
                }
            }
            Auth::Codex { tokens, source } => {
                let tokens = self.codex_turn_tokens(tokens, *source);
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
                    .refresh_codex_tokens(refresh_token, *source, &tokens, cancellation)
                    .await
                {
                    Ok(tokens) => tokens,
                    Err(error) => {
                        return ResponsesHttpResult::err(error)
                            .with_failed_attempts(failed_attempts);
                    }
                };
                self.remember_refreshed_codex_tokens(refreshed.clone());
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
        refresh_token: &str,
        source: CodexAuthSource,
        previous: &CodexTokens,
        cancellation: Option<&rho_sdk::CancellationToken>,
    ) -> Result<CodexTokens, ModelError> {
        let refresh = refresh_codex_token_at(
            self.client,
            self.credential_store,
            refresh_token,
            source,
            previous,
            self.codex_refresh_url,
        );
        match cancellation {
            Some(cancellation) => tokio::select! {
                result = refresh => result,
                () = cancellation.cancelled() => Err(ModelError::Interrupted),
            },
            None => refresh.await,
        }
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
                    .header("originator", "codex_cli_rs");
                // Compact historically sent the experimental beta header; create
                // HTTP fallback did not. Keep that split.
                if endpoint == ResponsesEndpoint::Compact {
                    request = request.header("OpenAI-Beta", "responses=experimental");
                }
                if self.profile.mode().uses_responses_lite() {
                    request = request.header("x-openai-internal-codex-responses-lite", "true");
                }
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
        match cancellation {
            Some(cancellation) => tokio::select! {
                response = request.send() => Ok(response?),
                () = cancellation.cancelled() => Err(ModelError::Interrupted),
            },
            None => Ok(request.send().await?),
        }
    }

    fn codex_turn_tokens(&self, initial: &CodexTokens, source: CodexAuthSource) -> CodexTokens {
        if source == CodexAuthSource::Store {
            if let Ok(Some(tokens)) = load_codex_tokens(self.credential_store) {
                return tokens;
            }
        }
        self.refreshed_codex_tokens
            .lock()
            .ok()
            .and_then(|guard| guard.clone())
            .unwrap_or_else(|| initial.clone())
    }

    fn remember_refreshed_codex_tokens(&self, tokens: CodexTokens) {
        if let Ok(mut guard) = self.refreshed_codex_tokens.lock() {
            *guard = Some(tokens);
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum ResponsesHttpAuth<'a> {
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
