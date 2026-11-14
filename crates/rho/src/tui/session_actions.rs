use crate::tui::DefaultTerminal;

use super::{
    session_picker, App, CommandInvocation, ComposerMode, Entry, InlineChoice, InlineChoiceOption,
    InlineChoicePending, Session,
};
use crate::session::{is_cross_project, SessionListing, SessionSummary, SessionTarget};

impl App {
    /// The id the followed session is saved under, once it is.
    pub(super) fn current_session_id(&self) -> Option<String> {
        self.info.session.daemon.saved_id().map(str::to_owned)
    }

    pub(super) async fn execute_resume_command(
        &mut self,
        invocation: CommandInvocation,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        if self.info.session.no_save {
            return self.open_resume_picker();
        }
        let session_id = invocation.args.trim();
        if !session_id.is_empty() {
            return self.submit_resume_selection(session_id, terminal).await;
        }

        self.open_resume_picker()
    }

    pub(super) fn open_resume_picker(&mut self) -> anyhow::Result<()> {
        if self.info.session.no_save {
            self.insert_entry(&Entry::Notice(
                "resume unavailable with --no-save; start Rho without it to resume".into(),
            ));
            return Ok(());
        }
        self.show_resume_picker(Session::list(
            &self.info.runtime.cwd,
            SessionListing::PersonFacing,
        ))
    }

    /// Open the resume picker for a listing of this workspace's sessions.
    pub(super) fn show_resume_picker(
        &mut self,
        sessions: anyhow::Result<Vec<SessionSummary>>,
    ) -> anyhow::Result<()> {
        match sessions {
            Ok(sessions) if sessions.is_empty() => {
                self.input_ui.set_composer(ComposerMode::Input);
                self.set_status("no saved sessions for this workspace");
            }
            Ok(sessions) => {
                let picker =
                    session_picker::session_picker(sessions, self.current_session_id().as_deref());
                if picker.items.is_empty() {
                    self.input_ui.set_composer(ComposerMode::Input);
                    self.set_status("no other saved sessions for this workspace");
                    return Ok(());
                }
                self.input_ui.set_composer(ComposerMode::Picker(picker));
                self.set_status("select session");
            }
            Err(err) => {
                self.input_ui.set_composer(ComposerMode::Input);
                self.insert_entry(&Entry::Error(format!("could not list sessions: {err}")));
                self.set_status("resume failed");
            }
        }
        Ok(())
    }

    pub(super) fn prompt_delete_selected_session(&mut self) -> anyhow::Result<()> {
        let Some(session_id) = self.selected_resume_session_id() else {
            return Ok(());
        };
        self.prompt_delete_session(SessionTarget::new(
            session_id,
            self.info.runtime.cwd.clone(),
        ))
    }

    pub(super) fn prompt_delete_session(&mut self, target: SessionTarget) -> anyhow::Result<()> {
        if self.refuse_while_sessions_delete_runs() {
            return Ok(());
        }
        let short = session_picker::short_session_id(&target.id);
        let choice = InlineChoice::new(
            format!("Delete session {short}?"),
            "Removes the transcript, cached web content, and this session's subagent runs. Usage history is kept.",
            vec![
                InlineChoiceOption::available(
                    "delete",
                    'd',
                    "Delete",
                    "Permanently remove this saved session",
                ),
                InlineChoiceOption::available(
                    "cancel",
                    'c',
                    "Cancel",
                    "Keep the session and return to the picker",
                )
                .with_alternate_shortcut('n'),
            ],
        )?;
        self.open_choice_over_picker(
            choice,
            InlineChoicePending::DeleteSession { target },
            "confirm delete",
        )
    }

    fn selected_resume_session_id(&self) -> Option<String> {
        match self.input_ui.composer() {
            ComposerMode::Picker(picker) if picker.is_resume_session() => {
                picker.selected_item().map(|item| item.value.clone())
            }
            _ => None,
        }
    }

    pub(super) async fn submit_resume_selection(
        &mut self,
        session_id: &str,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        match self.resume_session_by_id(session_id, terminal).await {
            Ok(()) => Ok(()),
            Err(err) => {
                self.report_resume_error(err);
                Ok(())
            }
        }
    }

    pub(super) async fn submit_resume_target(
        &mut self,
        target: &SessionTarget,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        match self.resume_session_target(target, terminal).await {
            Ok(()) => Ok(()),
            Err(err) => {
                self.report_resume_error(err);
                Ok(())
            }
        }
    }

    fn report_resume_error(&mut self, error: anyhow::Error) {
        self.input_ui.set_composer(ComposerMode::Input);
        self.sessions_hub_state.clear();
        self.insert_entry(&Entry::Error(format!("could not resume session: {error}")));
        self.set_status("resume failed");
    }

    /// The daemon reopens the session, as the agent it was saved as, and
    /// says on its next turn when its model or prompt changed.
    async fn resume_session_by_id(
        &mut self,
        session_id: &str,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        self.ensure_resume_saves()?;
        self.resume_daemon_session(session_id, terminal).await
    }

    async fn resume_session_target(
        &mut self,
        target: &SessionTarget,
        terminal: &mut DefaultTerminal,
    ) -> anyhow::Result<()> {
        self.ensure_resume_saves()?;
        self.ensure_this_project(&target.cwd)?;
        self.resume_daemon_target(target.clone(), terminal).await
    }

    // Checked before the daemon opens the session.
    fn ensure_resume_saves(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.info.session.no_save,
            "resume unavailable with --no-save; start Rho without it to resume"
        );
        Ok(())
    }

    pub(super) fn ensure_this_project(&self, session_cwd: &std::path::Path) -> anyhow::Result<()> {
        anyhow::ensure!(
            !is_cross_project(session_cwd, &self.info.runtime.cwd),
            "start Rho in {} to resume this session",
            crate::paths::display(session_cwd)
        );
        Ok(())
    }
}
