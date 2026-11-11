//! The guided agent creator through an explicit built-in skill turn. A
//! daemon session runs `/create-agent` itself (`daemon_gate`).

use super::{
    command_palette::slash_command_args, skill_actions::SkillCommandAction, App, ChatMedia,
    CommandInvocation, Entry, InteractiveRuntime, TurnPrompt,
};
use crate::commands::create_agent_turn;

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
        // The expanded model text keeps a pasted request body intact.
        let request = crate::commands::create_agent_request(
            &invocation.name,
            slash_command_args(&turn.model),
        );
        let model_prompt =
            match create_agent_turn::model_prompt(request, |name| agent.has_tool(name)) {
                Ok(model_prompt) => model_prompt,
                Err(error) => {
                    self.insert_entry(&Entry::Error(error.to_string()));
                    self.set_status("agent creator unavailable");
                    return Ok(());
                }
            };
        match self.skill_command_action(
            &format!("skill:{}", create_agent_turn::SKILL),
            model_prompt,
            turn.display,
            /*skill_tool_available*/ true,
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
