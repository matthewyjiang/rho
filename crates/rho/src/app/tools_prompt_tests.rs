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

async fn assemble(config: &Config, cwd: &std::path::Path) -> (bool, String) {
    let diagnostics = RuntimeDiagnostics::new(config);
    let agent = bound_agent(config);
    let assembled = assemble_tools_and_prompt(ToolsAndPromptOptions {
        catalog: None,
        config,
        config_path: cwd.join("config.toml"),
        cwd,
        no_system_prompt: false,
        no_tools: false,
        no_subagents: true,
        questionnaire_enabled: false,
        mcp_elicitation: crate::tools::mcp::McpElicitationSupport::Unavailable,
        mcp_sampling: super::McpSamplingSupport::Unavailable,
        await_catalog_names: false,
        background_subagents: BackgroundSubagents::Disabled,
        diagnostics: &diagnostics,
        agent: &agent,
    })
    .await
    .unwrap();
    let tools = assembled.tools;
    let prompt = assembled.system_prompt;
    let registered = tools.advisor_registered();
    let text = match prompt {
        SystemPrompt::Custom(text) => text,
        SystemPrompt::None => String::new(),
        _ => String::new(),
    };
    (registered, text)
}

// Covers: the advisor tool must appear only when advisor mode is on and an
// advisor model is configured. Steering stays off the system prompt.
// Owner: root tool/prompt assembly.
#[tokio::test]
async fn the_advisor_tool_needs_both_the_mode_and_a_model() {
    let cwd = tempfile::tempdir().unwrap();
    let cases = [
        (false, false, false),
        (true, false, false),
        (false, true, false),
        (true, true, true),
    ];

    for (advisor_mode, with_model, expected) in cases {
        let config = advisor_config(advisor_mode, with_model);

        let (registered, prompt) = assemble(&config, cwd.path()).await;

        assert_eq!(
            registered, expected,
            "advisor_mode={advisor_mode} with_model={with_model}"
        );
        assert!(
            !prompt.contains("Call advisor BEFORE substantive work"),
            "system prompt must stay advisor-agnostic; advisor_mode={advisor_mode} with_model={with_model}"
        );
    }
}

// Covers: the advisor must review the prompt the executor actually runs with.
// Owner: root tool/prompt assembly.
#[tokio::test]
async fn the_advisor_receives_the_executor_system_prompt() {
    let cwd = tempfile::tempdir().unwrap();
    let config = advisor_config(true, true);
    let diagnostics = RuntimeDiagnostics::new(&config);
    let agent = bound_agent(&config);

    let assembled = assemble_tools_and_prompt(ToolsAndPromptOptions {
        catalog: None,
        config: &config,
        config_path: cwd.path().join("config.toml"),
        cwd: cwd.path(),
        no_system_prompt: false,
        no_tools: false,
        no_subagents: true,
        questionnaire_enabled: false,
        mcp_elicitation: crate::tools::mcp::McpElicitationSupport::Unavailable,
        mcp_sampling: super::McpSamplingSupport::Unavailable,
        await_catalog_names: false,
        background_subagents: BackgroundSubagents::Disabled,
        diagnostics: &diagnostics,
        agent: &agent,
    })
    .await
    .unwrap();
    let tools = assembled.tools;
    let prompt = assembled.system_prompt;

    let SystemPrompt::Custom(text) = prompt else {
        panic!("expected a custom system prompt");
    };
    let store = tools.advisor().expect("advisor store");
    assert_eq!(store.system_prompt(), Some(text));
}

// Covers: the executor system prompt is a single form that does not encode
// advisor registration. Mid-session toggles must not rely on swapping prompts.
// Owner: root tool/prompt assembly.
#[tokio::test]
async fn system_prompt_stays_advisor_agnostic() {
    let cwd = tempfile::tempdir().unwrap();

    for advisor_mode in [false, true] {
        let config = advisor_config(advisor_mode, /*with_model*/ true);
        let diagnostics = RuntimeDiagnostics::new(&config);
        let agent = bound_agent(&config);

        let prompt = assemble_tools_and_prompt(ToolsAndPromptOptions {
            catalog: None,
            config: &config,
            config_path: cwd.path().join("config.toml"),
            cwd: cwd.path(),
            no_system_prompt: false,
            no_tools: false,
            no_subagents: true,
            questionnaire_enabled: false,
            mcp_elicitation: crate::tools::mcp::McpElicitationSupport::Unavailable,
            mcp_sampling: super::McpSamplingSupport::Unavailable,
            await_catalog_names: false,
            background_subagents: BackgroundSubagents::Disabled,
            diagnostics: &diagnostics,
            agent: &agent,
        })
        .await
        .unwrap()
        .system_prompt;

        let text = match prompt {
            SystemPrompt::Custom(text) => text,
            SystemPrompt::None => String::new(),
            _ => String::new(),
        };
        assert!(
            !text.contains("Call advisor BEFORE substantive work"),
            "advisor_mode={advisor_mode}"
        );
        assert!(
            !text.contains("You have access to an `advisor` tool"),
            "advisor_mode={advisor_mode}"
        );
    }
}

// Covers: the assembled system prompt names the model this run actually bound,
// so an agent that pins its own model is told that model, not the host's.
// Owner: root tool/prompt assembly.
#[tokio::test]
async fn the_assembled_prompt_names_the_bound_model() {
    let cwd = tempfile::tempdir().unwrap();
    let config = Config {
        provider: "openai".into(),
        model: "gpt-5.6-sol".into(),
        ..Config::default()
    };

    let (_, prompt) = assemble(&config, cwd.path()).await;

    // The seam, not the wording: the bound model reaches the assembled prompt.
    assert!(prompt.contains("openai/gpt-5.6-sol"), "{prompt}");
}
