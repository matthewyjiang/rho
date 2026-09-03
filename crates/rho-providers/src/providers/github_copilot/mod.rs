use crate::protocol::openai_chat::{
    convert_openai_response, invalid_stream_utf8, response_without_stream_context,
    to_openai_message_for_target, to_openai_tool, ChatStreamAccumulator, ChatToolCallPolicy,
};
use reqwest::StatusCode;

use crate::{
    auth::github_copilot_token::{GitHubCopilotAuthManager, GitHubCopilotAuthMaterial},
    model::{ModelError, ModelEvent, ModelIdentity, ModelRequest, ModelResponse, ModelUsage},
    protocol::openai_chat::{ChatRequest, ChatResponse, ChatStreamOptions},
    provider_backend::line_stream::collect_line_stream,
    providers::responses_http::post_with_optional_refresh,
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
            parallel_tool_calls: has_tools.then_some(true),
            stream,
            stream_options: stream.then_some(ChatStreamOptions {
                include_usage: true,
            }),
            prompt_cache_key: request.prompt_cache_key.map(str::to_owned),
            reasoning: None,
            reasoning_effort: None,
            thinking: None,
            chat_template_kwargs: None,
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
        post_with_optional_refresh(
            response,
            || self.auth.force_refresh(&self.client),
            || {
                if let Some(on_request_event) = on_request_event {
                    on_request_event(
                        rho_sdk::provider::ProviderRequestEvent::RequestAttemptFailed {
                            kind: rho_sdk::ProviderErrorKind::Authentication,
                            usage: ModelUsage::default(),
                        },
                    )?;
                }
                Ok(())
            },
            |refreshed| async move { self.send_chat_once(&body, &refreshed).await },
            None,
        )
        .await
        .response
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
        Ok(response_without_stream_context(convert_openai_response(
            response,
            ChatToolCallPolicy::Strict,
        )?))
    }

    /// Streams one turn through a `Send` callback for the public SDK adapter.
    pub(crate) async fn stream_turn(
        &self,
        request: ModelRequest<'_>,
        on_event: &mut (dyn FnMut(ModelEvent) -> Result<(), ModelError> + Send),
        on_request_event: &mut (dyn FnMut(rho_sdk::provider::ProviderRequestEvent) -> Result<(), ModelError>
                  + Send),
    ) -> Result<ModelResponse, ModelError> {
        self.send_turn_stream_inner(request, on_event, on_request_event)
            .await
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

        let mut chat_stream = ChatStreamAccumulator::default();
        collect_line_stream(response, invalid_stream_utf8, |line| {
            chat_stream.handle_line(line, on_event)
        })
        .await?;
        chat_stream.finish(on_event)
    }
}

async fn error_for_status(response: reqwest::Response) -> Result<reqwest::Response, ModelError> {
    if response.status() == StatusCode::UNAUTHORIZED {
        return Err(crate::model::registry::missing_credentials_error(
            "github-copilot",
        ));
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
    fn provider_construction_requires_available_auth() {
        let result = GitHubCopilotAuthManager::new_with_env_token(
            Arc::new(MemoryCredentialStore::default()),
            None,
        )
        .and_then(|auth| GitHubCopilotProvider::new("gpt-4.1".into(), auth));

        let error = match result {
            Ok(_) => panic!("provider construction should require auth"),
            Err(error) => error,
        };
        assert_eq!(
            error.to_string(),
            crate::model::registry::missing_credentials_error("github-copilot").to_string()
        );
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
            crate::reqwest_client(),
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
