use std::{future::Future, pin::Pin, time::Duration};

use rho_sdk::{
    model::{ContentBlock, Message, ModelRequest, ModelResponse},
    CancellationToken, ProviderRequestUsageContext, ProviderRequestUsageRecording, ReasoningLevel,
    SessionId,
};

use super::SessionTitleResult;
use rho_providers::providers::build_sdk_provider;

pub(super) struct PendingSessionTitle {
    session_id: String,
    cancellation: CancellationToken,
    handle: tokio::task::JoinHandle<SessionTitleResult>,
}

impl PendingSessionTitle {
    pub(super) fn new(
        session_id: String,
        cancellation: CancellationToken,
        handle: tokio::task::JoinHandle<SessionTitleResult>,
    ) -> Self {
        Self {
            session_id,
            cancellation,
            handle,
        }
    }

    pub(super) fn cancel(&self) {
        self.cancellation.cancel();
    }
}

impl Future for PendingSessionTitle {
    type Output = SessionTitleResult;

    fn poll(
        mut self: Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        match Pin::new(&mut self.handle).poll(context) {
            std::task::Poll::Ready(Ok(result)) => std::task::Poll::Ready(result),
            std::task::Poll::Ready(Err(error)) => std::task::Poll::Ready(SessionTitleResult {
                session_id: self.session_id.clone(),
                title: Err(anyhow::anyhow!("title generation task failed: {error}")),
            }),
            std::task::Poll::Pending => std::task::Poll::Pending,
        }
    }
}

impl Drop for PendingSessionTitle {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}

pub(super) async fn generate_session_title(
    provider_name: String,
    model: String,
    first_user_message: String,
    session_id: SessionId,
    workspace_path: std::path::PathBuf,
    usage_recording: ProviderRequestUsageRecording,
    cancellation: CancellationToken,
) -> anyhow::Result<String> {
    let provider = build_sdk_provider(&provider_name, &model, ReasoningLevel::Low)?;
    let request_messages = vec![
        Message::System(
            "Generate a concise title for this chat session. Return only the title, no quotes, no punctuation at the end. Use 3 to 7 words."
                .into(),
        ),
        Message::user_text(format!("First user message:\n\n{first_user_message}")),
    ];
    let usage_context = ProviderRequestUsageContext::for_purpose(provider.identity(), "title")
        .with_session_id(session_id)
        .with_workspace_path(workspace_path);
    let request = crate::usage::send_recorded(
        provider.as_ref(),
        ModelRequest {
            messages: &request_messages,
            tools: &[],
            cancellation: cancellation.clone(),
            reasoning_level: ReasoningLevel::Low,
            prompt_cache_key: None,
        },
        usage_context,
        usage_recording,
    );
    tokio::pin!(request);
    let (result, timed_out) = tokio::select! {
        result = &mut request => (result, false),
        () = tokio::time::sleep(Duration::from_secs(20)) => {
            cancellation.cancel();
            (request.await, true)
        }
    };
    let (response, _) = match result {
        Err(_) if timed_out => return Err(anyhow::anyhow!("title generation timed out")),
        result => result?,
    };
    let ModelResponse::Assistant(blocks) = response;
    let title = blocks
        .into_iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text),
            ContentBlock::Image(_) | ContentBlock::ToolCall(_) => None,
        })
        .collect::<Vec<_>>()
        .join(" ");
    sanitize_session_title(&title)
        .ok_or_else(|| anyhow::anyhow!("title model returned an empty title"))
}

pub(super) fn sanitize_session_title(title: &str) -> Option<String> {
    let title = title
        .lines()
        .find(|line| !line.trim().is_empty())?
        .trim()
        .trim_matches(|ch| matches!(ch, '"' | '\'' | '`' | '*' | '#'))
        .trim()
        .trim_end_matches(['.', ':', ';'])
        .trim();
    if title.is_empty() {
        return None;
    }
    let mut title = title.split_whitespace().collect::<Vec<_>>().join(" ");
    if title.chars().count() > 80 {
        title = title.chars().take(79).collect();
        title.push('…');
    }
    Some(title)
}

#[cfg(test)]
#[path = "session_title_tests.rs"]
mod tests;
