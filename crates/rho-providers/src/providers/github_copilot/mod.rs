use crate::protocol::openai_chat::{
    convert_openai_response, convert_streamed_response, handle_openai_stream_line,
    invalid_stream_utf8, to_openai_message_for_target, to_openai_tool,
};
use futures_util::StreamExt;
use reqwest::StatusCode;

use crate::{
    auth::github_copilot_token::{GitHubCopilotAuthManager, GitHubCopilotAuthMaterial},
    model::{ModelError, ModelEvent, ModelIdentity, ModelRequest, ModelResponse, ModelUsage},
    protocol::openai_chat::{ChatRequest, ChatResponse, ChatStreamOptions},
    provider_backend::{line_decoder::LineDecoder, stream_timeout::StreamIdleDeadline},
};

#[cfg(test)]
use crate::provider_backend::stream_timeout::provider_client;

const DEFAULT_COPILOT_CHAT_COMPLETIONS_URL: &str = "https://api.githubcopilot.com/chat/completions";
const COPILOT_INTEGRATION_ID: &str = "vscode-chat";

pub struct GitHubCopilotProvider {
    client: reqwest::Client,
    auth: GitHubCopilotAuthManager,
    model: String,
    chat_endpoint: Option<String>,
}

impl GitHubCopilotProvider {
    #[cfg(test)]
    pub(crate) fn new(model: String, auth: GitHubCopilotAuthManager) -> Result<Self, ModelError> {
        auth.ensure_auth_available()?;
        Ok(Self {
            client: provider_client(),
            auth,
            model,
            chat_endpoint: None,
        })
    }

    pub(crate) fn new_with_transport(
        model: String,
        auth: GitHubCopilotAuthManager,
        client: reqwest::Client,
        chat_endpoint: Option<String>,
    ) -> Result<Self, ModelError> {
        auth.ensure_auth_available()?;
        Ok(Self {
            client,
            auth,
            model,
            chat_endpoint,
        })
    }

    #[cfg(test)]
    fn new_with_client(
        model: String,
        auth: GitHubCopilotAuthManager,
        client: reqwest::Client,
    ) -> Self {
        Self {
            client,
            auth,
            model,
            chat_endpoint: None,
        }
    }

    fn chat_request(
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
            reasoning_effort: None,
            thinking: None,
        })
    }

    fn apply_headers(
        &self,
        builder: reqwest::RequestBuilder,
        auth: &GitHubCopilotAuthMaterial,
    ) -> reqwest::RequestBuilder {
        builder
            .bearer_auth(&auth.token)
            .header("Accept", "application/json")
            .header("User-Agent", crate::rho_user_agent())
            .header("Editor-Version", crate::rho_user_agent())
            .header("Editor-Plugin-Version", crate::rho_user_agent())
            .header("Copilot-Integration-Id", COPILOT_INTEGRATION_ID)
    }

    async fn send_chat_once(
        &self,
        body: &ChatRequest,
        auth: &GitHubCopilotAuthMaterial,
    ) -> Result<reqwest::Response, ModelError> {
        let endpoint = self.chat_endpoint.as_deref().unwrap_or_else(|| {
            if auth.chat_endpoint.trim().is_empty() {
                DEFAULT_COPILOT_CHAT_COMPLETIONS_URL
            } else {
                auth.chat_endpoint.as_str()
            }
        });
        Ok(self
            .apply_headers(self.client.post(endpoint), auth)
            .json(body)
            .send()
            .await?)
    }

    async fn send_chat_with_retry(
        &self,
        body: ChatRequest,
        auth: GitHubCopilotAuthMaterial,
        on_request_event: Option<
            &mut (dyn FnMut(rho_sdk::provider::ProviderRequestEvent) -> Result<(), ModelError>
                      + Send),
        >,
    ) -> Result<reqwest::Response, ModelError> {
        let response = self.send_chat_once(&body, &auth).await?;
        if response.status() != StatusCode::UNAUTHORIZED {
            return Ok(response);
        }
        if let Some(refreshed) = self.auth.force_refresh(&self.client).await? {
            if let Some(on_request_event) = on_request_event {
                on_request_event(
                    rho_sdk::provider::ProviderRequestEvent::RequestAttemptFailed {
                        kind: rho_sdk::ProviderErrorKind::Authentication,
                        usage: ModelUsage::default(),
                    },
                )?;
            }
            return self.send_chat_once(&body, &refreshed).await;
        }
        Ok(response)
    }
}

impl GitHubCopilotProvider {
    pub(crate) fn model_identity(&self) -> ModelIdentity {
        ModelIdentity::new("github-copilot", "openai-chat-completions", &self.model)
    }

    /// Completes one turn using inherent async methods so the future is `Send`.
    pub(crate) async fn complete_turn(
        &self,
        request: ModelRequest<'_>,
    ) -> Result<ModelResponse, ModelError> {
        let body = self.chat_request(request, false)?;
        let auth = self.auth.auth_material(&self.client).await?;
        let response = self.send_chat_with_retry(body, auth, None).await?;
        let response = error_for_status(response).await?;
        let response: ChatResponse = response.json().await?;
        convert_openai_response(response)
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
            result = self.send_turn_stream_inner(request, on_event, on_request_event) => result,
            () = cancellation.cancelled() => Err(ModelError::Interrupted),
        }
    }
}

crate::impl_sdk_model_provider!(GitHubCopilotProvider);

impl GitHubCopilotProvider {
    async fn send_turn_stream_inner(
        &self,
        request: ModelRequest<'_>,
        on_event: &mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send),
        on_request_event: &mut (dyn FnMut(rho_sdk::provider::ProviderRequestEvent) -> Result<(), ModelError>
                  + Send),
    ) -> Result<ModelResponse, ModelError> {
        let body = self.chat_request(request, true)?;
        let auth = self.auth.auth_material(&self.client).await?;
        let response = self
            .send_chat_with_retry(body, auth, Some(on_request_event))
            .await?;
        let response = error_for_status(response).await?;

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

async fn error_for_status(response: reqwest::Response) -> Result<reqwest::Response, ModelError> {
    if response.status() == StatusCode::UNAUTHORIZED {
        return Err(ModelError::MissingGithubCopilotAuth);
    }
    crate::provider_backend::http_error::error_for_status(response).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    use crate::{
        credentials::{save_github_copilot_tokens, GitHubCopilotTokens, MemoryCredentialStore},
        model::{ContentBlock, Message},
    };
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    #[test]
    fn chat_request_preserves_model_and_streaming_flag() {
        let store = Arc::new(crate::credentials::MemoryCredentialStore::default());
        save_github_copilot_tokens(
            store.as_ref(),
            &GitHubCopilotTokens {
                github_access_token: "github".into(),
                github_refresh_token: None,
                github_expires_at_unix: None,
                copilot_token: Some("copilot".into()),
                copilot_expires_at_unix: Some(4_102_444_800),
                copilot_refresh_after_unix: None,
                copilot_token_endpoint: None,
                copilot_chat_endpoint: None,
                copilot_models_endpoint: None,
            },
        )
        .unwrap();
        let provider = GitHubCopilotProvider::new(
            "gpt-4.1".into(),
            GitHubCopilotAuthManager::new(store).unwrap(),
        )
        .unwrap();

        let body = provider
            .chat_request(
                ModelRequest {
                    messages: &[Message::user_text("hello")],
                    tools: &[],
                    cancellation: Default::default(),
                    reasoning_level: Default::default(),
                    prompt_cache_key: None,
                },
                true,
            )
            .unwrap();

        assert_eq!(body.model, "gpt-4.1");
        assert!(body.stream);
        assert!(body.stream_options.is_some());
    }

    #[test]
    fn provider_construction_requires_available_auth() {
        let result = GitHubCopilotAuthManager::new_with_env_token(
            Arc::new(MemoryCredentialStore::default()),
            None,
        )
        .and_then(|auth| GitHubCopilotProvider::new("gpt-4.1".into(), auth));

        assert!(matches!(result, Err(ModelError::MissingGithubCopilotAuth)));
    }

    #[tokio::test]
    async fn chat_retries_once_after_unauthorized_with_refreshed_endpoint() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let requests_for_server = Arc::clone(&requests);
        let base_url_for_server = base_url.clone();
        tokio::spawn(async move {
            for index in 0..4 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut buffer = [0; 4096];
                let bytes = stream.read(&mut buffer).await.unwrap();
                let request = String::from_utf8_lossy(&buffer[..bytes]).to_string();
                requests_for_server.lock().unwrap().push(request);
                let body = match index {
                    0 => format!(
                        "{{\"token\":\"first\",\"endpoints\":{{\"chat\":\"{base_url_for_server}/chat\"}}}}"
                    ),
                    1 => String::new(),
                    2 => format!(
                        "{{\"token\":\"second\",\"endpoints\":{{\"chat\":\"{base_url_for_server}/chat\"}}}}"
                    ),
                    3 => r#"{"choices":[{"message":{"content":"ok"}}]}"#.to_string(),
                    _ => unreachable!(),
                };
                let status = if index == 1 {
                    "401 Unauthorized"
                } else {
                    "200 OK"
                };
                let reply = format!(
                    "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(), body
                );
                stream.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        let store = Arc::new(MemoryCredentialStore::default());
        save_github_copilot_tokens(
            store.as_ref(),
            &GitHubCopilotTokens {
                github_access_token: "github".into(),
                github_refresh_token: None,
                github_expires_at_unix: None,
                copilot_token: None,
                copilot_expires_at_unix: None,
                copilot_refresh_after_unix: None,
                copilot_token_endpoint: Some(base_url.clone()),
                copilot_chat_endpoint: None,
                copilot_models_endpoint: None,
            },
        )
        .unwrap();
        let provider = GitHubCopilotProvider::new_with_client(
            "gpt-4.1".into(),
            GitHubCopilotAuthManager::new(store).unwrap(),
            reqwest::Client::new(),
        );

        let response = provider
            .complete_turn(ModelRequest {
                messages: &[Message::user_text("hello")],
                tools: &[],
                cancellation: Default::default(),
                reasoning_level: Default::default(),
                prompt_cache_key: None,
            })
            .await
            .unwrap();

        assert!(matches!(
            response,
            ModelResponse::Assistant(blocks) if matches!(blocks.as_slice(), [ContentBlock::Text(text)] if text == "ok")
        ));
        let requests = requests.lock().unwrap();
        assert_eq!(
            requests
                .iter()
                .filter(|request| request.contains("POST /chat"))
                .count(),
            2
        );
        assert!(requests
            .iter()
            .any(|request| request.contains("authorization: Bearer first")));
        assert!(requests
            .iter()
            .any(|request| request.contains("authorization: Bearer second")));
    }
}
