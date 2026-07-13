use std::sync::Arc;

use futures_util::StreamExt;
use reqwest::StatusCode;

use crate::{
    auth::xai_token::XaiAuthManager,
    credentials::CredentialStore,
    model::{
        openai::{
            convert::{convert_openai_response, to_openai_message, to_openai_tool},
            stream::{convert_streamed_response, handle_openai_stream_line, invalid_stream_utf8},
            types::{ChatRequest, ChatResponse, ChatStreamOptions},
        },
        ModelError, ModelEvent, ModelProvider, ModelRequest, ModelResponse,
    },
    provider_backend::{
        line_decoder::LineDecoder,
        stream_timeout::{provider_client, StreamIdleDeadline},
    },
};

const API_BASE: &str = "https://api.x.ai/v1";

pub struct XaiProvider {
    client: reqwest::Client,
    model: String,
    auth: XaiAuthManager,
}

impl XaiProvider {
    pub(crate) fn new(model: String, store: Arc<dyn CredentialStore>) -> Result<Self, ModelError> {
        Ok(Self {
            client: provider_client(),
            model,
            auth: XaiAuthManager::new(store)?,
        })
    }

    async fn send_request(
        &self,
        request: ModelRequest<'_>,
        stream: bool,
    ) -> Result<reqwest::Response, ModelError> {
        let messages = request
            .messages
            .iter()
            .cloned()
            .map(to_openai_message)
            .collect::<Result<Vec<_>, _>>()?;
        let tools = request
            .tools
            .iter()
            .cloned()
            .map(to_openai_tool)
            .collect::<Vec<_>>();
        let has_tools = !tools.is_empty();
        let body = ChatRequest {
            model: self.model.clone(),
            messages,
            tools: has_tools.then_some(tools),
            tool_choice: has_tools.then_some("auto"),
            stream,
            stream_options: stream.then_some(ChatStreamOptions {
                include_usage: true,
            }),
        };
        let auth = self.auth.auth_material().await?;
        let response = self
            .send_request_with_token(&body, &auth.access_token)
            .await?;
        if response.status() != StatusCode::UNAUTHORIZED {
            return Ok(response);
        }
        let Some(refreshed) = self.auth.force_refresh(&auth.access_token).await? else {
            return Ok(response);
        };
        self.send_request_with_token(&body, &refreshed.access_token)
            .await
    }

    async fn send_request_with_token(
        &self,
        body: &ChatRequest,
        access_token: &str,
    ) -> Result<reqwest::Response, ModelError> {
        Ok(self
            .client
            .post(format!("{API_BASE}/chat/completions"))
            .bearer_auth(access_token)
            .header("User-Agent", concat!("rho/", env!("CARGO_PKG_VERSION")))
            .json(body)
            .send()
            .await?)
    }
}

#[async_trait::async_trait(?Send)]
impl ModelProvider for XaiProvider {
    async fn send_turn(&self, request: ModelRequest<'_>) -> Result<ModelResponse, ModelError> {
        let response = self.send_request(request, false).await?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ModelError::HttpStatus { status, body });
        }
        convert_openai_response(response.json::<ChatResponse>().await?)
    }

    async fn send_turn_stream(
        &self,
        request: ModelRequest<'_>,
        on_event: &mut dyn FnMut(ModelEvent) -> Result<(), ModelError>,
    ) -> Result<ModelResponse, ModelError> {
        let response = self.send_request(request, true).await?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ModelError::HttpStatus { status, body });
        }
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
}
