use ratatui::DefaultTerminal;

use super::{
    session_picker, App, CommandInvocation, ComposerMode, Entry, InteractiveRuntime, Session,
};

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
                self.insert_entry(&Entry::Notice(
                    "no saved sessions for this workspace".into(),
                ));
                self.status = "no sessions".into();
            }
            Ok(sessions) => {
                let picker = session_picker::session_picker(
                    sessions,
                    self.info.session.session_id.as_deref(),
                );
                if picker.items.is_empty() {
                    self.insert_entry(&Entry::Notice(
                        "no other saved sessions for this workspace".into(),
                    ));
                    self.status = "no sessions".into();
                    return Ok(());
                }
                self.composer = ComposerMode::Picker(picker);
                self.status = "select session".into();
            }
            Err(err) => {
                self.insert_entry(&Entry::Error(format!("could not list sessions: {err}")));
                self.status = "resume failed".into();
            }
        }
        Ok(())
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
                self.composer = ComposerMode::Input;
                self.insert_entry(&Entry::Error(format!("could not resume session: {err}")));
                self.status = "resume failed".into();
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
        let (agent_id, agent_fingerprint) = agent.agent_identity();
        session.validate_agent_identity(agent_id, agent_fingerprint)?;

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
