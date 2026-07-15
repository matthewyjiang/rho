use std::sync::Arc;

use futures_util::StreamExt;
use serde_json::Value;

pub(crate) mod auth;
pub mod cache;
mod codex_continuation;
mod codex_request;
mod codex_ws;

pub(crate) use cache::prompt_cache_key_from_session_id;

use crate::protocol::openai_chat::{
    convert_openai_response, convert_streamed_response, handle_openai_stream_line,
    invalid_stream_utf8, to_openai_message_for_target, to_openai_tool, ChatRequest, ChatResponse,
    ChatStreamOptions,
};
use crate::protocol::openai_responses::collect_codex_sse_response;
use auth::{load_codex_tokens_for_request, refresh_codex_token, Auth, CodexAuthSource};
use codex_request::{build_codex_responses_body, CodexRequestMode};
use codex_ws::{CodexWsTransport, CodexWsTurn};

use crate::{
    credentials::{CodexTokens, CredentialStore},
    model::{ModelError, ModelEvent, ModelIdentity, ModelProvider, ModelRequest, ModelResponse},
    provider_backend::{
        line_decoder::LineDecoder,
        stream_timeout::{provider_client, StreamIdleDeadline},
    },
};

pub struct OpenAiProvider {
    client: reqwest::Client,
    auth: Auth,
    api_base: String,
    model: String,
    provider: &'static str,
    reasoning_effort: Option<String>,
    reasoning_summary: Option<String>,
    codex_ws: CodexWsTransport,
    credential_store: Arc<dyn CredentialStore>,
}

impl OpenAiProvider {
    pub(crate) fn new_with_auth(
        model: String,
        auth: Auth,
        credential_store: Arc<dyn CredentialStore>,
        reasoning_effort: Option<String>,
        reasoning_summary: Option<String>,
    ) -> Self {
        let (api_base, provider): (String, &'static str) = match &auth {
            Auth::Codex { .. } => (
                "https://chatgpt.com/backend-api/codex".into(),
                "openai-codex",
            ),
            Auth::ApiKey(_) => ("https://api.openai.com/v1".into(), "openai"),
        };
        let codex_ws = CodexWsTransport::new(&api_base);
        Self {
            client: provider_client(),
            auth,
            api_base,
            model,
            provider,
            reasoning_effort,
            reasoning_summary,
            codex_ws,
            credential_store,
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
                let request_tokens =
                    load_codex_tokens_for_request(self.credential_store.as_ref(), tokens, *source)?;
                self.send_codex_responses_complete(request, request_tokens, *source)
                    .await
            }
        }
    }

    /// Streams one turn through a `Send` callback for the public SDK adapter.
    pub(crate) async fn stream_turn(
        &self,
        request: ModelRequest<'_>,
        on_event: &mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send),
    ) -> Result<ModelResponse, ModelError> {
        let cancellation = request.cancellation.clone();
        tokio::select! {
            result = async {
                match &self.auth {
                    Auth::ApiKey(key) => {
                        self.send_chat_completions_stream(request, key, on_event).await
                    }
                    Auth::Codex { tokens, source } => {
                        let request_tokens = load_codex_tokens_for_request(
                            self.credential_store.as_ref(), tokens, *source,
                        )?;
                        self.send_codex_responses_stream(
                            request, request_tokens, *source, on_event,
                        ).await
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
}

#[async_trait::async_trait(?Send)]
impl ModelProvider for OpenAiProvider {
    fn identity(&self) -> Option<ModelIdentity> {
        Some(self.model_identity())
    }

    fn set_reasoning(&mut self, reasoning: crate::reasoning::ReasoningLevel) -> bool {
        let supported_reasoning =
            crate::model::models_dev::cached_reasoning_levels(self.provider, &self.model);
        let reasoning = reasoning.normalize(supported_reasoning.as_deref());
        self.reasoning_effort = crate::model::models_dev::cached_reasoning_effort(
            self.provider,
            &self.model,
            reasoning,
        );
        self.reasoning_summary = reasoning.summary().map(str::to_string);
        true
    }

    async fn send_turn(&self, request: ModelRequest<'_>) -> Result<ModelResponse, ModelError> {
        self.complete_turn(request).await
    }

    async fn send_turn_stream(
        &self,
        request: ModelRequest<'_>,
        on_event: &mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send),
    ) -> Result<ModelResponse, ModelError> {
        let cancellation = request.cancellation.clone();
        tokio::select! {
            result = async {
                match &self.auth {
                    Auth::ApiKey(key) => {
                        self.send_chat_completions_stream(request, key, on_event).await
                    }
                    Auth::Codex { tokens, source } => {
                        let request_tokens = load_codex_tokens_for_request(
                            self.credential_store.as_ref(), tokens, *source,
                        )?;
                        self.send_codex_responses_stream(
                            request, request_tokens, *source, on_event,
                        ).await
                    }
                }
            } => result,
            () = cancellation.cancelled() => {
                if matches!(&self.auth, Auth::Codex { .. }) {
                    self.codex_ws.reset().await;
                }
                Err(ModelError::Interrupted)
            },
        }
    }
}

impl OpenAiProvider {
    async fn send_chat_completions(
        &self,
        request: ModelRequest<'_>,
        key: &str,
    ) -> Result<ModelResponse, ModelError> {
        let target = self.identity().expect("OpenAI provider has an identity");
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
        let url = format!("{}/chat/completions", self.api_base.trim_end_matches('/'));
        let response: ChatResponse = self
            .client
            .post(url)
            .bearer_auth(key)
            .json(&ChatRequest {
                model: self.model.clone(),
                messages,
                tools: has_tools.then_some(tools),
                tool_choice: has_tools.then_some("auto"),
                stream: false,
                stream_options: None,
            })
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        convert_openai_response(response)
    }

    async fn send_chat_completions_stream(
        &self,
        request: ModelRequest<'_>,
        key: &str,
        on_event: &mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send),
    ) -> Result<ModelResponse, ModelError> {
        let target = self.identity().expect("OpenAI provider has an identity");
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
        let url = format!("{}/chat/completions", self.api_base.trim_end_matches('/'));
        let response = self
            .client
            .post(url)
            .bearer_auth(key)
            .json(&ChatRequest {
                model: self.model.clone(),
                messages,
                tools: has_tools.then_some(tools),
                tool_choice: has_tools.then_some("auto"),
                stream: true,
                stream_options: Some(ChatStreamOptions {
                    include_usage: true,
                }),
            })
            .send()
            .await?
            .error_for_status()?;

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
        let body = build_codex_responses_body(
            &self.model,
            request,
            self.reasoning_effort.as_deref(),
            self.reasoning_summary.as_deref(),
        )?;
        let mode = CodexRequestMode::for_model(&self.model);
        match self
            .codex_ws
            .send_responses_turn_silent(body.clone(), &tokens, mode)
            .await?
        {
            CodexWsTurn::Completed(response) => return Ok(response),
            CodexWsTurn::FullSseFallback => {}
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
                    let status = response.status();
                    let body = response.text().await.unwrap_or_default();
                    return Err(ModelError::HttpStatus { status, body });
                }
                return self
                    .collect_codex_sse_response_silent(response, &body)
                    .await;
            }
        }
        if !response.status().is_success() {
            self.codex_ws.reset().await;
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ModelError::HttpStatus { status, body });
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
    ) -> Result<ModelResponse, ModelError> {
        self.send_codex_responses_inner(request, tokens, source, Some(on_event))
            .await
    }

    async fn send_codex_responses_inner(
        &self,
        request: ModelRequest<'_>,
        tokens: CodexTokens,
        source: CodexAuthSource,
        mut on_event: Option<&mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send)>,
    ) -> Result<ModelResponse, ModelError> {
        let body = build_codex_responses_body(
            &self.model,
            request,
            self.reasoning_effort.as_deref(),
            self.reasoning_summary.as_deref(),
        )?;
        let mode = CodexRequestMode::for_model(&self.model);
        match self
            .codex_ws
            .send_responses_turn(body.clone(), &tokens, mode, &mut on_event)
            .await?
        {
            CodexWsTurn::Completed(response) => return Ok(response),
            CodexWsTurn::FullSseFallback => {
                // The WebSocket transport has already reset stale continuation
                // state and withheld stream events, so replaying the full body
                // over SSE cannot duplicate caller-visible deltas.
            }
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
                    let status = response.status();
                    let body = response.text().await.unwrap_or_default();
                    return Err(ModelError::HttpStatus { status, body });
                }
                return self
                    .collect_codex_sse_response(response, &mut on_event, &body)
                    .await;
            }
        }
        if !response.status().is_success() {
            self.codex_ws.reset().await;
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ModelError::HttpStatus { status, body });
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
