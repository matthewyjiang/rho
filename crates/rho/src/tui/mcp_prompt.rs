//! Expanding an MCP server prompt into the turn it starts.
//!
//! A prompt is picked from the command palette like a skill, but unlike a skill
//! it cannot be expanded while completing the token: `prompts/get` is a
//! round-trip to the server. So the palette only completes `/mcp:<server>:<name>`
//! and the expansion happens here, on submit, where a turn has not started yet
//! and there is somewhere to report a failure.

use super::{App, InteractiveRuntime, TurnPrompt};

/// The prefix that marks a slash command as an MCP server prompt.
const MCP_PROMPT_PREFIX: &str = "mcp:";

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
        let Some((server, name)) = parse_command(command) else {
            return Ok(None);
        };
        let catalog = agent.mcp_catalog().clone();
        let Some(prompt) = catalog
            .prompts()
            .into_iter()
            .find(|prompt| prompt.server == server && prompt.name == name)
        else {
            return Ok(None);
        };

        // Prompt text is bounded by the same cap as tool output, so one very
        // large prompt cannot swamp the turn it starts.
        let max_output_bytes = self.info.services.config_repository.load()?.max_output_bytes;
        let arguments = prompt.parse_arguments(trailing);
        let missing = prompt.missing_arguments(&arguments);
        if !missing.is_empty() {
            self.set_status(format!(
                "{} needs {}",
                prompt.command_name(),
                missing.join(", ")
            ));
            return Ok(None);
        }

        match catalog
            .get_prompt(&server, &name, arguments, max_output_bytes)
            .await
        {
            Ok(expansion) => {
                // The server's own description of the prompt leads, so the
                // model knows what the following text is for.
                let model = match expansion.description {
                    Some(description) if !description.trim().is_empty() => {
                        format!("{description}\n\n{}", expansion.text)
                    }
                    _ => expansion.text,
                };
                Ok(Some(TurnPrompt::command(model, display.to_string())))
            }
            Err(error) => {
                self.set_status(format!("{} failed: {error}", prompt.command_name()));
                Ok(None)
            }
        }
    }
}

/// Split `mcp:<server>:<prompt>` into its parts. Server identities cannot
/// contain `:`, so the first separator after the prefix is unambiguous.
fn parse_command(command: &str) -> Option<(String, String)> {
    let rest = command
        .get(..MCP_PROMPT_PREFIX.len())
        .filter(|prefix| prefix.eq_ignore_ascii_case(MCP_PROMPT_PREFIX))
        .and_then(|_| command.get(MCP_PROMPT_PREFIX.len()..))?;
    let (server, prompt) = rest.split_once(':')?;
    if server.is_empty() || prompt.is_empty() {
        return None;
    }
    Some((server.to_string(), prompt.to_string()))
}

#[cfg(test)]
#[path = "mcp_prompt_tests.rs"]
mod tests;
