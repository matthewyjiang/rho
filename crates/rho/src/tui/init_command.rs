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
        if agent.permission_mode() == crate::permission::PermissionMode::Plan {
            self.insert_entry(&Entry::Error(
                "could not start init: file writes are denied in plan permission mode".into(),
            ));
            self.set_status("init unavailable");
            return Ok(());
        }
        let mut missing_tools = ["skill", "write"]
            .into_iter()
            .filter(|name| !agent.has_tool(name))
            .collect::<Vec<_>>();
        if !rho_tools::EditFormat::ALL
            .iter()
            .any(|format| agent.has_tool(format.tool_name()))
        {
            missing_tools.push("file-edit tool");
        }
        if !missing_tools.is_empty() {
            self.insert_entry(&Entry::Error(format!(
                "could not start init: active agent is missing required tools: {}",
                missing_tools.join(", ")
            )));
            self.set_status("init unavailable");
            return Ok(());
        }

        // Use the same git-root-to-cwd boundary as AGENTS.md discovery.
        let cwd = &self.info.runtime.cwd;
        let root = crate::workspace::project_ancestor_dirs(cwd)
            .into_iter()
            .next()
            .unwrap_or_else(|| cwd.to_path_buf());
        let target = root.join("AGENTS.md");
        let exists = match target.try_exists() {
            Ok(exists) => exists,
            Err(error) => {
                self.insert_entry(&Entry::Error(format!(
                    "could not start init: could not inspect {}: {error}",
                    target.display()
                )));
                self.set_status("init unavailable");
                return Ok(());
            }
        };
        let model_prompt = format!(
            "Survey the repository and create or update its project instructions using the rho-init skill.\nTarget path: {}\nTarget exists: {exists}",
            crate::paths::prompt_data(&target),
        );
        match self.skill_command_action(
            "skill:rho-init",
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
