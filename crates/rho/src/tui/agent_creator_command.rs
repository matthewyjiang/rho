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
        terminal: &mut ratatui::DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let missing_tools = ["skill", "questionnaire", "save_agent"]
            .into_iter()
            .filter(|name| !agent.has_tool(name))
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
