use std::{future::Future, pin::Pin, time::Duration};

use futures_util::task::noop_waker_ref;
use rho_sdk::{CancellationToken, ProviderRequestUsageRecording, SessionId};

use crate::agent::{
    effective_internal_agent_reasoning, internal_definition, run_one_shot_agent,
    OneShotAgentRequest, SESSION_TITLE_AGENT_ID,
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

#[allow(clippy::too_many_arguments)] // provider/model/auth identity plus run context
pub(super) async fn generate_session_title(
    provider_name: String,
    model: String,
    auth: String,
    reasoning: rho_providers::reasoning::ReasoningLevel,
    first_user_message: String,
    first_assistant_message: String,
    session_id: SessionId,
    workspace_path: std::path::PathBuf,
    usage_recording: ProviderRequestUsageRecording,
    cancellation: CancellationToken,
) -> anyhow::Result<String> {
    let request = run_one_shot_agent(
        OneShotAgentRequest {
            definition: internal_definition(SESSION_TITLE_AGENT_ID),
            usage_purpose: "title",
            reasoning: Some(reasoning),
            input: vec![rho_sdk::model::ContentBlock::Text(format!(
                "First turn:\n\nUser:\n{first_user_message}\n\nAssistant:\n{first_assistant_message}"
            ))],
            cancellation: cancellation.clone(),
            session_id: &session_id,
            workspace_path: &workspace_path,
        },
        &provider_name,
        &model,
        &auth,
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
    let result = match result {
        Err(_) if timed_out => return Err(anyhow::anyhow!("title generation timed out")),
        result => result?,
    };
    let title = result.texts.join(" ");
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
        // In-process /title lock, or CAS so external `sessions rename` wins.
        if self.session_title_locked {
            return Ok(false);
        }
        let Ok(Some(updated)) =
            Session::set_generated_title(&self.info.runtime.cwd, &result.session_id, &title)
        else {
            return Ok(false);
        };
        if self.info.session.session_id.as_deref() == Some(result.session_id.as_str()) {
            self.insert_entry(&Entry::Notice(format!("session titled: {}", updated.title)));
        }
        Ok(true)
    }

    pub(super) fn start_session_title_generation(
        &mut self,
        first_user_message: &str,
        first_assistant_message: &str,
        agent: &InteractiveRuntime,
    ) {
        let Some(session_id) = self.info.session.session_id.as_deref() else {
            return;
        };
        if self.session_title_locked
            || Session::title_is_set(&self.info.runtime.cwd, session_id).unwrap_or(false)
        {
            return;
        }
        let session_id = agent.session_id().clone();
        let workspace_path = agent.workspace_path().to_path_buf();
        let usage_recording = agent.usage_recording();
        self.pending_session_title = None;
        let configured = self.internal_agent_model_selection(crate::agent::SESSION_TITLE_AGENT_ID);
        let reasoning =
            effective_internal_agent_reasoning(crate::agent::SESSION_TITLE_AGENT_ID, &configured);
        let selection = super::model_actions::expect_rho_internal_agent_model(
            crate::agent::SESSION_TITLE_AGENT_ID,
            configured,
        );
        let cancellation = rho_sdk::CancellationToken::new();
        let task_cancellation = cancellation.clone();
        let task_session_id = session_id.clone();
        let first_user_message = first_user_message.to_owned();
        let first_assistant_message = first_assistant_message.to_owned();
        let handle = tokio::spawn(async move {
            let title = generate_session_title(
                selection.provider,
                selection.model,
                selection.auth,
                reasoning,
                first_user_message,
                first_assistant_message,
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
