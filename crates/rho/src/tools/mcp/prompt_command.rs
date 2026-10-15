//! `/mcp:<server>:<prompt>` commands, shared by the TUI and ACP hosts.
//!
//! A host only completes the command token from the cached catalog. Expansion
//! is a `prompts/get` round-trip, so it happens on submit, where the host can
//! still report a failure instead of starting a turn.

use super::{catalog::McpPrompt, McpCatalog, McpCatalogError};

/// The prefix that marks a slash command as an MCP server prompt.
const MCP_PROMPT_PREFIX: &str = "mcp:";

/// Result of expanding an MCP prompt command.
#[derive(Debug, PartialEq)]
pub(crate) enum McpPromptCommand {
    /// The prompt needs arguments the trailing text did not supply.
    MissingArguments(Vec<String>),
    /// The server could not expand the prompt.
    Failed(McpCatalogError),
    /// Model-facing turn text.
    Expanded(String),
}

/// The catalogued prompt `command` (the slash token without `/`) names.
/// Hosts check this before loading anything expansion needs.
pub(crate) fn find(catalog: &McpCatalog, command: &str) -> Option<McpPrompt> {
    let (server, name) = parse_command(command)?;
    catalog
        .prompts()
        .into_iter()
        .find(|prompt| prompt.server == server && prompt.name == name)
}

/// Expands `prompt` with the text typed after its command. Prompt text
/// shares the tool-output cap, so one very large prompt cannot swamp the
/// turn it starts.
pub(crate) async fn expand(
    catalog: &McpCatalog,
    prompt: &McpPrompt,
    trailing: &str,
    max_output_bytes: usize,
) -> McpPromptCommand {
    let arguments = prompt.parse_arguments(trailing);
    let missing = prompt.missing_arguments(&arguments);
    if !missing.is_empty() {
        return McpPromptCommand::MissingArguments(
            missing.into_iter().map(str::to_owned).collect(),
        );
    }
    match catalog
        .get_prompt(&prompt.server, &prompt.name, arguments, max_output_bytes)
        .await
    {
        Ok(expansion) => McpPromptCommand::Expanded(prompt_turn_text(
            expansion.description.as_deref(),
            &expansion.text,
            max_output_bytes,
        )),
        Err(error) => McpPromptCommand::Failed(error),
    }
}

/// Split `mcp:<server>:<prompt>` into its parts. Server identities cannot
/// contain `:`, so the first separator after the prefix is unambiguous.
pub(crate) fn parse_command(command: &str) -> Option<(String, String)> {
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

/// Build the model-facing turn text from a prompt expansion.
///
/// The server's description leads when present so the model knows what the
/// following text is for. Description and body share one `max_output_bytes`
/// budget; the message body alone is already capped upstream.
fn prompt_turn_text(description: Option<&str>, body: &str, max_output_bytes: usize) -> String {
    match description {
        Some(description) if !description.trim().is_empty() => {
            rho_tools::tool::truncate(format!("{description}\n\n{body}"), max_output_bytes)
        }
        _ => body.to_string(),
    }
}

#[cfg(test)]
#[path = "prompt_command_tests.rs"]
mod tests;
