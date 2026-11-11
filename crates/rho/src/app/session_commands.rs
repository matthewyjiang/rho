//! Prompt commands, resolved the same way by every host of a
//! [`SessionRuntime`](super::session_runtime::SessionRuntime).
//!
//! A host passes a prompt here before running it, and runs the
//! [`PromptCommand`] it gets with the runtime's cores. A leading command can
//! be a built-in the host's [`CommandSet`] runs (`/compact`, `/init`), a
//! prompt template (`/prompt:<name>`), an MCP prompt
//! (`/mcp:<server>:<prompt>`), a workspace workflow
//! (`/workflow:<name> [inputs JSON]`, which starts without a model turn), or
//! fresh-session agent binding (`/agent:<name> <task>` or
//! `/agent:<name> --model <reference> -- <task>`). Every model turn first
//! loads the `$skill`s its prompt mentions. Other prompts, including unknown
//! `/text`, reach the model unchanged.
//!
//! What the commands are shown as is the host's: ACP advertises them as
//! `available_commands_update` entries. A host with commands of its own,
//! such as ACP's `/rho-wake`, matches them before calling [`resolve`].

use std::path::Path;

use rho_sdk::{
    model::{ContentBlock, ToolCall},
    UserInput,
};

use crate::{
    agent::{AgentCatalog, AgentCatalogError, AgentRuntimeSpec},
    app::{
        session_runtime::{AgentTask, CommandRejected, SessionRuntime, SessionTools},
        workflow_cli::discover::{self as workflows, WorkflowLaunch},
    },
    commands::{create_agent_turn, init_turn, CommandId, CommandInvocation, CommandParseError},
    config::Config,
    permission::PermissionMode,
    prompt_templates,
    skills::{mention, Skill},
    tools::{
        mcp::prompt_command::{self, McpPromptCommand},
        workflow as workflow_tool,
    },
};

/// A built-in command the shared resolver can run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BuiltInCommand {
    /// `/compact [instructions]`: compacts the session's history.
    Compact,
    /// `/init`: a model turn that sets up the workspace's agent instructions.
    Init,
    /// `/create-agent [request]` and `/agents create [request]`: a model
    /// turn that guides the user to a new agent definition.
    CreateAgent,
}

impl BuiltInCommand {
    fn id(self) -> CommandId {
        match self {
            Self::Compact => CommandId::Compact,
            Self::Init => CommandId::Init,
            Self::CreateAgent => CommandId::CreateAgent,
        }
    }
}

/// The commands one kind of host runs. Every host runs prompt templates,
/// MCP prompts, and `$skill` mentions; built-ins it does not list, workflows
/// and `/agent:` when it does not run them, and the TUI's own commands are
/// rejected. A host's menu (`session_options`) lists what this set runs.
#[derive(Clone, Copy, Debug)]
pub(crate) struct CommandSet {
    /// The built-ins this host runs.
    pub(crate) built_ins: &'static [BuiltInCommand],
    /// `/workflow:<name>`, which starts a run that outlives the prompt.
    pub(crate) workflows: Hosting,
    /// `/agent:<name>`, which rebinds a fresh session to an agent definition.
    pub(crate) agents: Hosting,
    /// The host's sessions as rejections name them, such as `ACP sessions`.
    pub(crate) sessions_label: &'static str,
}

/// Whether a host runs one kind of command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Hosting {
    Runs,
    /// Rejected before the prompt is saved, and left out of the host's menu.
    Refused,
}

impl CommandSet {
    /// The built-in `id` names, when this set runs it.
    pub(crate) fn built_in(self, id: CommandId) -> Option<BuiltInCommand> {
        self.built_ins
            .iter()
            .copied()
            .find(|built_in| built_in.id() == id)
    }
}

/// What commands can see of one session. `home` roots user skills and
/// prompt templates. `permission_mode` is the session's current mode, which
/// a host can change from the process default. `project_trust` is the
/// session's, under which project agents and project plugins' skills are
/// found.
pub(crate) struct CommandScope<'a> {
    pub(crate) cwd: &'a Path,
    pub(crate) home: Option<&'a Path>,
    pub(crate) project_trust: crate::workspace::ProjectTrusts,
    pub(crate) tools: SessionTools<'a>,
    pub(crate) config: &'a Config,
    pub(crate) permission_mode: PermissionMode,
}

impl<'a> CommandScope<'a> {
    /// What commands can see of `runtime`'s session, with the process
    /// config they read.
    pub(crate) fn of(
        runtime: &'a SessionRuntime,
        home: Option<&'a Path>,
        process_config: &'a Config,
    ) -> Self {
        Self {
            cwd: runtime.cwd(),
            home,
            project_trust: runtime.project_trust(),
            tools: runtime.tools(),
            config: process_config,
            permission_mode: runtime.permission_mode(),
        }
    }

    /// The prompt templates `/prompt:<name>` expands.
    pub(crate) fn templates(&self) -> prompt_templates::PromptTemplates {
        prompt_templates::load(self.cwd, self.home, self.config.prompt_templates.clone())
    }

    /// The agent definitions `/agent:<name>` binds.
    pub(crate) fn agent_catalog(&self) -> Result<AgentCatalog, AgentCatalogError> {
        AgentCatalog::discover_with_home_and_trust(
            self.cwd,
            self.home,
            self.project_trust.agents,
            &crate::plugins::agent_tiers(self.cwd, self.home, self.project_trust),
        )
    }

    /// The skills a prompt can mention.
    pub(crate) fn skills(&self) -> Vec<Skill> {
        crate::skills::discover_with_home(self.cwd, self.home, self.project_trust)
    }
}

/// What a prompt asks the session to do.
#[derive(Debug, PartialEq)]
pub(crate) enum PromptCommand {
    /// A model turn on `input`, first running `calls` as one host batch.
    Turn {
        input: UserInput,
        calls: Vec<ToolCall>,
    },
    /// `/compact`, with optional guidance on what to keep.
    Compact { instructions: Option<String> },
    /// `/workflow:<name>`, `/workflow:plan/<id>`, `/workflow:run/<id>`:
    /// start a workflow run in the background.
    StartWorkflow(WorkflowLaunch),
    /// `/agent:<name>`: bind a fresh session before running the task.
    RunAsAgent(AgentTask),
}

/// Resolves a leading `/command` in the prompt, among those `commands` and
/// every host run, then `$skill` mentions in the model turn it starts.
pub(crate) async fn resolve(
    input: UserInput,
    scope: &CommandScope<'_>,
    commands: CommandSet,
) -> Result<PromptCommand, CommandRejected> {
    let Some(text) = leading_text(&input) else {
        return Ok(turn(input, &[], scope));
    };
    // Delegated briefs span paragraphs, and the shared parser declines any
    // multiline text, so `/agent:` is matched here with the whole remainder
    // as its task.
    if let Some(rest) = text.strip_prefix("/agent:") {
        let (agent_id, task) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
        let agent_id = agent_id.to_owned();
        let (model_override, task) = agent_task(task)?;
        return run_as_agent(input, &agent_id, model_override, task, scope, commands);
    }
    let parsed = match crate::commands::parse_command(text) {
        Ok(None) => first_line_command(text, commands),
        parsed => parsed,
    };
    match parsed {
        Ok(None) => Ok(turn(input, &[], scope)),
        Ok(Some(invocation)) => built_in(input, invocation, scope, commands),
        Err(CommandParseError::Unknown(name)) => dynamic(input, &name, scope, commands).await,
    }
}

/// The command on a multiline prompt's first line, for the commands whose
/// arguments a pasted body can span: the built-ins `commands` runs and
/// `/$skill`. The shared parser declines multiline text, and any other
/// command there stays prose.
fn first_line_command(
    text: &str,
    commands: CommandSet,
) -> Result<Option<CommandInvocation>, CommandParseError> {
    let Some((first_line, _)) = text.split_once('\n') else {
        return Ok(None);
    };
    match crate::commands::parse_command(first_line) {
        Ok(Some(invocation)) if commands.built_in(invocation.id).is_some() => Ok(Some(invocation)),
        Err(CommandParseError::Unknown(name)) if name.starts_with('$') => {
            Err(CommandParseError::Unknown(name))
        }
        Ok(_) | Err(_) => Ok(None),
    }
}

fn built_in(
    input: UserInput,
    invocation: CommandInvocation,
    scope: &CommandScope<'_>,
    commands: CommandSet,
) -> Result<PromptCommand, CommandRejected> {
    // The whole text after the command, as a pasted body spans lines the
    // invocation's arguments stop short of.
    let args = leading_text(&input).map_or("", command_args);
    match commands.built_in(invocation.id) {
        Some(BuiltInCommand::Compact) => Ok(PromptCommand::Compact {
            instructions: (!args.is_empty()).then(|| args.to_owned()),
        }),
        Some(BuiltInCommand::Init) => {
            let model_prompt = init_turn::model_prompt(scope.cwd, scope.permission_mode, |name| {
                scope.tools.contains(name)
            })
            .map_err(|error| CommandRejected(error.to_string()))?;
            Ok(PromptCommand::Turn {
                input: replace_leading_text(input, model_prompt),
                calls: vec![mention::load_call(init_turn::SKILL)],
            })
        }
        Some(BuiltInCommand::CreateAgent) => {
            let request = crate::commands::create_agent_request(&invocation.name, args);
            let model_prompt =
                create_agent_turn::model_prompt(request, |name| scope.tools.contains(name))
                    .map_err(|error| CommandRejected(error.to_string()))?;
            Ok(PromptCommand::Turn {
                input: replace_leading_text(input, model_prompt),
                calls: vec![mention::load_call(create_agent_turn::SKILL)],
            })
        }
        None => Err(CommandRejected(format!(
            "could not run /{}: not available in {}",
            invocation.name, commands.sessions_label
        ))),
    }
}

/// The override form requires `--` so task text is never parsed as options.
/// Consume only its separating whitespace, preserving the forwarded task verbatim.
fn agent_task(args: &str) -> Result<(Option<String>, String), CommandRejected> {
    let args = args.trim_start();
    let (option, rest) = args.split_once(char::is_whitespace).unwrap_or((args, ""));
    if option == "--" {
        return Ok((None, rest.to_owned()));
    }
    if option != "--model" && !option.starts_with("--model=") {
        return Ok((None, args.trim().to_owned()));
    }
    let usage = || CommandRejected("usage: /agent:<name> --model <reference> -- <task>".into());
    if option != "--model" {
        return Err(usage());
    }
    let (model, rest) = rest
        .trim_start()
        .split_once(char::is_whitespace)
        .ok_or_else(usage)?;
    if model.is_empty() || model == "--" {
        return Err(usage());
    }
    let (delimiter, task) = rest
        .trim_start()
        .split_once(char::is_whitespace)
        .ok_or_else(usage)?;
    if delimiter != "--" || task.trim().is_empty() {
        return Err(usage());
    }
    Ok((Some(model.to_owned()), task.to_owned()))
}

/// Run this fresh session as a Rho agent definition, optionally changing its model.
fn run_as_agent(
    input: UserInput,
    agent_id: &str,
    model_override: Option<String>,
    task: String,
    scope: &CommandScope<'_>,
    commands: CommandSet,
) -> Result<PromptCommand, CommandRejected> {
    if agent_id.is_empty() || task.is_empty() {
        return Err(CommandRejected("usage: /agent:<name> <task>".into()));
    }
    let rejected =
        |detail: String| CommandRejected(format!("could not run /agent:{agent_id}: {detail}"));
    match commands.agents {
        Hosting::Runs => {}
        Hosting::Refused => {
            return Err(rejected(format!(
                "not available in {}",
                commands.sessions_label
            )))
        }
    }
    let catalog = scope
        .agent_catalog()
        .map_err(|error| rejected(error.to_string()))?;
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
                "{agent_id} runs on {}, which {} cannot host",
                definition.runtime.runtime(),
                commands.sessions_label
            )));
        }
    }
    Ok(PromptCommand::RunAsAgent(AgentTask {
        agent_id: agent_id.to_owned(),
        model_override,
        input: replace_leading_text(input, task),
    }))
}

async fn dynamic(
    input: UserInput,
    name: &str,
    scope: &CommandScope<'_>,
    commands: CommandSet,
) -> Result<PromptCommand, CommandRejected> {
    // Spec-following clients send a picked `$skill` command as `/$skill`. It
    // loads first, then any skills mentioned after it.
    if let Some(skill) = name.strip_prefix('$') {
        if !scope.tools.contains(mention::SKILL_TOOL) {
            return Err(CommandRejected(mention::NO_SKILL_TOOL.into()));
        }
        if scope.skills().iter().any(|known| known.name == skill) {
            return Ok(turn(input, &[skill], scope));
        }
    }
    let trailing = leading_text(&input).map_or("", command_args);
    if let Some(resolved) = workflows::resolve_launch(scope.cwd, name, trailing) {
        let rejected = |detail: &str| CommandRejected(format!("could not run /{name}: {detail}"));
        match commands.workflows {
            Hosting::Runs => {}
            Hosting::Refused => {
                return Err(rejected(&format!(
                    "not available in {}; use rho workflow run",
                    commands.sessions_label
                )))
            }
        }
        if !scope.tools.contains(workflow_tool::NAME) {
            return Err(rejected("this session has no workflow tool"));
        }
        // Workflow nodes write and run commands, which plan mode forbids.
        if scope.permission_mode == PermissionMode::Plan {
            return Err(rejected("workflows cannot start in plan mode"));
        }
        return resolved
            .map(PromptCommand::StartWorkflow)
            .map_err(|detail| rejected(&detail));
    }
    if let Some(template) =
        prompt_templates::command_template_name(name).and_then(|template_name| {
            prompt_templates::find(&scope.templates(), template_name).map(str::to_owned)
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
fn turn(input: UserInput, leading: &[&str], scope: &CommandScope<'_>) -> PromptCommand {
    let calls = if scope.tools.contains(mention::SKILL_TOOL) {
        skill_calls(leading, &input, || scope.skills())
    } else {
        Vec::new()
    };
    PromptCommand::Turn { input, calls }
}

/// Host calls that load the `leading` skills, then every known skill the
/// prompt's text blocks mention, each skill once and in that order.
fn skill_calls(
    leading: &[&str],
    input: &UserInput,
    discover: impl FnOnce() -> Vec<Skill>,
) -> Vec<ToolCall> {
    let texts = input
        .blocks()
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.as_str()),
            ContentBlock::Image(_) | ContentBlock::ToolCall(_) => None,
        })
        .collect::<Vec<_>>();
    mention::skills_to_load(leading, &texts, discover)
        .into_iter()
        .map(mention::load_call)
        .collect()
}

/// The text after the first whitespace-delimited token, such as a
/// command's arguments or a usage line's argument hint.
pub(crate) fn command_args(text: &str) -> &str {
    text.trim()
        .split_once(char::is_whitespace)
        .map_or("", |(_, args)| args.trim())
}

/// The prompt's leading text, the only place a command can be.
pub(crate) fn leading_text(input: &UserInput) -> Option<&str> {
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
#[path = "session_commands_test_support.rs"]
pub(in crate::app) mod test_support;

#[cfg(test)]
#[path = "session_commands_tests.rs"]
mod tests;
