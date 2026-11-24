//! Names sessions for their host, `/resume`, and `rho sessions`: once a
//! runtime saves the first prompt it runs, it asks its host to name the
//! session from that message, unless the session has a title. The title
//! agent then works beside the turn, so the name lands while a long first
//! turn still runs, and a turn that is cancelled or fails is named all the
//! same; after a turn that fails, the next prompt asks again, since the
//! title model may have failed with it. A loaded session that was never
//! named is asked for at the first prompt of its reload. Either way the
//! session is named from the first message its transcript shows. Until the title lands, or if it
//! fails, or if no host runs it, the session is listed by its first
//! message.
//!
//! The runtime only asks, with a [`SessionEvent::TitleRequested`]: the host
//! runs the job it carries, and shows the title it saves if the session is
//! still its own (the daemon, as a `title_changed` event, #1670, which
//! `rho acp` passes on as a `session_info_update`). The daemon titles
//! every session it runs, `rho run --detach` runs included: they are
//! listed and followed like any other. In-process headless hosts (`rho
//! run`, subagents, workflow nodes) drop the job rather than spend a model
//! call on every run.
//!
//! [`SessionEvent::TitleRequested`]: super::SessionEvent::TitleRequested

use rho_sdk::{model::Message, UserInput};

use super::{model_scoped_config, SessionRuntime};
use crate::{
    app::title_agent::{generate_title, TitleRequest},
    config::Config,
    session::{NoSessionFound, Session as StoredSession},
};

/// What the title agent needs to name one session, and where the title is
/// saved.
pub(crate) struct TitleJob {
    request: TitleRequest,
    stored_id: String,
}

/// Whether a runtime asked to name its session: it asks at the first
/// prompt it saves, and again at the prompt after a turn that failed.
pub(super) enum TitleState {
    Unasked,
    Asked,
}

impl TitleJob {
    /// Generates and saves the title. Yields it only when this request
    /// saved it: a title someone else set first wins and is not resent, and
    /// a session deleted since its prompt gets none, so no model call names
    /// it.
    pub(crate) async fn generate(self) -> Option<String> {
        let Self { request, stored_id } = self;
        let cwd = request.cwd.clone();
        match StoredSession::title_is_set(&cwd, &stored_id) {
            Ok(true) => return None,
            Err(error) if error.is::<NoSessionFound>() => return None,
            // A title that could not be read is generated all the same; the
            // save keeps one set meanwhile.
            Ok(false) | Err(_) => {}
        }
        let title = generate_title(request).await;
        match title.and_then(|title| StoredSession::set_generated_title(&cwd, &stored_id, &title)) {
            Ok(saved) => saved.map(|update| update.title),
            // Deleted while the title agent worked.
            Err(error) if error.is::<NoSessionFound>() => None,
            Err(error) => {
                // Nothing else says why a session kept its first message
                // for a name, such as a title model nobody signed in to.
                tracing::warn!(error = %format!("{error:#}"), "could not title session");
                None
            }
        }
    }
}

impl SessionRuntime {
    /// The job that names the session, when the runtime has yet to ask for
    /// one on a session with a file, and `None` otherwise. `prompt` is the
    /// one a turn just saved, as the user sent it. The session is named
    /// from the first message its transcript shows: `prompt` in a new
    /// session, an earlier one when the session was reloaded untitled or
    /// its last turn failed.
    pub(super) fn request_title(
        &mut self,
        process_config: &Config,
        prompt: &UserInput,
    ) -> Option<TitleJob> {
        let stored = self.storage.file()?;
        match std::mem::replace(&mut self.title, TitleState::Asked) {
            TitleState::Unasked => {}
            TitleState::Asked => return None,
        }
        // The first message as its sender wrote it, from the transcript the
        // session saved: `prompt` itself in a new session. The model's
        // history holds it with whatever context its client added, and not
        // at all for a turn cut off before its first step.
        let shown = stored.active_display_history().unwrap_or_else(|error| {
            tracing::debug!(%error, "could not read the session's first message");
            Vec::new()
        });
        let first = shown
            .iter()
            .find_map(|row| match &row.message {
                Message::User(blocks) => Some(&**blocks),
                Message::System(_)
                | Message::Assistant(_)
                | Message::EnrichedAssistant(_)
                | Message::AbortedAssistant(_)
                | Message::ToolResult(_)
                | Message::CompactionSummary(_)
                | Message::ModelContext(_)
                | Message::BoundaryInput(_)
                | Message::ReasoningShift(_) => None,
            })
            .unwrap_or(prompt.blocks());
        Some(TitleJob {
            request: TitleRequest {
                config: model_scoped_config(&self.current_model(), process_config),
                config_path: self.build.launch.config_path.clone(),
                cwd: self.storage.cwd().to_path_buf(),
                added_dirs: self.added_dirs.clone(),
                environment: self.build.host.environment.clone(),
                session: self.built.session.id().clone(),
                input: crate::title::session_input(first),
            },
            stored_id: stored.id().to_owned(),
        })
    }

    /// Has the next prompt ask for a title again, after a turn that
    /// failed: the title model may share what failed it, such as an outage
    /// or an expired sign-in. A title saved meanwhile costs that request no
    /// model call (see [`TitleJob::generate`]); a title job still under way
    /// is doubled, and the first of the two to save wins.
    pub(super) fn ask_for_title_again(&mut self) {
        self.title = TitleState::Unasked;
    }

    /// The title the session's file has saved, if any: none for a session
    /// not saved yet, or one whose title could not be read.
    pub(crate) fn stored_title(&self) -> Option<String> {
        let stored = self.storage.file()?;
        StoredSession::title(self.storage.cwd(), stored.id()).unwrap_or_else(|error| {
            tracing::debug!(%error, "could not read the session's title");
            None
        })
    }
}

#[cfg(test)]
#[path = "title_tests.rs"]
mod tests;
