//! Names ACP sessions for the host, `/resume`, and `rho sessions`, as the TUI
//! names its own: after the first turn a host completes, a background
//! title-agent call turns that exchange into a short title, unless the
//! session has one. A first turn the host cancelled does not count, and a
//! loaded session that was never named gets its title then. Until it lands,
//! or if it fails, the session is listed by its first message.
//!
//! The host only asks; the agent runs the request once the prompt returns
//! and sends the title it saves as `session_info_update`, if the host that
//! asked still owns the session (`agent_titles.rs`).

use std::{path::PathBuf, sync::Arc};

use agent_client_protocol::schema::v1::SessionId;
use rho_sdk::{
    model::{ContentBlock, Message},
    CancellationToken, UserInput,
};

use super::{model_scoped_config, PromptGate, SessionHost};
use crate::{config::Config, session::Session as StoredSession, title::TitleModel};

/// A title one host asked for, not yet generated.
pub(in crate::app::acp) struct PendingTitle {
    pub(in crate::app::acp) session_id: SessionId,
    /// The asking host's prompt gate, which its published slot shares, so
    /// the agent can tell that host from a successor with the same ID.
    pub(in crate::app::acp) host: Arc<PromptGate>,
    model: TitleModel,
    /// The first exchange, as the title agent reads it.
    input: String,
    sdk_session_id: rho_sdk::SessionId,
    stored_id: String,
    cwd: PathBuf,
}

impl PendingTitle {
    /// Generates and saves the title. Yields it only when this request
    /// saved it: a title someone else set first wins and is not resent.
    pub(in crate::app::acp) async fn generate(self) -> Option<String> {
        let Self {
            model,
            input,
            sdk_session_id,
            stored_id,
            cwd,
            ..
        } = self;
        if StoredSession::title_is_set(&cwd, &stored_id).unwrap_or(false) {
            return None;
        }
        let usage_recording = crate::usage::default_recording().await;
        let title = crate::title::generate_title(
            model,
            input,
            sdk_session_id,
            cwd.clone(),
            usage_recording,
            CancellationToken::new(),
        )
        .await;
        match title.and_then(|title| StoredSession::set_generated_title(&cwd, &stored_id, &title)) {
            Ok(saved) => saved.map(|update| update.title),
            Err(error) => {
                tracing::debug!(%error, "could not title ACP session");
                None
            }
        }
    }
}

impl SessionHost {
    /// Asks to name the session from `user`'s message and the reply the
    /// turn ended with, once per host, for [`Self::take_title`].
    pub(super) fn request_title(&mut self, process_config: &Config, user: &UserInput) {
        let Some(stored) = self.storage.file() else {
            return;
        };
        let history = self.built.session.history();
        let Some(reply) = history
            .last()
            .and_then(Message::completed_assistant_content)
        else {
            return;
        };
        self.title_requested = true;
        self.pending_title = Some(PendingTitle {
            session_id: self.acp_session_id.clone(),
            host: Arc::clone(&self.prompt_gate),
            model: crate::title::title_model_from_config(&model_scoped_config(
                &self.current_model(),
                process_config,
            )),
            input: crate::title::first_turn_input(&text_of(user.blocks()), &text_of(reply)),
            sdk_session_id: self.built.session.id().clone(),
            stored_id: stored.id().to_owned(),
            cwd: self.storage.cwd().to_path_buf(),
        });
    }

    /// The title the last prompt asked for, which the caller must run.
    pub(in crate::app::acp) fn take_title(&mut self) -> Option<PendingTitle> {
        self.pending_title.take()
    }
}

fn text_of(blocks: &[ContentBlock]) -> String {
    blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.as_str()),
            ContentBlock::Image(_) | ContentBlock::ToolCall(_) => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}
