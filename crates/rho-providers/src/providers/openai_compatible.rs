use futures_util::StreamExt;
use reqwest::StatusCode;

#[path = "openai_compatible/reasoning.rs"]
mod reasoning;

pub(crate) use crate::openai_compatible_dialect::OpenAiCompatibleDialect;

use crate::{
    auth::kimi_token::KimiAuthManager,
    auth::ollama_device::OllamaDeviceKey,
    model::{ModelError, ModelEvent, ModelIdentity, ModelRequest, ModelResponse, ModelUsage},
    protocol::openai_chat::{
        convert_openai_response, invalid_stream_utf8, response_without_stream_context,
        to_openai_message_for_target, to_openai_tool, ChatRequest, ChatResponse,
        ChatStreamAccumulator, ChatStreamOptions,
    },
    provider_backend::{line_decoder::LineDecoder, stream_timeout::StreamIdleDeadline},
};

pub enum CompatibleAuth {
    None,
    ApiKey(String),
    KimiOAuth(KimiAuthManager),
    OllamaDevice(OllamaDeviceKey),
}

pub(crate) struct OpenAiCompatibleProvider {
    client: reqwest::Client,
    provider: &'static str,
    model: String,
    dialect: OpenAiCompatibleDialect,
    auth: CompatibleAuth,
    api_base: String,
    reasoning: reasoning::DialectReasoning,
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
        let reasoning = reasoning::DialectReasoning::new(dialect, provider, &model);
        Self {
            client,
            provider,
            model,
            dialect,
            auth,
            api_base,
            reasoning,
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
        // `ModelResponse` cannot carry ProviderContext. Reasoning replay for
        // Qwen-style tool loops requires `stream_turn` (Rho orchestration always
        // streams). See `response_without_stream_context`.
        Ok(response_without_stream_context(convert_openai_response(
            response.json::<ChatResponse>().await?,
            self.dialect.chat_tool_call_policy(),
        )?))
    }

    pub(crate) async fn stream_turn(
        &self,
        request: ModelRequest<'_>,
        on_event: &mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send),
        on_request_event: &mut (dyn FnMut(rho_sdk::provider::ProviderRequestEvent) -> Result<(), ModelError>
                  + Send),
    ) -> Result<ModelResponse, ModelError> {
        self.stream_inner(request, on_event, on_request_event).await
    }

    async fn stream_inner(
        &self,
        request: ModelRequest<'_>,
        on_event: &mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send),
        on_request_event: &mut (dyn FnMut(rho_sdk::provider::ProviderRequestEvent) -> Result<(), ModelError>
                  + Send),
    ) -> Result<ModelResponse, ModelError> {
        let reasoning_level = request.reasoning_level;
        let body = self.request_body(request, true)?;
        let response = self.send(&body, Some(on_request_event)).await?;
        let response = crate::provider_backend::http_error::error_for_status(response).await?;
        let hidden_reasoning_risk = self
            .reasoning
            .hidden_reasoning_risk(&self.model, reasoning_level);
        let mut chat_stream =
            ChatStreamAccumulator::new(self.dialect.chat_tool_call_policy(), hidden_reasoning_risk);
        let mut decoder = LineDecoder::default();
        let mut stream = response.bytes_stream();
        let mut idle_deadline = StreamIdleDeadline::new();
        loop {
            let Some(chunk) = idle_deadline.wait_for(stream.next()).await? else {
                break;
            };
            decoder.push(&chunk?);
            while let Some(line) = decoder.next_line().map_err(invalid_stream_utf8)? {
                if chat_stream.handle_line(line, on_event)? {
                    idle_deadline.record_activity();
                }
            }
        }
        if let Some(line) = decoder.finish().map_err(invalid_stream_utf8)? {
            chat_stream.handle_line(line, on_event)?;
        }
        chat_stream.finish(on_event)
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
        let reasoning_fields = self.reasoning.fields(&self.model, request.reasoning_level);
        let wire_model = crate::provider::provider_descriptor(self.provider)
            .map(|descriptor| descriptor.wire_model_id(&self.model))
            .unwrap_or_else(|| self.model.clone());
        Ok(ChatRequest {
            model: wire_model,
            messages,
            tools: has_tools.then_some(tools),
            tool_choice: has_tools.then_some("auto"),
            parallel_tool_calls: has_tools.then_some(true),
            stream,
            stream_options: stream.then_some(ChatStreamOptions {
                include_usage: true,
            }),
            prompt_cache_key: request.prompt_cache_key.map(str::to_owned),
            reasoning: reasoning_fields.reasoning,
            reasoning_effort: reasoning_fields.reasoning_effort,
            thinking: reasoning_fields.thinking,
            chat_template_kwargs: reasoning_fields.chat_template_kwargs,
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
        match &self.auth {
            CompatibleAuth::None => self.send_request(body, RequestAuth::None).await,
            CompatibleAuth::ApiKey(key) => self.send_request(body, RequestAuth::Bearer(key)).await,
            CompatibleAuth::KimiOAuth(auth) => {
                let token = auth.access_token().await?;
                let response = self.send_request(body, RequestAuth::Bearer(&token)).await?;
                if response.status() != StatusCode::UNAUTHORIZED {
                    return Ok(response);
                }
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
                self.send_request(body, RequestAuth::Bearer(&refreshed))
                    .await
            }
            CompatibleAuth::OllamaDevice(key) => {
                self.send_request(body, RequestAuth::OllamaDevice(key))
                    .await
            }
        }
    }

    async fn send_request(
        &self,
        body: &ChatRequest,
        auth: RequestAuth<'_>,
    ) -> Result<reqwest::Response, ModelError> {
        let endpoint = format!("{}/chat/completions", self.api_base.trim_end_matches('/'));
        let (url, authorization) = match &auth {
            RequestAuth::None | RequestAuth::Bearer(_) => (endpoint, None),
            RequestAuth::OllamaDevice(key) => {
                let url = url::Url::parse(&endpoint).map_err(|error| {
                    ModelError::InvalidResponse(format!("invalid chat completions URL: {error}"))
                })?;
                let (url, authorization) = key
                    .authorize_request("POST", url)
                    .map_err(|error| ModelError::InvalidResponse(error.to_string()))?;
                (url.to_string(), Some(authorization))
            }
        };
        let mut request = self.client.post(url).json(body);
        match auth {
            RequestAuth::None => {}
            RequestAuth::Bearer(token) => {
                request = request.bearer_auth(token);
            }
            RequestAuth::OllamaDevice(_) => {
                request = request.header(
                    reqwest::header::AUTHORIZATION,
                    authorization.expect("Ollama device auth resolves an Authorization header"),
                );
            }
        }
        Ok(request.send().await?)
    }
}

enum RequestAuth<'a> {
    None,
    Bearer(&'a str),
    OllamaDevice(&'a OllamaDeviceKey),
}

crate::impl_sdk_model_provider!(OpenAiCompatibleProvider);

#[cfg(test)]
#[path = "openai_compatible_tests.rs"]
mod tests;
