//! Following another daemon session in this TUI: `/new`, `/resume`, and a
//! `/sessions` pick.
//!
//! The session to follow opens on a connection of its own before anything
//! here changes, so a refusal leaves the TUI on the session it had. Then
//! the old session's connection closes, which detaches it without
//! cancelling anything, and the screen resets. These commands run only
//! while the TUI shows no turn, so work is left behind only in a session
//! the TUI had stopped following, or whose turn began unseen; it keeps
//! running in the daemon and can be resumed. The new session's attach
//! replays its conversation as a first attach does.
//!
//! [`App::reset_session_view`] is also the in-process `/new`'s reset.

use super::{daemon_launch::DaemonTarget, App, ComposerMode, DefaultTerminal, InputSubmissionMode};
use crate::session::Session;

impl App {
    /// Clears what the screen holds of the session it showed, for one that
    /// starts empty or is about to replay.
    pub(super) fn reset_session_view(
        &mut self,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        self.input_ui.set_composer(ComposerMode::Input);
        self.input_ui.clear_text();
        self.input_ui.clear_paste_segments();
        self.input_ui.set_shell_mode(None);
        self.input_ui
            .set_submission_mode(InputSubmissionMode::ParseCommands);
        self.input_ui.reset_history_navigation();
        self.input_ui.set_cursor(0);
        self.cancel_all_pending_attachments();
        self.input_ui.clear_attachments();
        self.input_ui.set_command_palette_dismissed(false);
        self.clamp_command_selection();
        self.start_follow_ups = None;
        self.daemon_compact_handoff = false;
        self.pending.clear_follow_ups();
        self.goal = None;
        self.pending.clear_steering();
        self.pending.clear_input_action();
        self.pending_input_changed();
        self.reset_streams();
        self.end_busy_ui();
        self.turn.clear_tool_calls();
        self.reset_usage();
        self.usage.current_context = None;
        self.pending_session_title = None;
        self.session_title_locked = false;
        self.turn.set_current_turn_start(None);
        self.discard_side_chat();
        self.provisional_history = None;
        self.history.clear_entries();
        self.history.images_mut().clear();
        self.history.set_images_dirty_from(None);
        self.history.lines_mut().invalidate_from(0);
        self.scroll_history_to_bottom();
        self.clamp_history_scroll_for_terminal(terminal)?;
        Ok(())
    }

    /// Follows `target` in place of the daemon session followed now, or
    /// keeps that one when `target` cannot be opened here.
    pub(super) async fn switch_daemon_session(
        &mut self,
        target: DaemonTarget,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        let current = self
            .info
            .session
            .daemon
            .as_ref()
            .expect("switching needs a daemon session");
        let opened = current.open_sibling(target).await?;
        self.info.session.session_id = opened.saved_id().map(str::to_owned);
        // The session followed until now detaches as its connection drops.
        self.info.session.daemon = Some(opened);
        self.sessions_hub_state.clear();
        self.daemon_option_requests.clear();
        self.daemon_login_selected = None;
        self.daemon_title = None;
        self.awaited_daemon_report = None;
        self.reset_daemon_mcp_offers();
        // Held for the session left behind, as `/new` drops them in process.
        self.held_turns.clear();
        self.clear_tool_sources_loading_activity();
        self.reset_daemon_rail();
        self.drop_daemon_login();
        self.reset_session_view(terminal)?;
        // What the attach answered with, the session's options among it:
        // a session at rest sends nothing more until it is prompted.
        self.show_taken_daemon_views(terminal).await?;
        self.sync_herdr_session().await;
        Ok(())
    }

    /// Follows the saved session `id` names, resolved as `--resume` does.
    pub(super) async fn resume_daemon_session(
        &mut self,
        id: &str,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        let target = Session::resolve_target(&self.info.runtime.cwd, id)?;
        // An id lookup can find a session saved under another project;
        // refused before the daemon opens it.
        self.ensure_this_project(&target.cwd)?;
        // It runs as the agent it was saved as, so a launch that named an
        // agent resumes only that agent's sessions, as at startup.
        if let Some(definition) = &self.info.session.resume_launch.definition {
            Session::validate_agent_definition_identity_of(&target, definition)?;
        }
        let short = super::session_picker::short_session_id(&target.id);
        self.switch_daemon_session(
            DaemonTarget::Resume(rho_protocol::SessionId::new(target.id)),
            terminal,
        )
        .await?;
        self.set_status(format!("resumed session {short}"));
        self.request_daemon_context_handoff();
        Ok(())
    }
}
