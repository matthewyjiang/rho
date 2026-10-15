//! Project instruction onboarding through an explicit built-in skill turn.

use super::{
    skill_actions::SkillCommandAction, App, ChatMedia, Entry, InteractiveRuntime, TurnPrompt,
};

impl App {
    pub(super) async fn execute_init_command(
        &mut self,
        turn: TurnPrompt,
        media: Vec<ChatMedia>,
        paste_segments: Vec<super::PasteSegment>,
        terminal: &mut crate::tui::DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let model_prompt = match crate::commands::init_turn::model_prompt(
            &self.info.runtime.cwd,
            agent.permission_mode(),
            |name| agent.has_tool(name),
        ) {
            Ok(model_prompt) => model_prompt,
            Err(error) => {
                self.insert_entry(&Entry::Error(error.to_string()));
                self.set_status("init unavailable");
                return Ok(());
            }
        };
        match self.skill_command_action(
            &format!("skill:{}", crate::commands::init_turn::SKILL),
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
                    "could not start init: built-in instructions are unavailable".into(),
                ));
                self.set_status("init unavailable");
            }
        }
        Ok(())
    }
}
