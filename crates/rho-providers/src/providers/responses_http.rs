//! Credential-aware HTTP transport for Responses create/compact.
//!
//! Shared by every provider that speaks the OpenAI Responses wire shape
//! (OpenAI API key, Codex, custom Responses hosts, xAI). The transport owns
//! URL assembly, auth headers, cancellation, and the single refresh-on-`401`
//! retry. Body shape and stream parsing live with the caller.

use serde_json::Value;

use crate::{
    auth::xai_token::XaiAuthManager, credentials::CodexTokens, model::ModelError,
    provider_backend::cancel::cancel_aware,
};

use super::openai::auth::{refresh_codex_token_at, Auth, CodexAuthSource};

const DEFAULT_CODEX_REFRESH_URL: &str = "https://auth.openai.com/oauth/token";

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

/// Credential the transport should present on a Responses request.
///
/// Refresh policy is per variant: `Keyless` and `ApiKey` never refresh, `Codex`
/// refreshes through the OpenAI OAuth token endpoint, and `Xai` defers to
/// [`XaiAuthManager::force_refresh`].
#[derive(Clone, Copy)]
pub(crate) enum ResponsesAuth<'a> {
    Keyless,
    ApiKey(&'a str),
    Codex(&'a Auth),
    Xai(&'a XaiAuthManager),
}

impl<'a> ResponsesAuth<'a> {
    /// Maps the OpenAI provider's optional credential onto a transport auth.
    ///
    /// `None` is a keyless custom host; `Auth::ApiKey` and `Auth::Codex` map
    /// directly.
    pub(crate) fn from_openai(auth: Option<&'a Auth>) -> Self {
        match auth {
            None => Self::Keyless,
            Some(Auth::ApiKey(key)) => Self::ApiKey(key),
            Some(auth @ Auth::Codex { .. }) => Self::Codex(auth),
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

/// Shared Responses HTTP client used by API-key turns, Codex HTTP fallback,
/// xAI turns, and compact.
pub(crate) struct ResponsesHttpTransport<'a> {
    client: &'a reqwest::Client,
    api_base: &'a str,
    codex_refresh_url: &'a str,
}

/// Per-request auth material resolved from a [`ResponsesAuth`].
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
    Xai {
        access_token: &'a str,
    },
}

impl<'a> ResponsesHttpAuth<'a> {
    fn codex(tokens: &'a CodexTokens) -> Self {
        Self::Codex {
            access_token: &tokens.access_token,
            account_id: tokens.account_id.as_deref(),
        }
    }
}

impl<'a> ResponsesHttpTransport<'a> {
    pub(crate) fn new(client: &'a reqwest::Client, api_base: &'a str) -> Self {
        Self {
            client,
            api_base,
            codex_refresh_url: DEFAULT_CODEX_REFRESH_URL,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_codex_refresh_url(mut self, url: &'a str) -> Self {
        self.codex_refresh_url = url;
        self
    }

    /// Posts JSON and, for refreshable credentials, refreshes once on `401`.
    ///
    /// Failed physical auth attempts are reported in the typed result so callers
    /// can account for them without an out-of-band retry callback. Once a `401`
    /// is eligible for refresh, the authentication failed attempt is recorded
    /// immediately and survives refresh failure, cancellation, and retry-send
    /// failure as well as a successful retry response. A `401` that cannot be
    /// refreshed is the final response and records no failed attempt.
    pub(crate) async fn post_json(
        &self,
        auth: ResponsesAuth<'_>,
        endpoint: ResponsesEndpoint,
        body: &Value,
        cancellation: Option<&rho_sdk::CancellationToken>,
    ) -> ResponsesHttpResult {
        match auth {
            ResponsesAuth::Keyless => {
                self.send_final(endpoint, body, ResponsesHttpAuth::Keyless, cancellation)
                    .await
            }
            ResponsesAuth::ApiKey(key) => {
                self.send_final(
                    endpoint,
                    body,
                    ResponsesHttpAuth::ApiKey { key },
                    cancellation,
                )
                .await
            }
            ResponsesAuth::Codex(auth) => self.post_codex(auth, endpoint, body, cancellation).await,
            ResponsesAuth::Xai(auth) => self.post_xai(auth, endpoint, body, cancellation).await,
        }
    }

    async fn send_final(
        &self,
        endpoint: ResponsesEndpoint,
        body: &Value,
        auth: ResponsesHttpAuth<'_>,
        cancellation: Option<&rho_sdk::CancellationToken>,
    ) -> ResponsesHttpResult {
        match self.send(endpoint, body, auth, cancellation).await {
            Ok(response) => ResponsesHttpResult::ok(response),
            Err(error) => ResponsesHttpResult::err(error),
        }
    }

    async fn post_codex(
        &self,
        auth: &Auth,
        endpoint: ResponsesEndpoint,
        body: &Value,
        cancellation: Option<&rho_sdk::CancellationToken>,
    ) -> ResponsesHttpResult {
        let Auth::Codex { source, .. } = auth else {
            return ResponsesHttpResult::err(ModelError::InvalidResponse(
                "Codex tokens requested for non-Codex auth".into(),
            ));
        };
        let source = *source;
        let tokens = match auth.codex_tokens_for_request() {
            Ok(tokens) => tokens,
            Err(error) => return ResponsesHttpResult::err(error),
        };
        let response = match self
            .send(
                endpoint,
                body,
                ResponsesHttpAuth::codex(&tokens),
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
                return ResponsesHttpResult::err(error).with_failed_attempts(failed_attempts);
            }
        };
        auth.remember_refreshed_codex_tokens(refreshed.clone());
        self.send_final(
            endpoint,
            body,
            ResponsesHttpAuth::codex(&refreshed),
            cancellation,
        )
        .await
        .with_failed_attempts(failed_attempts)
    }

    async fn post_xai(
        &self,
        auth: &XaiAuthManager,
        endpoint: ResponsesEndpoint,
        body: &Value,
        cancellation: Option<&rho_sdk::CancellationToken>,
    ) -> ResponsesHttpResult {
        let material = match cancel_aware(cancellation, auth.auth_material()).await {
            Ok(material) => material,
            Err(error) => return ResponsesHttpResult::err(error),
        };
        let response = match self
            .send(
                endpoint,
                body,
                ResponsesHttpAuth::Xai {
                    access_token: &material.access_token,
                },
                cancellation,
            )
            .await
        {
            Ok(response) => response,
            Err(error) => return ResponsesHttpResult::err(error),
        };
        if response.status() != reqwest::StatusCode::UNAUTHORIZED {
            return ResponsesHttpResult::ok(response);
        }
        // No-refresh 401 remains a final response with no prior failed attempt.
        let refreshed =
            match cancel_aware(cancellation, auth.force_refresh(&material.access_token)).await {
                Ok(None) => return ResponsesHttpResult::ok(response),
                Ok(Some(refreshed)) => refreshed,
                Err(error) => {
                    // Refresh was attempted (store credentials); count the prior 401.
                    return ResponsesHttpResult::err(error).with_failed_attempts(vec![
                        ResponsesFailedAttempt {
                            kind: ResponsesFailedAttemptKind::Authentication,
                        },
                    ]);
                }
            };
        let failed_attempts = vec![ResponsesFailedAttempt {
            kind: ResponsesFailedAttemptKind::Authentication,
        }];
        self.send_final(
            endpoint,
            body,
            ResponsesHttpAuth::Xai {
                access_token: &refreshed.access_token,
            },
            cancellation,
        )
        .await
        .with_failed_attempts(failed_attempts)
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
            ResponsesHttpAuth::Xai { access_token } => {
                request = request
                    .bearer_auth(access_token)
                    .header("User-Agent", crate::rho_user_agent());
            }
        }
        request
    }

    async fn send(
        &self,
        endpoint: ResponsesEndpoint,
        body: &Value,
        auth: ResponsesHttpAuth<'_>,
        cancellation: Option<&rho_sdk::CancellationToken>,
    ) -> Result<reqwest::Response, ModelError> {
        let request = self.build_request(endpoint, body, auth);
        cancel_aware(cancellation, async { Ok(request.send().await?) }).await
    }
}

#[cfg(test)]
#[path = "responses_http_tests.rs"]
mod tests;
