//! Slash commands `rho acp` advertises and runs.
//!
//! Hosts list `available_commands_update` entries in their command menus and
//! send a picked command back as `/name args` prompt text. Rho offers the
//! commands that work without the TUI: `/compact`, `/init`, prompt templates
//! (`/prompt:<name>`), MCP prompts (`/mcp:<server>:<prompt>`), workspace
//! workflows (`/workflow:<name> [inputs JSON]`, which start without a model
//! turn), fresh-session agent binding (`/agent:<name> <task>`), and `$skill`
//! mentions (see [`super::skills`]). Other prompts,
//! including unknown `/text`, reach the model unchanged.
//!
//! `/rho-wake <token>` runs a T3 wake watcher (see [`super::t3_wake`]). It is
//! never advertised: only the host delegates it.

use std::path::Path;

use agent_client_protocol::schema::v1::{
    AvailableCommand, AvailableCommandInput, UnstructuredCommandInput,
};
use rho_sdk::{
    model::{ContentBlock, ToolCall},
    UserInput,
};

use super::{
    skills,
    t3_wake::{WakeSignal, WAKE_COMMAND},
};
use crate::{
    agent::{AgentCatalog, AgentRuntimeSpec},
    app::workflow_cli::discover::{self as workflows, StartCommand},
    commands::{init_turn, CommandId, CommandInvocation, CommandParseError, COMMANDS},
    config::Config,
    permission::PermissionMode,
    prompt_templates,
    tools::{
        mcp::prompt_command::{self, McpPromptCommand},
        sdk_registry::AppToolSet,
        workflow as workflow_tool,
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
    /// A model turn on `input`, first running `calls` as one host batch.
    Turn {
        input: UserInput,
        calls: Vec<ToolCall>,
    },
    /// `/compact`, with optional guidance on what to keep.
    Compact { instructions: Option<String> },
    /// `/workflow:<name>`: start one workspace workflow in the background.
    StartWorkflow(StartCommand),
    /// `/agent:<name>`: bind a fresh session before running the task.
    RunAsAgent { agent_id: String, input: UserInput },
    /// `/rho-wake <token>`: wait for a parent's wake signal, without a model turn.
    AwaitWake(WakeSignal),
}

/// A recognized command that cannot run, such as a TUI-only command or an
/// MCP prompt missing arguments. The message is user-facing.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub(super) struct CommandRejected(pub(super) String);

/// Every command this session can run: built-ins, prompt templates, MCP
/// prompts, workflows when the session has the workflow tool, `$skill`
/// mentions when it has the skill tool, and Rho agent definitions.
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
    if scope.tools.contains(skills::SKILL_TOOL) {
        commands.extend(skills::available_commands(
            &crate::skills::discover_with_home(scope.cwd, scope.home),
        ));
    }
    if let Ok(catalog) = agent_catalog(scope) {
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

/// Resolves a leading `/command` in the prompt, then `$skill` mentions in
/// the model turn it starts.
pub(super) async fn resolve(
    input: UserInput,
    scope: &CommandScope<'_>,
) -> Result<PromptAction, CommandRejected> {
    let Some(text) = leading_text(&input) else {
        return Ok(turn(input, &[], scope));
    };
    if let Some(token) = text
        .strip_prefix(WAKE_COMMAND)
        .and_then(|rest| rest.starts_with(char::is_whitespace).then_some(rest))
    {
        return WakeSignal::parse(token)
            .map(PromptAction::AwaitWake)
            .ok_or_else(|| CommandRejected(format!("usage: {WAKE_COMMAND} <token>")));
    }
    // Delegated briefs span paragraphs, and the shared parser declines any
    // multiline text, so `/agent:` is matched here with the whole remainder
    // as its task.
    if let Some(rest) = text.strip_prefix("/agent:") {
        let (agent_id, task) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
        let (agent_id, task) = (agent_id.to_owned(), task.trim().to_owned());
        return run_as_agent(input, &agent_id, task, scope);
    }
    match crate::commands::parse_command(text) {
        Ok(None) => Ok(turn(input, &[], scope)),
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
                calls: vec![skills::load_call(init_turn::SKILL)],
            })
        }
        _ => Err(CommandRejected(format!(
            "could not run /{}: not available in ACP sessions",
            invocation.name
        ))),
    }
}

/// `/agent:<id> <task>`: run this fresh session as a Rho agent definition.
fn run_as_agent(
    input: UserInput,
    agent_id: &str,
    task: String,
    scope: &CommandScope<'_>,
) -> Result<PromptAction, CommandRejected> {
    if agent_id.is_empty() || task.is_empty() {
        return Err(CommandRejected("usage: /agent:<name> <task>".into()));
    }
    let rejected =
        |detail: String| CommandRejected(format!("could not run /agent:{agent_id}: {detail}"));
    let catalog = agent_catalog(scope).map_err(|error| rejected(error.to_string()))?;
    let definition = &catalog
        .find(agent_id)
        .map_err(|error| rejected(error.to_string()))?
        .definition;
    match &definition.runtime {
        AgentRuntimeSpec::Rho { .. } => {}
        AgentRuntimeSpec::ClaudeCli(_)
        | AgentRuntimeSpec::Cursor(_)
        | AgentRuntimeSpec::Antigravity(_) => {
            return Err(rejected(format!(
                "{agent_id} runs on {}, which an ACP session cannot host",
                definition.runtime.runtime()
            )));
        }
    }
    Ok(PromptAction::RunAsAgent {
        agent_id: agent_id.to_owned(),
        input: replace_leading_text(input, task),
    })
}

async fn dynamic(
    input: UserInput,
    name: &str,
    scope: &CommandScope<'_>,
) -> Result<PromptAction, CommandRejected> {
    // Spec-following clients send a picked `$skill` command as `/$skill`. It
    // loads first, then any skills mentioned after it.
    if let Some(skill) = name.strip_prefix('$') {
        if scope.tools.contains(skills::SKILL_TOOL)
            && crate::skills::discover_with_home(scope.cwd, scope.home)
                .iter()
                .any(|known| known.name == skill)
        {
            return Ok(turn(input, &[skill], scope));
        }
    }
    let trailing = leading_text(&input).map_or("", command_args);
    if let Some(resolved) = workflows::resolve_start_command(scope.cwd, name, trailing) {
        let rejected = |detail: &str| CommandRejected(format!("could not run /{name}: {detail}"));
        if !scope.tools.contains(workflow_tool::NAME) {
            return Err(rejected("this session has no workflow tool"));
        }
        // Workflow nodes write and run commands, which plan mode forbids.
        if scope.permission_mode == PermissionMode::Plan {
            return Err(rejected("workflows cannot start in plan mode"));
        }
        return resolved
            .map(PromptAction::StartWorkflow)
            .map_err(|detail| rejected(&detail));
    }
    if let Some(template) =
        prompt_templates::command_template_name(name).and_then(|template_name| {
            prompt_templates::find(&templates(scope), template_name).map(str::to_owned)
        })
    {
        let expanded = prompt_templates::expand(&template, trailing);
        return Ok(turn(replace_leading_text(input, expanded), &[], scope));
    }
    let catalog = scope.tools.mcp_catalog();
    let Some(prompt) = prompt_command::find(catalog, name) else {
        return Ok(turn(input, &[], scope));
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
        McpPromptCommand::Expanded(text) => Ok(turn(replace_leading_text(input, text), &[], scope)),
    }
}

/// A model turn that first loads the `leading` skills and every mentioned
/// `$skill`, all in one host batch.
fn turn(input: UserInput, leading: &[&str], scope: &CommandScope<'_>) -> PromptAction {
    let calls = if scope.tools.contains(skills::SKILL_TOOL) {
        skills::load_calls(leading, &input, || {
            crate::skills::discover_with_home(scope.cwd, scope.home)
        })
    } else {
        Vec::new()
    };
    PromptAction::Turn { input, calls }
}

fn templates(scope: &CommandScope<'_>) -> prompt_templates::PromptTemplates {
    prompt_templates::load(scope.cwd, scope.home, scope.config.prompt_templates.clone())
}

fn agent_catalog(
    scope: &CommandScope<'_>,
) -> Result<AgentCatalog, crate::agent::AgentCatalogError> {
    AgentCatalog::discover_with_home_and_trust(
        scope.cwd,
        scope.home,
        crate::workspace::ProjectTrust::from_agents_env(),
        &crate::plugins::agent_tiers(scope.cwd, scope.home),
    )
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
