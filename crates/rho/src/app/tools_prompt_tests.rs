use std::sync::Arc;

use pretty_assertions::assert_eq;
use rho_sdk::SystemPrompt;

use crate::{
    agent::{
        AgentDefinition, AgentId, AgentRuntimeSpec, ModelPolicy, PromptPolicy, ToolCapability,
        ToolPolicy, ADVISOR_AGENT_ID,
    },
    app::agent_binding::{AgentBinder, AgentInvocation, AgentRole},
    config::Config,
    diagnostics::RuntimeDiagnostics,
    tools::agent::BackgroundSubagents,
};

use super::{assemble_tools_and_prompt, ToolsAndPromptOptions};

fn advisor_config(advisor_mode: bool, with_model: bool) -> Config {
    let mut config = Config {
        advisor_mode,
        ..Config::default()
    };
    if with_model {
        config.set_internal_agent_model(
            ADVISOR_AGENT_ID,
            "anthropic".into(),
            "claude-test".into(),
            "api-key".into(),
        );
    }
    config
}

fn bound_agent(config: &Config) -> crate::app::agent_binding::BoundAgent {
    AgentBinder::bind(
        Arc::new(AgentDefinition {
            id: AgentId::new("test").unwrap(),
            description: "test".into(),
            prompt: PromptPolicy::Extend(String::new()),
            runtime: AgentRuntimeSpec::Rho {
                tools: ToolPolicy::Allow(
                    [ToolCapability::Advisor, ToolCapability::ReadFile]
                        .into_iter()
                        .collect(),
                ),
                model: ModelPolicy::Inherit,
                reasoning: None,
            },
        }),
        AgentInvocation {
            role: AgentRole::InteractiveRoot,
            available_tools: crate::agent::AgentCapabilities::all_host_tools(),
        },
        config,
    )
    .unwrap()
}

fn assemble(config: &Config, cwd: &std::path::Path) -> (bool, String) {
    let diagnostics = RuntimeDiagnostics::new(config);
    let agent = bound_agent(config);
    let (tools, prompt) = assemble_tools_and_prompt(ToolsAndPromptOptions {
        config,
        config_path: cwd.join("config.toml"),
        cwd,
        no_system_prompt: false,
        no_tools: false,
        no_subagents: true,
        questionnaire_enabled: false,
        background_subagents: BackgroundSubagents::Disabled,
        diagnostics: &diagnostics,
        agent: &agent,
    })
    .unwrap();
    let registered = tools.advisor_registered();
    let text = match prompt.for_advisor_mode(registered) {
        SystemPrompt::Custom(text) => text,
        SystemPrompt::None => String::new(),
        _ => String::new(),
    };
    (registered, text)
}

// Covers: the advisor tool must appear only when advisor mode is on and an
// advisor model is configured, and the steering text must track the tool.
// Owner: root tool/prompt assembly.
#[test]
fn the_advisor_tool_needs_both_the_mode_and_a_model() {
    let cwd = tempfile::tempdir().unwrap();
    let cases = [
        (false, false, false),
        (true, false, false),
        (false, true, false),
        (true, true, true),
    ];

    for (advisor_mode, with_model, expected) in cases {
        let config = advisor_config(advisor_mode, with_model);

        let (registered, prompt) = assemble(&config, cwd.path());

        assert_eq!(
            registered, expected,
            "advisor_mode={advisor_mode} with_model={with_model}"
        );
        assert_eq!(
            prompt.contains("You have access to an `advisor` tool"),
            expected,
            "steering text for advisor_mode={advisor_mode} with_model={with_model}"
        );
    }
}

// Covers: the advisor must review the prompt the executor actually runs with.
// Owner: root tool/prompt assembly.
#[test]
fn the_advisor_receives_the_executor_system_prompt() {
    let cwd = tempfile::tempdir().unwrap();
    let config = advisor_config(true, true);
    let diagnostics = RuntimeDiagnostics::new(&config);
    let agent = bound_agent(&config);

    let (tools, prompt) = assemble_tools_and_prompt(ToolsAndPromptOptions {
        config: &config,
        config_path: cwd.path().join("config.toml"),
        cwd: cwd.path(),
        no_system_prompt: false,
        no_tools: false,
        no_subagents: true,
        questionnaire_enabled: false,
        background_subagents: BackgroundSubagents::Disabled,
        diagnostics: &diagnostics,
        agent: &agent,
    })
    .unwrap();

    let SystemPrompt::Custom(text) = prompt.for_advisor_mode(true) else {
        panic!("expected a custom system prompt");
    };
    let store = tools.advisor().expect("advisor store");
    assert_eq!(store.system_prompt(), Some(text));
}

// Covers: both prompt forms are built once so a mid-session /advisor toggle can
// swap them, and the executor is never told about a tool it does not have.
// Owner: root tool/prompt assembly.
#[test]
fn both_prompt_variants_are_available_whatever_the_saved_mode_is() {
    let cwd = tempfile::tempdir().unwrap();
    let steering = "You have access to an `advisor` tool";

    for advisor_mode in [false, true] {
        let config = advisor_config(advisor_mode, /*with_model*/ true);
        let diagnostics = RuntimeDiagnostics::new(&config);
        let agent = bound_agent(&config);

        let (_, prompt) = assemble_tools_and_prompt(ToolsAndPromptOptions {
            config: &config,
            config_path: cwd.path().join("config.toml"),
            cwd: cwd.path(),
            no_system_prompt: false,
            no_tools: false,
            no_subagents: true,
            questionnaire_enabled: false,
            background_subagents: BackgroundSubagents::Disabled,
            diagnostics: &diagnostics,
            agent: &agent,
        })
        .unwrap();

        let text = |enabled| match prompt.for_advisor_mode(enabled) {
            SystemPrompt::Custom(text) => text,
            SystemPrompt::None => String::new(),
            _ => String::new(),
        };

        assert_eq!(
            (
                text(/*enabled*/ true).contains(steering),
                text(/*enabled*/ false).contains(steering)
            ),
            (true, false),
            "advisor_mode={advisor_mode}"
        );
    }
}
