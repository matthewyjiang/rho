//! Names ACP sessions for `/resume` and `rho sessions`, as the TUI names its
//! own: after the first turn a host completes, a background title-agent call
//! turns that exchange into a short title, unless the session has one. A
//! first turn the host cancelled does not count, and a loaded session that
//! was never named gets its title then. Until it lands, or if it fails, the
//! session is listed by its first message.

use rho_sdk::{
    model::{ContentBlock, Message},
    CancellationToken, UserInput,
};

use super::{model_scoped_config, SessionHost};
use crate::{config::Config, session::Session as StoredSession};

impl SessionHost {
    /// Names the session from `user`'s message and the reply the turn ended
    /// with, once per host. A title the session already has wins.
    pub(super) fn spawn_title(&mut self, process_config: &Config, user: &UserInput) {
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
        let input = crate::title::first_turn_input(&text_of(user.blocks()), &text_of(reply));
        let model = crate::title::title_model_from_config(&model_scoped_config(
            &self.current_model(),
            process_config,
        ));
        let session_id = self.built.session.id().clone();
        let stored_id = stored.id().to_owned();
        let cwd = self.storage.cwd().to_path_buf();
        self.title_requested = true;
        tokio::spawn(async move {
            if StoredSession::title_is_set(&cwd, &stored_id).unwrap_or(false) {
                return;
            }
            let usage_recording = crate::usage::default_recording().await;
            let title = crate::title::generate_title(
                model,
                input,
                session_id,
                cwd.clone(),
                usage_recording,
                CancellationToken::new(),
            )
            .await;
            let saved = title
                .and_then(|title| StoredSession::set_generated_title(&cwd, &stored_id, &title));
            if let Err(error) = saved {
                tracing::debug!(%error, "could not title ACP session");
            }
        });
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
