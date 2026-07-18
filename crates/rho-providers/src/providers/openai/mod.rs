use std::sync::{Arc, Mutex};

use futures_util::StreamExt;
use serde_json::Value;

pub mod auth;
pub mod cache;
mod codex_continuation;
mod codex_request;
mod codex_ws;
mod reasoning;

pub use cache::prompt_cache_key_from_session_id;

use crate::protocol::openai_chat::{
    convert_openai_response, convert_streamed_response, handle_openai_stream_line,
    invalid_stream_utf8, to_openai_message_for_target, to_openai_tool, ChatRequest, ChatResponse,
    ChatStreamOptions,
};
use crate::protocol::openai_responses::collect_codex_sse_response;
use auth::{refresh_codex_token, Auth, CodexAuthSource};
use codex_request::{build_codex_responses_body, CodexRequestMode};
use codex_ws::{CodexWsTransport, CodexWsTurn};
use reasoning::openai_reasoning_config;

use crate::{
    credentials::{load_codex_tokens, CodexTokens, CredentialStore},
    model::{ModelError, ModelEvent, ModelIdentity, ModelRequest, ModelResponse, ModelUsage},
    provider_backend::{line_decoder::LineDecoder, stream_timeout::StreamIdleDeadline},
};

#[cfg(test)]
use crate::provider_backend::stream_timeout::provider_client;

pub struct OpenAiProvider {
    client: reqwest::Client,
    auth: Auth,
    api_base: String,
    model: String,
    provider: &'static str,
    codex_ws: CodexWsTransport,
    credential_store: Arc<dyn CredentialStore>,
    refreshed_codex_tokens: Mutex<Option<CodexTokens>>,
}

impl OpenAiProvider {
    #[cfg(test)]
    pub(crate) fn new_with_auth(
        model: String,
        auth: Auth,
        credential_store: Arc<dyn CredentialStore>,
    ) -> Self {
        Self::new_with_transport(model, auth, credential_store, provider_client(), None)
    }

    pub(crate) fn new_with_transport(
        model: String,
        auth: Auth,
        credential_store: Arc<dyn CredentialStore>,
        client: reqwest::Client,
        api_base_override: Option<String>,
    ) -> Self {
        let (default_api_base, provider): (String, &'static str) = match &auth {
            Auth::Codex { .. } => (
                "https://chatgpt.com/backend-api/codex".into(),
                "openai-codex",
            ),
            Auth::ApiKey(_) => ("https://api.openai.com/v1".into(), "openai"),
        };
        let api_base = api_base_override.unwrap_or(default_api_base);
        let codex_ws = CodexWsTransport::new(&api_base);
        Self {
            client,
            auth,
            api_base,
            model,
            provider,
            codex_ws,
            credential_store,
            refreshed_codex_tokens: Mutex::new(None),
        }
    }
}

impl OpenAiProvider {
    pub(crate) fn model_identity(&self) -> ModelIdentity {
        let api = match self.auth {
            Auth::ApiKey(_) => "openai-chat-completions",
            Auth::Codex { .. } => "openai-responses",
        };
        ModelIdentity::new(self.provider, api, &self.model)
    }

    /// Completes one turn using a `Send` future suitable for the public SDK trait.
    pub(crate) async fn complete_turn(
        &self,
        request: ModelRequest<'_>,
    ) -> Result<ModelResponse, ModelError> {
        match &self.auth {
            Auth::ApiKey(key) => self.send_chat_completions(request, key).await,
            Auth::Codex { tokens, source } => {
                let tokens = self.codex_turn_tokens(tokens, *source);
                self.send_codex_responses_complete(request, tokens, *source)
                    .await
            }
        }
    }

    /// Streams one turn through a `Send` callback for the public SDK adapter.
    pub(crate) async fn stream_turn(
        &self,
        request: ModelRequest<'_>,
        on_event: &mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send),
        on_request_event: &mut (dyn FnMut(rho_sdk::provider::ProviderRequestEvent) -> Result<(), ModelError>
                  + Send),
    ) -> Result<ModelResponse, ModelError> {
        let cancellation = request.cancellation.clone();
        tokio::select! {
            result = async {
                match &self.auth {
                    Auth::ApiKey(key) => {
                        self.send_chat_completions_stream(request, key, on_event).await
                    }
                    Auth::Codex { tokens, source } => {
                        let tokens = self.codex_turn_tokens(tokens, *source);
                        self.send_codex_responses_stream(
                            request,
                            tokens,
                            *source,
                            on_event,
                            on_request_event,
                        )
                        .await
                    }
                }
            } => result,
            () = cancellation.cancelled() => {
                if matches!(&self.auth, Auth::Codex { .. }) {
                    self.codex_ws.reset().await;
                }
                Err(ModelError::Interrupted)
            }
        }
    }

    fn codex_turn_tokens(&self, initial: &CodexTokens, source: CodexAuthSource) -> CodexTokens {
        if source == CodexAuthSource::Store {
            if let Ok(Some(tokens)) = load_codex_tokens(self.credential_store.as_ref()) {
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

crate::impl_sdk_model_provider!(OpenAiProvider);

impl OpenAiProvider {
    fn chat_completions_request(
        &self,
        request: ModelRequest<'_>,
        stream: bool,
    ) -> Result<ChatRequest, ModelError> {
        let target = self.model_identity();
        let messages = request
            .messages
            .iter()
            .cloned()
            .map(|message| to_openai_message_for_target(message, Some(&target)))
            .collect::<Result<Vec<_>, _>>()?;
        let tools = request
            .tools
            .iter()
            .cloned()
            .map(to_openai_tool)
            .collect::<Vec<_>>();
        let has_tools = !tools.is_empty();
        let reasoning =
            openai_reasoning_config(self.provider, &self.model, request.reasoning_level)?;
        Ok(ChatRequest {
            model: self.model.clone(),
            messages,
            tools: has_tools.then_some(tools),
            tool_choice: has_tools.then_some("auto"),
            stream,
            stream_options: stream.then_some(ChatStreamOptions {
                include_usage: true,
            }),
            reasoning: None,
            reasoning_effort: reasoning.effort,
            thinking: None,
        })
    }

    async fn send_chat_completions(
        &self,
        request: ModelRequest<'_>,
        key: &str,
    ) -> Result<ModelResponse, ModelError> {
        let url = format!("{}/chat/completions", self.api_base.trim_end_matches('/'));
        let body = self.chat_completions_request(request, /*stream*/ false)?;
        let response = self
            .client
            .post(url)
            .bearer_auth(key)
            .json(&body)
            .send()
            .await?;
        let response = crate::provider_backend::http_error::error_for_status(response).await?;
        let response: ChatResponse = response.json().await?;
        convert_openai_response(response)
    }

    async fn send_chat_completions_stream(
        &self,
        request: ModelRequest<'_>,
        key: &str,
        on_event: &mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send),
    ) -> Result<ModelResponse, ModelError> {
        let url = format!("{}/chat/completions", self.api_base.trim_end_matches('/'));
        let body = self.chat_completions_request(request, /*stream*/ true)?;
        let response = self
            .client
            .post(url)
            .bearer_auth(key)
            .json(&body)
            .send()
            .await?;
        let response = crate::provider_backend::http_error::error_for_status(response).await?;

        let mut text = String::new();
        let mut tool_calls = Vec::new();
        let mut decoder = LineDecoder::default();
        let mut stream = response.bytes_stream();
        let mut idle_deadline = StreamIdleDeadline::new();
        loop {
            let Some(chunk) = idle_deadline.wait_for(stream.next()).await? else {
                break;
            };
            decoder.push(&chunk?);
            while let Some(line) = decoder.next_line().map_err(invalid_stream_utf8)? {
                if handle_openai_stream_line(line, &mut text, &mut tool_calls, on_event)? {
                    idle_deadline.record_activity();
                }
            }
        }
        if let Some(line) = decoder.finish().map_err(invalid_stream_utf8)? {
            handle_openai_stream_line(line, &mut text, &mut tool_calls, on_event)?;
        }

        convert_streamed_response(text, tool_calls)
    }

    async fn send_codex_responses_complete(
        &self,
        request: ModelRequest<'_>,
        tokens: CodexTokens,
        source: CodexAuthSource,
    ) -> Result<ModelResponse, ModelError> {
        let body = build_codex_responses_body(&self.model, request)?;
        let mode = CodexRequestMode::for_model(&self.model);
        match self
            .codex_ws
            .send_responses_turn_silent(body.clone(), &tokens, mode)
            .await?
        {
            CodexWsTurn::Completed(response) => return Ok(response),
            CodexWsTurn::FullSseFallback { .. } => {}
        }

        let url = format!("{}/responses", self.api_base.trim_end_matches('/'));
        let make_request = |token: &str| {
            let request = self
                .client
                .post(&url)
                .bearer_auth(token)
                .header("User-Agent", "codex-cli")
                .header("originator", "codex_cli_rs")
                .json(&body);
            if mode.uses_responses_lite() {
                request.header("x-openai-internal-codex-responses-lite", "true")
            } else {
                request
            }
        };
        let mut req = make_request(&tokens.access_token);
        if let Some(account_id) = tokens.account_id.as_deref() {
            req = req.header("ChatGPT-Account-ID", account_id);
        }
        let response = match req.send().await {
            Ok(response) => response,
            Err(err) => {
                self.codex_ws.reset().await;
                return Err(err.into());
            }
        };
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            self.codex_ws.reset().await;
            if let Some(refresh_token) = tokens.refresh_token.as_deref() {
                let refreshed = refresh_codex_token(
                    &self.client,
                    self.credential_store.as_ref(),
                    refresh_token,
                    source,
                    &tokens,
                )
                .await?;
                self.remember_refreshed_codex_tokens(refreshed.clone());
                let mut req = make_request(&refreshed.access_token);
                if let Some(account_id) = refreshed.account_id.as_deref() {
                    req = req.header("ChatGPT-Account-ID", account_id);
                }
                let response = match req.send().await {
                    Ok(response) => response,
                    Err(err) => {
                        self.codex_ws.reset().await;
                        return Err(err.into());
                    }
                };
                if !response.status().is_success() {
                    self.codex_ws.reset().await;
                    return Err(crate::provider_backend::http_error::from_response(response).await);
                }
                return self
                    .collect_codex_sse_response_silent(response, &body)
                    .await;
            }
        }
        if !response.status().is_success() {
            self.codex_ws.reset().await;
            return Err(crate::provider_backend::http_error::from_response(response).await);
        }
        self.collect_codex_sse_response_silent(response, &body)
            .await
    }

    async fn send_codex_responses_stream(
        &self,
        request: ModelRequest<'_>,
        tokens: CodexTokens,
        source: CodexAuthSource,
        on_event: &mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send),
        on_request_event: &mut (dyn FnMut(rho_sdk::provider::ProviderRequestEvent) -> Result<(), ModelError>
                  + Send),
    ) -> Result<ModelResponse, ModelError> {
        self.send_codex_responses_inner(request, tokens, source, Some(on_event), on_request_event)
            .await
    }

    async fn send_codex_responses_inner(
        &self,
        request: ModelRequest<'_>,
        tokens: CodexTokens,
        source: CodexAuthSource,
        mut on_event: Option<&mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send)>,
        on_request_event: &mut (dyn FnMut(rho_sdk::provider::ProviderRequestEvent) -> Result<(), ModelError>
                  + Send),
    ) -> Result<ModelResponse, ModelError> {
        let body = build_codex_responses_body(&self.model, request.clone())?;
        let mode = CodexRequestMode::for_model(&self.model);
        match self
            .codex_ws
            .send_responses_turn(body, &tokens, mode, &mut on_event)
            .await?
        {
            CodexWsTurn::Completed(response) => return Ok(response),
            CodexWsTurn::FullSseFallback { request_submitted } => {
                if request_submitted {
                    // The submitted WebSocket request may have reached the model
                    // before the transport failed, so account for it separately.
                    on_request_event(
                        rho_sdk::provider::ProviderRequestEvent::RequestAttemptFailed {
                            kind: rho_sdk::ProviderErrorKind::Unavailable,
                            usage: ModelUsage::default(),
                        },
                    )?;
                }
            }
        }

        // Rebuilt only on this rare fallback path so the common WebSocket
        // turn does not clone the full-history request body.
        let body = build_codex_responses_body(&self.model, request)?;

        let url = format!("{}/responses", self.api_base.trim_end_matches('/'));
        let make_request = |token: &str| {
            let request = self
                .client
                .post(&url)
                .bearer_auth(token)
                .header("User-Agent", "codex-cli")
                .header("originator", "codex_cli_rs")
                .json(&body);
            if mode.uses_responses_lite() {
                request.header("x-openai-internal-codex-responses-lite", "true")
            } else {
                request
            }
        };
        let mut req = make_request(&tokens.access_token);
        if let Some(account_id) = tokens.account_id.as_deref() {
            req = req.header("ChatGPT-Account-ID", account_id);
        }
        let response = match req.send().await {
            Ok(response) => response,
            Err(err) => {
                self.codex_ws.reset().await;
                return Err(err.into());
            }
        };
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            self.codex_ws.reset().await;
            if let Some(refresh_token) = tokens.refresh_token.as_deref() {
                let refreshed = refresh_codex_token(
                    &self.client,
                    self.credential_store.as_ref(),
                    refresh_token,
                    source,
                    &tokens,
                )
                .await?;
                self.remember_refreshed_codex_tokens(refreshed.clone());
                on_request_event(
                    rho_sdk::provider::ProviderRequestEvent::RequestAttemptFailed {
                        kind: rho_sdk::ProviderErrorKind::Authentication,
                        usage: ModelUsage::default(),
                    },
                )?;
                let mut req = make_request(&refreshed.access_token);
                if let Some(account_id) = refreshed.account_id.as_deref() {
                    req = req.header("ChatGPT-Account-ID", account_id);
                }
                let response = match req.send().await {
                    Ok(response) => response,
                    Err(err) => {
                        self.codex_ws.reset().await;
                        return Err(err.into());
                    }
                };
                if !response.status().is_success() {
                    self.codex_ws.reset().await;
                    return Err(crate::provider_backend::http_error::from_response(response).await);
                }
                return self
                    .collect_codex_sse_response(response, &mut on_event, &body)
                    .await;
            }
        }
        if !response.status().is_success() {
            self.codex_ws.reset().await;
            return Err(crate::provider_backend::http_error::from_response(response).await);
        }
        self.collect_codex_sse_response(response, &mut on_event, &body)
            .await
    }

    async fn collect_codex_sse_response(
        &self,
        response: reqwest::Response,
        on_event: &mut Option<&mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send)>,
        body: &Value,
    ) -> Result<ModelResponse, ModelError> {
        match collect_codex_sse_response(response, on_event).await {
            Ok(output) => {
                self.codex_ws
                    .record_full_request_success(body, &output)
                    .await?;
                Ok(output.response)
            }
            Err(err) => {
                self.codex_ws.reset().await;
                Err(err)
            }
        }
    }

    async fn collect_codex_sse_response_silent(
        &self,
        response: reqwest::Response,
        body: &Value,
    ) -> Result<ModelResponse, ModelError> {
        match crate::providers::send_stream::collect_codex_sse_silent(response).await {
            Ok(output) => {
                self.codex_ws
                    .record_full_request_success(body, &output)
                    .await?;
                Ok(output.response)
            }
            Err(err) => {
                self.codex_ws.reset().await;
                Err(err)
            }
        }
    }
}

#[cfg(test)]
#[path = "stream_tests.rs"]
mod stream_tests;

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
