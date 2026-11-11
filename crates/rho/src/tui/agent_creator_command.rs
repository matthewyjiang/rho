use super::{
    command_palette::slash_command_args, skill_actions::SkillCommandAction, App, ChatMedia,
    CommandInvocation, Entry, InteractiveRuntime, TurnPrompt,
};

pub(super) fn create_agent_model_prompt(
    invocation: &CommandInvocation,
    turn: &TurnPrompt,
) -> String {
    let request =
        crate::commands::create_agent_request(&invocation.name, slash_command_args(&turn.model));
    if request.is_empty() {
        "Create a new Rho agent through the guided workflow.".to_string()
    } else {
        format!("Create a new Rho agent through the guided workflow. User request: {request}")
    }
}

impl App {
    pub(super) async fn execute_create_agent_command(
        &mut self,
        invocation: &CommandInvocation,
        turn: TurnPrompt,
        media: Vec<ChatMedia>,
        paste_segments: Vec<super::PasteSegment>,
        terminal: &mut crate::tui::DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        if self.info.session.daemon.is_some() {
            // The daemon offers the skill only to a session that can run it.
            let offered = self.daemon_options.as_ref().is_some_and(|options| {
                options.commands.iter().any(|command| {
                    command.kind == rho_protocol::CommandKind::Skill
                        && command.name == "rho-agent-creator"
                })
            });
            if !offered {
                self.insert_entry(&Entry::Error(
                    "could not start agent creator: the session does not offer the rho-agent-creator skill".into(),
                ));
                self.set_status("agent creator unavailable");
                return Ok(());
            }
        }
        let missing_tools = ["skill", "questionnaire", "read_file", "write", "rho"]
            .into_iter()
            .filter(|name| self.info.session.daemon.is_none() && !agent.has_tool(name))
            .collect::<Vec<_>>();
        if !missing_tools.is_empty() {
            self.insert_entry(&Entry::Error(format!(
                "could not start agent creator: active agent is missing required tools: {}",
                missing_tools.join(", ")
            )));
            self.set_status("agent creator unavailable");
            return Ok(());
        }

        let model_prompt = create_agent_model_prompt(invocation, &turn);
        match self.skill_command_action(
            "skill:rho-agent-creator",
            model_prompt,
            turn.display,
            true,
        )? {
            SkillCommandAction::Prompt(prompt) => {
                self.submit_interactive_turn(*prompt, media, paste_segments, terminal, agent)
                    .await?;
            }
            SkillCommandAction::Rejected => {}
            SkillCommandAction::NotSkill => {
                self.insert_entry(&Entry::Error(
                    "could not start agent creator: built-in instructions are unavailable".into(),
                ));
                self.set_status("agent creator unavailable");
            }
        }
        Ok(())
    }
}
