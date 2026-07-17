use futures_util::StreamExt;
use reqwest::StatusCode;

#[path = "openai_compatible/dialect.rs"]
mod dialect;

pub(crate) use dialect::OpenAiCompatibleDialect;

use crate::{
    auth::kimi_token::KimiAuthManager,
    model::{ModelError, ModelEvent, ModelIdentity, ModelRequest, ModelResponse, ModelUsage},
    protocol::openai_chat::{
        convert_openai_response, convert_streamed_response, handle_openai_stream_line,
        invalid_stream_utf8, to_openai_message_for_target, to_openai_tool, ChatRequest,
        ChatResponse, ChatStreamOptions, OpenAiReasoning,
    },
    provider_backend::{line_decoder::LineDecoder, stream_timeout::StreamIdleDeadline},
};

pub(crate) enum CompatibleAuth {
    ApiKey(String),
    KimiOAuth(KimiAuthManager),
}

pub(crate) struct OpenAiCompatibleProvider {
    client: reqwest::Client,
    provider: &'static str,
    model: String,
    dialect: OpenAiCompatibleDialect,
    auth: CompatibleAuth,
    api_base: String,
}

impl OpenAiCompatibleProvider {
    pub(crate) fn new(
        client: reqwest::Client,
        provider: &'static str,
        model: String,
        dialect: OpenAiCompatibleDialect,
        auth: CompatibleAuth,
        api_base: String,
    ) -> Self {
        Self {
            client,
            provider,
            model,
            dialect,
            auth,
            api_base,
        }
    }

    pub(crate) fn model_identity(&self) -> ModelIdentity {
        ModelIdentity::new(self.provider, "openai-chat-completions", &self.model)
    }

    pub(crate) async fn complete_turn(
        &self,
        request: ModelRequest<'_>,
    ) -> Result<ModelResponse, ModelError> {
        let body = self.request_body(request, false)?;
        let response = self.send(&body, None).await?;
        let response = crate::provider_backend::http_error::error_for_status(response).await?;
        convert_openai_response(response.json::<ChatResponse>().await?)
    }

    pub(crate) async fn stream_turn(
        &self,
        request: ModelRequest<'_>,
        on_event: &mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send),
        on_request_event: &mut (dyn FnMut(rho_sdk::provider::ProviderRequestEvent) -> Result<(), ModelError>
                  + Send),
    ) -> Result<ModelResponse, ModelError> {
        let cancellation = request.cancellation.clone();
        tokio::select! {
            result = self.stream_inner(request, on_event, on_request_event) => result,
            () = cancellation.cancelled() => Err(ModelError::Interrupted),
        }
    }

    async fn stream_inner(
        &self,
        request: ModelRequest<'_>,
        on_event: &mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send),
        on_request_event: &mut (dyn FnMut(rho_sdk::provider::ProviderRequestEvent) -> Result<(), ModelError>
                  + Send),
    ) -> Result<ModelResponse, ModelError> {
        let body = self.request_body(request, true)?;
        let response = self.send(&body, Some(on_request_event)).await?;
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

    fn request_body(
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
            .map(|tool| self.dialect.normalize_tool(tool))
            .collect::<Vec<_>>();
        let has_tools = !tools.is_empty();
        Ok(ChatRequest {
            model: self.model.clone(),
            messages,
            tools: has_tools.then_some(tools),
            tool_choice: has_tools.then_some("auto"),
            stream,
            stream_options: stream.then_some(ChatStreamOptions {
                include_usage: true,
            }),
            reasoning: self
                .dialect
                .reasoning(request.reasoning_level)
                .map(|effort| OpenAiReasoning { effort }),
            reasoning_effort: None,
            thinking: self
                .dialect
                .thinking(self.provider, &self.model, request.reasoning_level),
        })
    }

    async fn send(
        &self,
        body: &ChatRequest,
        on_request_event: Option<
            &mut (dyn FnMut(rho_sdk::provider::ProviderRequestEvent) -> Result<(), ModelError>
                      + Send),
        >,
    ) -> Result<reqwest::Response, ModelError> {
        let token = match &self.auth {
            CompatibleAuth::ApiKey(key) => key.clone(),
            CompatibleAuth::KimiOAuth(auth) => auth.access_token().await?,
        };
        let response = self.send_with_token(body, &token).await?;
        if response.status() != StatusCode::UNAUTHORIZED {
            return Ok(response);
        }
        let CompatibleAuth::KimiOAuth(auth) = &self.auth else {
            return Ok(response);
        };
        let Some(refreshed) = auth.force_refresh(&token).await? else {
            return Ok(response);
        };
        if let Some(on_request_event) = on_request_event {
            on_request_event(
                rho_sdk::provider::ProviderRequestEvent::RequestAttemptFailed {
                    kind: rho_sdk::ProviderErrorKind::Authentication,
                    usage: ModelUsage::default(),
                },
            )?;
        }
        self.send_with_token(body, &refreshed).await
    }

    async fn send_with_token(
        &self,
        body: &ChatRequest,
        token: &str,
    ) -> Result<reqwest::Response, ModelError> {
        Ok(self
            .client
            .post(format!(
                "{}/chat/completions",
                self.api_base.trim_end_matches('/')
            ))
            .bearer_auth(token)
            .json(body)
            .send()
            .await?)
    }
}

crate::impl_sdk_model_provider!(OpenAiCompatibleProvider);

#[cfg(test)]
#[path = "openai_compatible_tests.rs"]
mod tests;
