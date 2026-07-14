use futures_util::StreamExt;
use reqwest::StatusCode;

use crate::{
    auth::github_copilot_token::{GitHubCopilotAuthManager, GitHubCopilotAuthMaterial},
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

const DEFAULT_COPILOT_CHAT_COMPLETIONS_URL: &str = "https://api.githubcopilot.com/chat/completions";
const USER_AGENT: &str = concat!("rho/", env!("CARGO_PKG_VERSION"));
const EDITOR_VERSION: &str = concat!("rho/", env!("CARGO_PKG_VERSION"));
const EDITOR_PLUGIN_VERSION: &str = concat!("rho/", env!("CARGO_PKG_VERSION"));
const COPILOT_INTEGRATION_ID: &str = "vscode-chat";

pub struct GitHubCopilotProvider {
    client: reqwest::Client,
    auth: GitHubCopilotAuthManager,
    model: String,
}

impl GitHubCopilotProvider {
    pub fn new(model: String, auth: GitHubCopilotAuthManager) -> Result<Self, ModelError> {
        auth.ensure_auth_available()?;
        Ok(Self {
            client: provider_client(),
            auth,
            model,
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
        }
    }

    fn chat_request(
        &self,
        request: ModelRequest<'_>,
        stream: bool,
    ) -> Result<ChatRequest, ModelError> {
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
        Ok(ChatRequest {
            model: self.model.clone(),
            messages,
            tools: has_tools.then_some(tools),
            tool_choice: has_tools.then_some("auto"),
            stream,
            stream_options: stream.then_some(ChatStreamOptions {
                include_usage: true,
            }),
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
            .header("User-Agent", USER_AGENT)
            .header("Editor-Version", EDITOR_VERSION)
            .header("Editor-Plugin-Version", EDITOR_PLUGIN_VERSION)
            .header("Copilot-Integration-Id", COPILOT_INTEGRATION_ID)
    }

    async fn send_chat_once(
        &self,
        body: &ChatRequest,
        auth: &GitHubCopilotAuthMaterial,
    ) -> Result<reqwest::Response, ModelError> {
        let endpoint = if auth.chat_endpoint.trim().is_empty() {
            DEFAULT_COPILOT_CHAT_COMPLETIONS_URL
        } else {
            auth.chat_endpoint.as_str()
        };
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
    ) -> Result<reqwest::Response, ModelError> {
        let response = self.send_chat_once(&body, &auth).await?;
        if response.status() != StatusCode::UNAUTHORIZED {
            return Ok(response);
        }
        if let Some(refreshed) = self.auth.force_refresh(&self.client).await? {
            return self.send_chat_once(&body, &refreshed).await;
        }
        Ok(response)
    }
}

#[async_trait::async_trait(?Send)]
impl ModelProvider for GitHubCopilotProvider {
    async fn send_turn(&self, request: ModelRequest<'_>) -> Result<ModelResponse, ModelError> {
        let body = self.chat_request(request, false)?;
        let auth = self.auth.auth_material(&self.client).await?;
        let response = self.send_chat_with_retry(body, auth).await?;
        if !response.status().is_success() {
            return Err(http_status_error(response).await);
        }
        let response: ChatResponse = response.json().await?;
        convert_openai_response(response)
    }

    async fn send_turn_stream(
        &self,
        request: ModelRequest<'_>,
        on_event: &mut dyn FnMut(ModelEvent) -> Result<(), ModelError>,
    ) -> Result<ModelResponse, ModelError> {
        let cancellation = request.cancellation.clone();
        tokio::select! {
            result = self.send_turn_stream_inner(request, on_event) => result,
            () = cancellation.cancelled() => Err(ModelError::Interrupted),
        }
    }
}

impl GitHubCopilotProvider {
    async fn send_turn_stream_inner(
        &self,
        request: ModelRequest<'_>,
        on_event: &mut dyn FnMut(ModelEvent) -> Result<(), ModelError>,
    ) -> Result<ModelResponse, ModelError> {
        let body = self.chat_request(request, true)?;
        let auth = self.auth.auth_material(&self.client).await?;
        let response = self.send_chat_with_retry(body, auth).await?;
        if !response.status().is_success() {
            return Err(http_status_error(response).await);
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

async fn http_status_error(response: reqwest::Response) -> ModelError {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if status == StatusCode::UNAUTHORIZED {
        ModelError::MissingGithubCopilotAuth
    } else {
        ModelError::HttpStatus { status, body }
    }
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
        let provider =
            GitHubCopilotProvider::new("gpt-4.1".into(), GitHubCopilotAuthManager::new(store))
                .unwrap();

        let body = provider
            .chat_request(
                ModelRequest {
                    messages: &[Message::user_text("hello")],
                    tools: &[],
                    cancellation: Default::default(),
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
        let result = GitHubCopilotProvider::new(
            "gpt-4.1".into(),
            GitHubCopilotAuthManager::new_with_env_token(
                Arc::new(MemoryCredentialStore::default()),
                None,
            ),
        );

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
                    "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}",
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
            GitHubCopilotAuthManager::new(store),
            reqwest::Client::new(),
        );

        let response = provider
            .send_turn(ModelRequest {
                messages: &[Message::user_text("hello")],
                tools: &[],
                cancellation: Default::default(),
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
