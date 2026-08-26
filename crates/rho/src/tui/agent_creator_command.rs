use super::{skill_actions::SkillCommandAction, App, ChatMedia, Entry, InteractiveRuntime};

impl App {
    pub(super) async fn execute_create_agent_command(
        &mut self,
        request: &str,
        display: String,
        media: Vec<ChatMedia>,
        terminal: &mut ratatui::DefaultTerminal,
        agent: &mut InteractiveRuntime,
    ) -> anyhow::Result<()> {
        let mut missing_tools = ["skill", "questionnaire", "read_file", "write"]
            .into_iter()
            .filter(|name| !agent.has_tool(name))
            .collect::<Vec<_>>();
        if !["bash", "powershell", "shell"]
            .into_iter()
            .any(|name| agent.has_tool(name))
        {
            missing_tools.push("bash, powershell, or shell");
        }
        if !missing_tools.is_empty() {
            self.insert_entry(&Entry::Error(format!(
                "could not start agent creator: active agent is missing required tools: {}",
                missing_tools.join(", ")
            )));
            self.set_status("agent creator unavailable");
            return Ok(());
        }

        let model_prompt = if request.is_empty() {
            "Create a new Rho agent through the guided workflow.".to_string()
        } else {
            format!("Create a new Rho agent through the guided workflow. User request: {request}")
        };
        match self.skill_command_action(
            "skill:rho-agent-creator",
            model_prompt,
            display,
            agent.has_tool("skill"),
        )? {
            SkillCommandAction::Prompt(prompt) => {
                self.run_prompt_turn(*prompt, media, terminal, agent)
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
