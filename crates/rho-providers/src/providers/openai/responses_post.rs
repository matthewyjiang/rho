//! OpenAI/Codex credential policy for the shared Responses HTTP transport.

use serde_json::Value;

use crate::{
    credentials::CodexTokens,
    model::ModelError,
    providers::responses_http::{
        ResponsesEndpoint, ResponsesHttpAuth, ResponsesHttpResult, ResponsesHttpTransport,
    },
};

use super::auth::{refresh_codex_token_at, Auth};

pub(crate) const DEFAULT_CODEX_REFRESH_URL: &str = "https://auth.openai.com/oauth/token";

/// Codex Responses headers: bearer, `codex-cli` UA, originator, beta, account.
pub(crate) fn codex_http_auth(tokens: &CodexTokens) -> ResponsesHttpAuth {
    let mut auth = ResponsesHttpAuth::bearer(&tokens.access_token, "codex-cli")
        .with_header("originator", "codex_cli_rs")
        .with_header("OpenAI-Beta", "responses=experimental");
    if let Some(account_id) = tokens.account_id.as_deref() {
        auth = auth.with_header("ChatGPT-Account-ID", account_id);
    }
    auth
}

/// Posts a Responses create/compact body with OpenAI credential policy.
///
/// Keyless and API-key hosts never refresh. Codex refreshes once on `401`
/// when a refresh token is present.
pub(crate) async fn post(
    http: &ResponsesHttpTransport<'_>,
    client: &reqwest::Client,
    auth: Option<&Auth>,
    refresh_url: &str,
    endpoint: ResponsesEndpoint,
    body: &Value,
    cancellation: Option<&rho_sdk::CancellationToken>,
) -> ResponsesHttpResult {
    match auth {
        None => {
            http.post_json(&ResponsesHttpAuth::keyless(), endpoint, body, cancellation)
                .await
        }
        Some(Auth::ApiKey(key)) => {
            http.post_json(
                &ResponsesHttpAuth::api_key(key),
                endpoint,
                body,
                cancellation,
            )
            .await
        }
        Some(auth) => {
            post_codex(
                http,
                client,
                auth,
                refresh_url,
                endpoint,
                body,
                cancellation,
            )
            .await
        }
    }
}

async fn post_codex(
    http: &ResponsesHttpTransport<'_>,
    client: &reqwest::Client,
    auth: &Auth,
    refresh_url: &str,
    endpoint: ResponsesEndpoint,
    body: &Value,
    cancellation: Option<&rho_sdk::CancellationToken>,
) -> ResponsesHttpResult {
    let Auth::Codex {
        source,
        refresh_store,
        ..
    } = auth
    else {
        return ResponsesHttpResult::err(ModelError::InvalidResponse(
            "Codex tokens requested for non-Codex auth".into(),
        ));
    };
    let tokens = match auth.codex_tokens_for_request() {
        Ok(tokens) => tokens,
        Err(error) => return ResponsesHttpResult::err(error),
    };
    let request_auth = codex_http_auth(&tokens);
    let Some(refresh_token) = tokens.refresh_token.clone() else {
        return http
            .post_json(&request_auth, endpoint, body, cancellation)
            .await;
    };

    let source = *source;
    let refresh_store = refresh_store.clone();
    let previous = tokens;
    let refresh_url = refresh_url.to_string();
    let client = client.clone();
    http.post_json_refreshing(
        &request_auth,
        move || {
            let auth = auth;
            async move {
                let refreshed = refresh_codex_token_at(
                    &client,
                    refresh_store.as_ref(),
                    &refresh_token,
                    source,
                    &previous,
                    &refresh_url,
                )
                .await?;
                auth.remember_refreshed_codex_tokens(refreshed.clone());
                Ok(Some(codex_http_auth(&refreshed)))
            }
        },
        endpoint,
        body,
        cancellation,
    )
    .await
}
