//! Following another daemon session in this TUI: `/new`, `/resume`, and a
//! `/sessions` pick.
//!
//! The session to follow opens on a connection of its own before anything
//! here changes, so a refusal leaves the TUI on the session it had; a new
//! one only connects there, as its first prompt opens it. Then
//! the old session's connection closes, which detaches it without
//! cancelling anything, and the screen resets. These commands run only
//! while the TUI shows no turn, so work is left behind only in a session
//! the TUI had stopped following, or whose turn began unseen; it keeps
//! running in the daemon and can be resumed. A resumed session's attach
//! replays its conversation as a first attach does. A session of another
//! workspace, as `/jobs` attaches to, moves the TUI to that workspace
//! before its replay shows.

use std::path::PathBuf;

use super::{
    daemon_launch::DaemonTarget, palette::PaletteCaches, App, ComposerMode, DefaultTerminal,
    InputSubmissionMode,
};
use crate::session::{Session, SessionTarget};

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
        self.start_follow_ups = false;
        self.daemon_compact_handoff = false;
        self.pending.clear_follow_ups();
        self.pending.clear_steering();
        self.pending.clear_input_action();
        self.pending_input_changed();
        self.reset_streams();
        self.end_busy_ui();
        self.turn.clear_tool_calls();
        self.reset_usage();
        self.usage.current_context = None;
        self.turn.set_current_turn_start(None);
        self.discard_side_chat();
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
        let opened = self.info.session.daemon.open_sibling(target).await?;
        self.info.session.session_id = opened.saved_id().map(str::to_owned);
        // The session followed until now detaches as its connection drops.
        self.info.session.daemon = opened;
        let workspace = self.info.session.daemon.workspace().to_path_buf();
        if workspace != self.info.runtime.cwd {
            self.move_to_workspace(workspace);
        }
        self.sessions_hub_state.clear();
        self.daemon_option_requests.clear();
        self.daemon_login_selected = None;
        self.daemon_unshown_switch = None;
        self.daemon_title = None;
        self.awaited_daemon_report = None;
        self.reset_daemon_mcp_offers();
        // Held for the session left behind.
        self.held_turns.clear();
        self.clear_tool_sources_loading_activity();
        self.reset_daemon_rail();
        self.drop_daemon_login();
        self.reset_session_view(terminal)?;
        // A new session sends no reset replay to clear what the old one
        // left, such as its checklist, until its first prompt opens it.
        self.clear_daemon_transcript();
        // What the attach answered with, the session's options among it:
        // a session at rest sends nothing more until it is prompted.
        self.show_taken_daemon_views(terminal).await?;
        self.sync_herdr_session().await;
        Ok(())
    }

    /// Points what this TUI resolves against its workspace at `workspace`,
    /// the followed session's: the files, skills, and git state it looks
    /// up, and the status line that names them.
    fn move_to_workspace(&mut self, workspace: PathBuf) {
        self.statusline.move_to(workspace.clone());
        self.info.runtime.cwd = workspace;
        self.palette_caches = PaletteCaches::default();
        self.restart_github_pr_fetch();
    }

    /// Follows the saved session `id` names, resolved as `--resume` does,
    /// when it is of this TUI's workspace.
    pub(super) async fn resume_daemon_session(
        &mut self,
        id: &str,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        let target = Session::resolve_target(&self.info.runtime.cwd, id)?;
        // An id lookup can find a session saved under another project;
        // refused before the daemon opens it.
        self.ensure_this_project(&target.cwd)?;
        self.resume_daemon_target(target, terminal).await
    }

    /// Follows the saved session `target`, in whichever workspace it was
    /// saved.
    pub(super) async fn resume_daemon_target(
        &mut self,
        target: SessionTarget,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
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
