//! Names sessions for their host, `/resume`, and `rho sessions`, as the TUI
//! names its own: after the first turn a runtime completes, a background
//! title-agent call turns that exchange into a short title, unless the
//! session has one. A first turn the host cancelled does not count, and a
//! loaded session that was never named gets its title then. Until it lands,
//! or if it fails, the session is listed by its first message.
//!
//! The runtime only asks, from the turn: the host takes the request once the
//! prompt returns, runs it, and shows the title it saves if the runtime that
//! asked still owns the session (ACP, for example, as a
//! `session_info_update`).

use std::path::PathBuf;

use rho_sdk::{
    model::{ContentBlock, Message},
    CancellationToken, UserInput,
};

use super::{model_scoped_config, SessionRuntime};
use crate::{config::Config, session::Session as StoredSession, title::TitleModel};

/// What the title agent needs to name one session.
pub(crate) struct TitleJob {
    model: TitleModel,
    /// The first exchange, as the title agent reads it.
    input: String,
    sdk_session_id: rho_sdk::SessionId,
    stored_id: String,
    cwd: PathBuf,
}

/// How far a runtime got asking to name its session: it asks once, from
/// the first turn that reached the model's answer.
pub(super) enum TitleState {
    /// No turn has asked yet.
    Unasked,
    /// A turn asked, and the host has not taken the job yet.
    Pending(TitleJob),
    /// The host took the job; no later turn asks again.
    Asked,
}

impl TitleJob {
    /// Generates and saves the title. Yields it only when this request
    /// saved it: a title someone else set first wins and is not resent.
    pub(crate) async fn generate(self) -> Option<String> {
        let Self {
            model,
            input,
            sdk_session_id,
            stored_id,
            cwd,
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
                tracing::debug!(%error, "could not title session");
                None
            }
        }
    }
}

impl SessionRuntime {
    /// Asks to name the session from `user`'s message and the reply the
    /// turn ended with, for [`Self::take_title`]. A turn calls it until one
    /// has a reply to name the session from.
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
        self.title = TitleState::Pending(TitleJob {
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

    /// The title a turn of the last prompt asked for. Take it as soon as
    /// the prompt returns; one a prompt dropped mid-await left behind is
    /// taken after the next.
    pub(crate) fn take_title(&mut self) -> Option<TitleJob> {
        let (state, job) = match std::mem::replace(&mut self.title, TitleState::Asked) {
            TitleState::Unasked => (TitleState::Unasked, None),
            TitleState::Pending(job) => (TitleState::Asked, Some(job)),
            TitleState::Asked => (TitleState::Asked, None),
        };
        self.title = state;
        job
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

#[cfg(test)]
#[path = "title_tests.rs"]
mod tests;
