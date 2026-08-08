use ratatui::DefaultTerminal;

use super::{
    session_picker, App, CommandInvocation, ComposerMode, Entry, InlineChoice, InlineChoiceModal,
    InlineChoiceOption, InlineChoicePending, InteractiveRuntime, Session, SessionDeleteReopen,
};
use crate::session::DeleteOptions;

impl App {
    pub(super) async fn execute_resume_command(
        &mut self,
        invocation: CommandInvocation,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let session_id = invocation.args.trim();
        if !session_id.is_empty() {
            return self
                .submit_resume_selection(session_id, terminal, agent)
                .await;
        }

        self.open_resume_picker()
    }

    pub(super) fn open_resume_picker(&mut self) -> anyhow::Result<()> {
        match Session::list(&self.info.runtime.cwd) {
            Ok(sessions) if sessions.is_empty() => {
                self.input_ui.set_composer(ComposerMode::Input);
                self.set_status("no saved sessions for this workspace");
            }
            Ok(sessions) => {
                let picker = session_picker::session_picker(
                    sessions,
                    self.info.session.session_id.as_deref(),
                );
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
        self.prompt_delete_session(session_id, SessionDeleteReopen::ResumePicker)
    }

    pub(super) fn prompt_delete_session(
        &mut self,
        session_id: String,
        reopen: SessionDeleteReopen,
    ) -> anyhow::Result<()> {
        let short = session_picker::short_session_id(&session_id);
        let choice = InlineChoice::new(
            format!("Delete session {short}?"),
            "Removes the transcript, web sidecar, and parent-linked subagent runs. Usage history is kept.",
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
        self.input_ui
            .set_composer(ComposerMode::InlineChoice(InlineChoiceModal {
                choice,
                pending: InlineChoicePending::DeleteSession { session_id, reopen },
            }));
        self.set_status("confirm delete");
        Ok(())
    }

    fn reopen_after_session_delete(&mut self, reopen: SessionDeleteReopen) -> anyhow::Result<()> {
        match reopen {
            SessionDeleteReopen::ResumePicker => self.open_resume_picker(),
            SessionDeleteReopen::SessionsHub => {
                self.open_sessions_hub_or_report();
                Ok(())
            }
        }
    }

    pub(super) fn submit_delete_session_choice(
        &mut self,
        value: &str,
        session_id: &str,
        reopen: SessionDeleteReopen,
    ) -> anyhow::Result<()> {
        if value != "delete" {
            return self.reopen_after_session_delete(reopen);
        }

        let short = session_picker::short_session_id(session_id);
        match Session::delete_by_id(
            &self.info.runtime.cwd,
            session_id,
            DeleteOptions {
                force: false,
                protect_session_id: self.info.session.session_id.clone(),
            },
        ) {
            Ok(outcome) => {
                let mut notice = format!("deleted session {short}");
                if outcome.deleted_run_count > 0 {
                    notice.push_str(&format!(
                        " and {} related run{}",
                        outcome.deleted_run_count,
                        if outcome.deleted_run_count == 1 {
                            ""
                        } else {
                            "s"
                        }
                    ));
                }
                self.reopen_after_session_delete(reopen)?;
                self.set_status(notice);
            }
            Err(err) => {
                self.insert_entry(&Entry::Error(format!("could not delete session: {err}")));
                self.reopen_after_session_delete(reopen)?;
                self.set_status("delete failed");
            }
        }
        Ok(())
    }

    fn selected_resume_session_id(&self) -> Option<String> {
        match self.input_ui.composer() {
            ComposerMode::Picker(picker) if picker.action == super::PickerAction::ResumeSession => {
                picker.selected_item().map(|item| item.value.clone())
            }
            _ => None,
        }
    }

    pub(super) async fn submit_resume_selection(
        &mut self,
        session_id: &str,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        match self.resume_session_by_id(session_id, terminal, agent).await {
            Ok(()) => Ok(()),
            Err(err) => {
                self.input_ui.set_composer(ComposerMode::Input);
                self.insert_entry(&Entry::Error(format!("could not resume session: {err}")));
                self.set_status("resume failed");
                Ok(())
            }
        }
    }

    async fn resume_session_by_id(
        &mut self,
        session_id: &str,
        terminal: &mut DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let (session, histories) =
            Session::open_by_id_with_histories(&self.info.runtime.cwd, session_id)?;
        session.validate_agent_definition_identity(agent.bound_definition())?;

        if self.offer_resume_context_handoff(
            &session,
            &histories.model,
            &histories.display,
            agent,
        )? {
            return Ok(());
        }

        self.apply_resume_session(session, histories.model, histories.display, terminal, agent)
            .await
    }
}

impl App {
    pub(super) fn ensure_session(&mut self, agent: &mut InteractiveRuntime) -> anyhow::Result<()> {
        if self.info.session.session_id.is_none() {
            let session_id = agent.session_id().to_string();
            let (agent_id, agent_fingerprint) = agent.agent_identity();
            let session = Session::create_with_id(
                &self.info.runtime.cwd,
                &session_id,
                agent_id,
                agent_fingerprint,
            )?;
            self.info.session.session_id = Some(session_id);
            agent.attach_storage(session);
        }
        Ok(())
    }
}
