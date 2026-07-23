use std::{future::Future, pin::Pin, time::Duration};

use futures_util::task::noop_waker_ref;
use rho_sdk::{CancellationToken, ProviderRequestUsageRecording, SessionId};

use crate::agent::{
    internal_definition, run_one_shot_agent, OneShotAgentRequest, SESSION_TITLE_AGENT_ID,
};

use super::{App, Entry, InteractiveRuntime, Session, SessionTitleResult};

pub(crate) const SESSION_TITLE_PROMPT: &str =
    "Generate a concise title for this chat session. Return only the title, no quotes, no punctuation at the end. Use 3 to 7 words.";

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
    let request = run_one_shot_agent(
        OneShotAgentRequest {
            definition: internal_definition(SESSION_TITLE_AGENT_ID),
            usage_purpose: "title",
            provider_name: &provider_name,
            model: &model,
            input: format!("First user message:\n\n{first_user_message}"),
            cancellation: cancellation.clone(),
            session_id: &session_id,
            workspace_path: &workspace_path,
        },
        usage_recording,
    )?;
    tokio::pin!(request);
    let (result, timed_out) = tokio::select! {
        result = &mut request => (result, false),
        () = tokio::time::sleep(Duration::from_secs(20)) => {
            cancellation.cancel();
            (request.await, true)
        }
    };
    let blocks = match result {
        Err(_) if timed_out => return Err(anyhow::anyhow!("title generation timed out")),
        result => result?,
    };
    let title = blocks.join(" ");
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

impl App {
    pub(super) fn poll_pending_session_title(&mut self) -> anyhow::Result<bool> {
        let Some(future) = self.pending_session_title.as_mut() else {
            return Ok(false);
        };
        let waker = noop_waker_ref();
        let mut context = std::task::Context::from_waker(waker);
        let std::task::Poll::Ready(result) = Pin::new(future).poll(&mut context) else {
            return Ok(false);
        };
        self.pending_session_title = None;
        let Ok(title) = result.title else {
            return Ok(false);
        };
        if Session::set_title(&self.info.runtime.cwd, &result.session_id, &title).is_err() {
            return Ok(false);
        }
        if self.info.session.session_id.as_deref() == Some(result.session_id.as_str()) {
            self.insert_entry(&Entry::Notice(format!("session titled: {title}")));
        }
        Ok(true)
    }

    pub(super) fn start_session_title_generation(
        &mut self,
        first_user_message: String,
        agent: &InteractiveRuntime,
    ) {
        if self.info.session.session_id.is_none() {
            return;
        }
        let session_id = agent.session_id().clone();
        let workspace_path = agent.workspace_path().to_path_buf();
        let usage_recording = agent.usage_recording();
        self.pending_session_title = None;
        let (provider, model, _auth) =
            self.internal_agent_model_selection(crate::agent::SESSION_TITLE_AGENT_ID);
        let cancellation = rho_sdk::CancellationToken::new();
        let task_cancellation = cancellation.clone();
        let task_session_id = session_id.clone();
        let handle = tokio::spawn(async move {
            let title = generate_session_title(
                provider,
                model,
                first_user_message,
                task_session_id.clone(),
                workspace_path,
                usage_recording,
                task_cancellation,
            )
            .await;
            SessionTitleResult {
                session_id: task_session_id.to_string(),
                title,
            }
        });
        self.pending_session_title = Some(PendingSessionTitle::new(
            session_id.to_string(),
            cancellation,
            handle,
        ));
    }
}

#[cfg(test)]
#[path = "session_title_tests.rs"]
mod tests;
