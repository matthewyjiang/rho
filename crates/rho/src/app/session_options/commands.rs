//! The commands a session's prompt may run, as a host lists them in its
//! command menu: the built-ins the host runs, prompt templates, MCP
//! prompts, workflows, `$skill` mentions, and Rho agent definitions.
//! [`crate::app::session_commands`] resolves a picked one when the prompt
//! arrives; this module only lists them.

use rho_protocol as wire;

use crate::{
    agent::AgentRuntimeSpec,
    app::{
        session_commands::{command_args, CommandScope, CommandSet, Hosting},
        workflow_cli::discover as workflows,
    },
    commands::COMMANDS,
    prompt_templates,
    skills::{
        mention::{self, SKILL_TOOL},
        Skill,
    },
    tools::workflow as workflow_tool,
};

/// Every command `scope`'s session can run that `commands` runs, in menu
/// order: built-ins, prompt templates, MCP prompts, workflows when the
/// session has the workflow tool, skills when it has the skill tool, and
/// Rho agent definitions. A skill is `$<name>` anywhere in a prompt, which
/// the shared resolver loads before the first model request;
/// [`crate::skills::mention`] owns the mention rule shared with the TUI.
pub(crate) fn available_commands(
    scope: &CommandScope<'_>,
    commands: CommandSet,
) -> Vec<wire::SessionCommand> {
    let built_in = COMMANDS
        .iter()
        .filter(|spec| commands.built_in(spec.id).is_some())
        .map(|spec| slash(spec.name.to_string(), spec.description, spec.usage));
    let templates = scope.templates().into_iter().map(|(name, template)| {
        let name = format!("prompt:{name}");
        let usage = prompt_templates::usage(&name, &template);
        slash(name, &prompt_templates::description(&template), &usage)
    });
    let mcp_prompts = scope
        .tools
        .mcp_catalog()
        .prompts()
        .into_iter()
        .map(|prompt| {
            let description = prompt.description.as_deref().unwrap_or(prompt.label());
            slash(prompt.command_name(), description, &prompt.usage())
        });
    let mut listed = built_in
        .chain(templates)
        .chain(mcp_prompts)
        .collect::<Vec<_>>();
    match commands.workflows {
        Hosting::Runs if scope.tools.contains(workflow_tool::NAME) => listed.extend(
            workflows::command_workflows(scope.cwd)
                .into_iter()
                .filter_map(|workflow| {
                    Some(slash(
                        workflow.command_name()?,
                        &workflow.command_description(),
                        &workflow.command_usage()?,
                    ))
                }),
        ),
        Hosting::Runs | Hosting::Refused => {}
    }
    if scope.tools.contains(SKILL_TOOL) {
        listed.extend(skill_commands(&scope.skills()));
    }
    match commands.agents {
        Hosting::Runs => {
            if let Ok(catalog) = scope.agent_catalog() {
                listed.extend(catalog.iter().filter_map(|entry| {
                    let definition = &entry.definition;
                    match &definition.runtime {
                        AgentRuntimeSpec::Rho { .. } => Some(slash(
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
        }
        Hosting::Refused => {}
    }
    listed
}

/// A skill command for every skill a composer can mention: T3 Code, for
/// one, reads a `$` before digits as an amount.
fn skill_commands(skills: &[Skill]) -> Vec<wire::SessionCommand> {
    skills
        .iter()
        .filter(|skill| mention::mentionable(&skill.name))
        .map(|skill| wire::SessionCommand {
            name: skill.name.clone(),
            kind: wire::CommandKind::Skill,
            description: skill.description.clone(),
            input_hint: None,
        })
        .collect()
}

/// A slash command. `usage` is `/name args`; the args become the hint.
fn slash(name: String, description: &str, usage: &str) -> wire::SessionCommand {
    let hint = command_args(usage);
    wire::SessionCommand {
        name,
        kind: wire::CommandKind::Slash,
        description: description.to_owned(),
        input_hint: (!hint.is_empty()).then(|| hint.to_owned()),
    }
}

#[cfg(test)]
#[path = "commands_tests.rs"]
mod tests;
