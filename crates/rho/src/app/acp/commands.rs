//! Slash commands `rho acp` advertises and runs.
//!
//! Hosts list `available_commands_update` entries in their command menus and
//! send a picked command back as `/name args` prompt text. Rho offers the
//! commands that work without the TUI: `/compact`, `/init`, prompt templates
//! (`/prompt:<name>`), MCP prompts (`/mcp:<server>:<prompt>`), and `$skill`
//! mentions (see [`super::skills`]). Other prompts, including unknown
//! `/text`, reach the model unchanged.

use std::path::Path;

use agent_client_protocol::schema::v1::{
    AvailableCommand, AvailableCommandInput, UnstructuredCommandInput,
};
use rho_sdk::{
    model::{ContentBlock, ToolCall},
    UserInput,
};

use super::skills;
use crate::{
    commands::{init_turn, CommandId, CommandInvocation, CommandParseError, COMMANDS},
    config::Config,
    permission::PermissionMode,
    prompt_templates,
    tools::{
        mcp::prompt_command::{self, McpPromptCommand},
        sdk_registry::AppToolSet,
    },
};

/// Built-in commands that run without the TUI, in advertised order.
const BUILT_IN: [CommandId; 2] = [CommandId::Compact, CommandId::Init];

/// What commands can see of one ACP session. `home` roots user skills and
/// prompt templates. `permission_mode` is the session's current mode, which
/// `session/set_mode` can change from the process default.
pub(super) struct CommandScope<'a> {
    pub(super) cwd: &'a Path,
    pub(super) home: Option<&'a Path>,
    pub(super) tools: &'a AppToolSet,
    pub(super) config: &'a Config,
    pub(super) permission_mode: PermissionMode,
}

/// What a prompt asks the session to do.
#[derive(Debug, PartialEq)]
pub(super) enum PromptAction {
    /// A model turn on `input`, first running `call` when it is set.
    Turn {
        input: UserInput,
        call: Option<ToolCall>,
    },
    /// `/compact`, with optional guidance on what to keep.
    Compact { instructions: Option<String> },
}

/// A recognized command that cannot run, such as a TUI-only command or an
/// MCP prompt missing arguments. The message is user-facing.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub(super) struct CommandRejected(String);

/// Every command this session can run: built-ins, prompt templates, MCP
/// prompts, then `$skill` mentions when the session has the skill tool.
pub(super) fn available_commands(scope: &CommandScope<'_>) -> Vec<AvailableCommand> {
    let built_in = COMMANDS
        .iter()
        .filter(|spec| BUILT_IN.contains(&spec.id))
        .map(|spec| command(spec.name.to_string(), spec.description, spec.usage));
    let templates = templates(scope).into_iter().map(|(name, template)| {
        let name = format!("prompt:{name}");
        let usage = prompt_templates::usage(&name, &template);
        command(name, &prompt_templates::description(&template), &usage)
    });
    let mcp_prompts = scope
        .tools
        .mcp_catalog()
        .prompts()
        .into_iter()
        .map(|prompt| {
            let description = prompt.description.as_deref().unwrap_or(prompt.label());
            command(prompt.command_name(), description, &prompt.usage())
        });
    let mut commands = built_in
        .chain(templates)
        .chain(mcp_prompts)
        .collect::<Vec<_>>();
    if scope.tools.contains(skills::SKILL_TOOL) {
        commands.extend(skills::available_commands(
            &crate::skills::discover_with_home(scope.cwd, scope.home),
        ));
    }
    commands
}

/// Resolves a leading `/command` in the prompt, then `$skill` mentions in
/// the model turn it starts.
pub(super) async fn resolve(
    input: UserInput,
    scope: &CommandScope<'_>,
) -> Result<PromptAction, CommandRejected> {
    let Some(text) = leading_text(&input) else {
        return Ok(turn(input, scope));
    };
    match crate::commands::parse_command(text) {
        Ok(None) => Ok(turn(input, scope)),
        Ok(Some(invocation)) => built_in(input, invocation, scope),
        Err(CommandParseError::Unknown(name)) => dynamic(input, &name, scope).await,
    }
}

fn built_in(
    input: UserInput,
    invocation: CommandInvocation,
    scope: &CommandScope<'_>,
) -> Result<PromptAction, CommandRejected> {
    match invocation.id {
        CommandId::Compact => Ok(PromptAction::Compact {
            instructions: (!invocation.args.is_empty()).then_some(invocation.args),
        }),
        CommandId::Init => {
            let model_prompt = init_turn::model_prompt(scope.cwd, scope.permission_mode, |name| {
                scope.tools.contains(name)
            })
            .map_err(|error| CommandRejected(error.to_string()))?;
            Ok(PromptAction::Turn {
                input: replace_leading_text(input, model_prompt),
                call: Some(skills::load_call(init_turn::SKILL)),
            })
        }
        _ => Err(CommandRejected(format!(
            "could not run /{}: not available in ACP sessions",
            invocation.name
        ))),
    }
}

async fn dynamic(
    input: UserInput,
    name: &str,
    scope: &CommandScope<'_>,
) -> Result<PromptAction, CommandRejected> {
    let trailing = leading_text(&input).map_or("", command_args);
    if let Some(template) =
        prompt_templates::command_template_name(name).and_then(|template_name| {
            prompt_templates::find(&templates(scope), template_name).map(str::to_owned)
        })
    {
        let expanded = prompt_templates::expand(&template, trailing);
        return Ok(turn(replace_leading_text(input, expanded), scope));
    }
    let catalog = scope.tools.mcp_catalog();
    let Some(prompt) = prompt_command::find(catalog, name) else {
        return Ok(turn(input, scope));
    };
    let rejected = |detail: String| {
        CommandRejected(format!(
            "could not run /{}: {detail}",
            prompt.command_name()
        ))
    };
    match prompt_command::expand(catalog, &prompt, trailing, scope.config.max_output_bytes).await {
        McpPromptCommand::MissingArguments(missing) => Err(rejected(format!(
            "missing required arguments: {}",
            missing.join(", ")
        ))),
        McpPromptCommand::Failed(error) => Err(rejected(error.to_string())),
        McpPromptCommand::Expanded(text) => Ok(turn(replace_leading_text(input, text), scope)),
    }
}

/// A model turn that loads the first mentioned `$skill` up front.
fn turn(input: UserInput, scope: &CommandScope<'_>) -> PromptAction {
    let call = scope
        .tools
        .contains(skills::SKILL_TOOL)
        .then(|| {
            skills::first_mention(&input, || {
                crate::skills::discover_with_home(scope.cwd, scope.home)
            })
        })
        .flatten()
        .map(|skill| skills::load_call(&skill.name));
    PromptAction::Turn { input, call }
}

fn templates(scope: &CommandScope<'_>) -> prompt_templates::PromptTemplates {
    prompt_templates::load(scope.cwd, scope.home, scope.config.prompt_templates.clone())
}

/// An advertised command. `usage` is `/name args`; the args become the hint.
fn command(name: String, description: &str, usage: &str) -> AvailableCommand {
    let hint = command_args(usage);
    let command = AvailableCommand::new(name, description);
    if hint.is_empty() {
        command
    } else {
        command.input(AvailableCommandInput::Unstructured(
            UnstructuredCommandInput::new(hint),
        ))
    }
}

/// The text after the first whitespace-delimited token.
fn command_args(text: &str) -> &str {
    text.trim()
        .split_once(char::is_whitespace)
        .map_or("", |(_, args)| args.trim())
}

/// A command can only lead the prompt's first block.
fn leading_text(input: &UserInput) -> Option<&str> {
    match input.blocks().first()? {
        ContentBlock::Text(text) => Some(text),
        ContentBlock::Image(_) | ContentBlock::ToolCall(_) => None,
    }
}

/// Swaps the command text for its expansion, keeping attached blocks.
fn replace_leading_text(input: UserInput, text: String) -> UserInput {
    let mut blocks = input.blocks().to_vec();
    blocks[0] = ContentBlock::Text(text);
    UserInput::content(blocks).expect("replacing a text block keeps the input valid")
}

#[cfg(test)]
#[path = "commands_tests.rs"]
mod tests;
