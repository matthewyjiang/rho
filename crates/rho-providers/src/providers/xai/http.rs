//! xAI JSON POST transport with optional one-shot auth refresh.

use reqwest::StatusCode;
use serde_json::Value;

use super::XaiProvider;
use crate::model::ModelError;

/// Physical request attempts that failed before the final HTTP result.
///
/// Mirrors OpenAI Responses transport semantics: a no-refresh `401` is the final
/// response and does not produce a prior failed attempt. Only a refresh-eligible
/// `401` records an authentication failed attempt before refresh/retry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum XaiFailedAttempt {
    Authentication,
}

/// Outcome of an xAI JSON POST that may refresh once on `401`.
pub(crate) struct XaiHttpResult {
    pub(crate) response: Result<reqwest::Response, ModelError>,
    pub(crate) failed_attempts: Vec<XaiFailedAttempt>,
}

impl XaiHttpResult {
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

    fn with_failed_attempts(mut self, failed_attempts: Vec<XaiFailedAttempt>) -> Self {
        self.failed_attempts = failed_attempts;
        self
    }
}

impl XaiProvider {
    pub(super) async fn post_json(
        &self,
        path: &str,
        access_token: &str,
        body: &Value,
        cancellation: Option<&rho_sdk::CancellationToken>,
    ) -> Result<reqwest::Response, ModelError> {
        let request = self
            .client
            .post(format!(
                "{}/{}",
                self.api_base.trim_end_matches('/'),
                path.trim_start_matches('/')
            ))
            .bearer_auth(access_token)
            .header("User-Agent", crate::rho_user_agent())
            .json(body);
        match cancellation {
            Some(cancellation) => tokio::select! {
                response = request.send() => Ok(response?),
                () = cancellation.cancelled() => Err(ModelError::Interrupted),
            },
            None => Ok(request.send().await?),
        }
    }

    /// Posts JSON and refreshes once on `401` when credentials allow it.
    ///
    /// `before_retry` runs only after a successful refresh and before the second
    /// POST (create uses this to emit a request-attempt event).
    ///
    /// When `cancellation` is `Some`, credential lookup, both POSTs, and force
    /// refresh cooperate with it for the whole operation.
    pub(super) async fn post_with_auth_retry(
        &self,
        path: &str,
        body: &Value,
        cancellation: Option<&rho_sdk::CancellationToken>,
        before_retry: impl FnOnce() -> Result<(), ModelError>,
    ) -> XaiHttpResult {
        let auth = match cancel_aware(cancellation, self.auth.auth_material()).await {
            Ok(auth) => auth,
            Err(error) => return XaiHttpResult::err(error),
        };
        let response = match self
            .post_json(path, &auth.access_token, body, cancellation)
            .await
        {
            Ok(response) => response,
            Err(error) => return XaiHttpResult::err(error),
        };
        if response.status() != StatusCode::UNAUTHORIZED {
            return XaiHttpResult::ok(response);
        }
        // No-refresh 401 remains a final response with no prior failed attempt.
        let refreshed =
            match cancel_aware(cancellation, self.auth.force_refresh(&auth.access_token)).await {
                Ok(None) => return XaiHttpResult::ok(response),
                Ok(Some(refreshed)) => refreshed,
                Err(error) => {
                    // Refresh was attempted (store credentials); count the prior 401.
                    return XaiHttpResult::err(error)
                        .with_failed_attempts(vec![XaiFailedAttempt::Authentication]);
                }
            };

        let failed_attempts = vec![XaiFailedAttempt::Authentication];
        if let Err(error) = before_retry() {
            return XaiHttpResult::err(error).with_failed_attempts(failed_attempts);
        }
        match self
            .post_json(path, &refreshed.access_token, body, cancellation)
            .await
        {
            Ok(response) => XaiHttpResult::ok(response).with_failed_attempts(failed_attempts),
            Err(error) => XaiHttpResult::err(error).with_failed_attempts(failed_attempts),
        }
    }
}

async fn cancel_aware<T>(
    cancellation: Option<&rho_sdk::CancellationToken>,
    future: impl std::future::Future<Output = Result<T, ModelError>>,
) -> Result<T, ModelError> {
    match cancellation {
        Some(cancellation) => tokio::select! {
            result = future => result,
            () = cancellation.cancelled() => Err(ModelError::Interrupted),
        },
        None => future.await,
    }
}
