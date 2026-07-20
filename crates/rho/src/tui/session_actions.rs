use ratatui::DefaultTerminal;

use super::{
    is_tool_entry, recovered_history_tail, session_picker, short_session_id,
    transcript_entries_from_messages, App, CommandInvocation, ComposerMode, Entry,
    InteractiveRuntime, Session, RECOVERED_HISTORY_LINE_LIMIT,
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
            Ok(()) => {
                self.info
                    .services
                    .herdr
                    .report_session(self.info.session.session_id.as_deref())
                    .await;
                Ok(())
            }
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
        let full_id = session.id().to_string();
        let short_id = short_session_id(&full_id);

        let display_history = histories.display;
        agent.resume(session, histories.model).await?;
        self.info.session.session_id = Some(full_id);
        self.info.session.recovered_messages = display_history.clone();
        self.composer = ComposerMode::Input;
        self.input.clear();
        self.paste_segments.clear();
        self.input_cursor = 0;
        self.command_palette_dismissed = false;
        self.clamp_command_selection();
        self.reset_streams();
        self.running = false;
        self.goal = None;
        self.reset_usage();
        self.current_context = None;
        let entries = transcript_entries_from_messages(&display_history, &self.info.runtime.cwd);
        let width = terminal.size()?.width as usize;
        let (_omitted, visible_entries) = recovered_history_tail(
            &entries,
            width,
            RECOVERED_HISTORY_LINE_LIMIT,
            self.info.runtime.max_tool_output_lines,
        );
        self.transcript = visible_entries;
        self.history_lines.invalidate_from(0);
        self.last_inserted_was_tool = self.transcript.last().is_some_and(is_tool_entry);
        self.scroll_history_to_bottom();
        self.clamp_history_scroll_for_terminal(terminal)?;
        self.insert_runtime_notices(agent);
        self.insert_entry(&Entry::Notice(format!("resumed session {short_id}")));
        self.status = format!("resumed {short_id}");
        Ok(())
    }
}
