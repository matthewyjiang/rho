//! Expanding an MCP server prompt into the turn it starts.
//!
//! A prompt is picked from the command palette like a skill, but unlike a skill
//! it cannot be expanded while completing the token: `prompts/get` is a
//! round-trip to the server. So the palette only completes `/mcp:<server>:<name>`
//! and the expansion happens here, on submit, where a turn has not started yet
//! and there is somewhere to report a failure.

use crate::tools::mcp::prompt_command::{self, McpPromptCommand};

use super::{App, InteractiveRuntime, TurnPrompt};

impl App {
    /// Expand `/mcp:<server>:<prompt>` into the turn it should start.
    ///
    /// Returns `Ok(None)` when the command is not an MCP prompt, so the caller
    /// can fall through to its other dynamic-command handlers. A prompt that
    /// exists but cannot be fetched reports the reason and yields `Ok(None)`
    /// with the composer already cleared, because starting a turn on a failed
    /// expansion would send the model a command it cannot act on.
    pub(super) async fn expand_mcp_prompt(
        &mut self,
        command: &str,
        trailing: &str,
        display: &str,
        agent: &InteractiveRuntime,
    ) -> anyhow::Result<Option<TurnPrompt>> {
        let catalog = agent.mcp_catalog();
        let Some(prompt) = prompt_command::find(catalog, command) else {
            return Ok(None);
        };
        let max_output_bytes = self
            .info
            .services
            .config_repository
            .load()?
            .max_output_bytes;
        match prompt_command::expand(catalog, &prompt, trailing, max_output_bytes).await {
            McpPromptCommand::MissingArguments(missing) => {
                self.set_status(format!(
                    "{} needs {}",
                    prompt.command_name(),
                    missing.join(", ")
                ));
                Ok(None)
            }
            McpPromptCommand::Failed(error) => {
                self.set_status(format!("{} failed: {error}", prompt.command_name()));
                Ok(None)
            }
            McpPromptCommand::Expanded(model) => {
                Ok(Some(TurnPrompt::command(model, display.to_string())))
            }
        }
    }
}
