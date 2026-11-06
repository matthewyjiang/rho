//! The commands `rho acp` advertises and runs.
//!
//! Hosts list `available_commands_update` entries in their command menus and
//! send a picked command back as `/name args` prompt text. ACP sessions run
//! every command of [`crate::app::session_commands`], which resolves them,
//! including both built-ins, `/compact` and `/init`; this module advertises
//! them, with `$skill` mentions as T3 Code reads them (see [`super::skills`]).
//!
//! `/rho-wake <token>` runs a T3 wake watcher (see [`super::t3_wake`]). It is
//! ACP's own command, matched before the shared ones, and never advertised:
//! only the host delegates it.

use agent_client_protocol::schema::v1::{
    AvailableCommand, AvailableCommandInput, UnstructuredCommandInput,
};
use rho_sdk::UserInput;

use super::{
    skills,
    t3_wake::{WakeSignal, WAKE_COMMAND},
};
use crate::{
    agent::AgentRuntimeSpec,
    app::{
        session_commands::{
            self, command_args, leading_text, BuiltInCommand, CommandScope, CommandSet,
            PromptCommand,
        },
        session_runtime::CommandRejected,
        workflow_cli::discover as workflows,
    },
    commands::COMMANDS,
    prompt_templates,
    skills::mention::SKILL_TOOL,
    tools::workflow as workflow_tool,
};

/// The shared commands ACP sessions run: every built-in.
const ACP_COMMANDS: CommandSet = CommandSet::new(
    &[BuiltInCommand::Compact, BuiltInCommand::Init],
    "ACP sessions",
);

/// What a prompt asks an ACP session to do.
#[derive(Debug)]
pub(super) enum AcpPrompt {
    /// A command every host runs, or a model turn.
    Shared(PromptCommand),
    /// `/rho-wake <token>`: wait for a parent's wake signal, without a model turn.
    AwaitWake(WakeSignal),
}

/// Every command this session can run: built-ins, prompt templates, MCP
/// prompts, workflows when the session has the workflow tool, `$skill`
/// mentions when it has the skill tool, and Rho agent definitions.
pub(super) fn available_commands(scope: &CommandScope<'_>) -> Vec<AvailableCommand> {
    let built_in = COMMANDS
        .iter()
        .filter(|spec| ACP_COMMANDS.built_in(spec.id).is_some())
        .map(|spec| command(spec.name.to_string(), spec.description, spec.usage));
    let templates = scope.templates().into_iter().map(|(name, template)| {
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
    if scope.tools.contains(workflow_tool::NAME) {
        commands.extend(
            workflows::command_workflows(scope.cwd)
                .into_iter()
                .filter_map(|workflow| {
                    Some(command(
                        workflow.command_name()?,
                        &workflow.command_description(),
                        &workflow.command_usage()?,
                    ))
                }),
        );
    }
    if scope.tools.contains(SKILL_TOOL) {
        commands.extend(skills::available_commands(&scope.skills()));
    }
    if let Ok(catalog) = scope.agent_catalog() {
        commands.extend(catalog.iter().filter_map(|entry| {
            let definition = &entry.definition;
            match &definition.runtime {
                AgentRuntimeSpec::Rho { .. } => Some(command(
                    format!("agent:{}", definition.id),
                    &definition.description,
                    &format!("/agent:{} <task>", definition.id),
                )),
                AgentRuntimeSpec::ClaudeCli(_)
                | AgentRuntimeSpec::Cursor(_)
                | AgentRuntimeSpec::Antigravity(_) => None,
            }
        }));
    }
    commands
}

/// Matches ACP's `/rho-wake`, then resolves the shared commands.
pub(super) async fn resolve(
    input: UserInput,
    scope: &CommandScope<'_>,
) -> Result<AcpPrompt, CommandRejected> {
    if let Some(token) = leading_text(&input)
        .and_then(|text| text.strip_prefix(WAKE_COMMAND))
        .and_then(|rest| rest.starts_with(char::is_whitespace).then_some(rest))
    {
        return WakeSignal::parse(token)
            .map(AcpPrompt::AwaitWake)
            .ok_or_else(|| CommandRejected(format!("usage: {WAKE_COMMAND} <token>")));
    }
    session_commands::resolve(input, scope, ACP_COMMANDS)
        .await
        .map(AcpPrompt::Shared)
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

#[cfg(test)]
#[path = "commands_tests.rs"]
mod tests;
