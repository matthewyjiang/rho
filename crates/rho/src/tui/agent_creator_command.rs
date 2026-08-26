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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::{parse_command, CommandId};

    fn prompt_for(input: &str) -> String {
        let invocation = parse_command(input).unwrap().unwrap();
        assert_eq!(invocation.id, CommandId::CreateAgent, "{input}");
        create_agent_model_prompt(
            &invocation,
            &TurnPrompt::standard(input.to_string(), input.to_string()),
        )
    }

    // Covers: the prompt execute_create_agent_command submits must strip
    // `create` from `/agents create` and keep `/create-agent` request text.
    // Owner: slash-command submission ownership
    #[test]
    fn create_agent_model_prompt_matches_both_spellings() {
        let expected = "Create a new Rho agent through the guided workflow. User request: a read-only reviewer";
        assert_eq!(prompt_for("/agents create a read-only reviewer"), expected);
        assert_eq!(prompt_for("/create-agent a read-only reviewer"), expected);
        assert_eq!(
            prompt_for("/agents create"),
            "Create a new Rho agent through the guided workflow."
        );
        assert_eq!(
            prompt_for("/create-agent"),
            "Create a new Rho agent through the guided workflow."
        );
        assert_ne!(
            prompt_for("/agents create a read-only reviewer"),
            "Create a new Rho agent through the guided workflow. User request: create a read-only reviewer"
        );
    }
}
