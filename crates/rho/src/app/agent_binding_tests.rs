use super::*;
use crate::agent::{ModelPolicy, PromptPolicy, ToolPolicy};

fn definition(tools: ToolPolicy) -> Arc<AgentDefinition> {
    Arc::new(AgentDefinition {
        id: AgentId::new("test").unwrap(),
        description: "test".into(),
        prompt: PromptPolicy::Extend("instructions".into()),
        model: ModelPolicy::Inherit,
        tools,
        reasoning: None,
    })
}

fn capabilities() -> BTreeSet<String> {
    [
        "read_file",
        "write_file",
        "agent",
        "agents",
        "questionnaire",
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

#[test]
fn root_roles_bind_equivalently() {
    let config = Config::default();
    let interactive = AgentBinder::bind(
        definition(ToolPolicy::All),
        AgentInvocation {
            role: AgentRole::InteractiveRoot,
            available_tools: capabilities(),
        },
        &config,
    )
    .unwrap();
    let automation = AgentBinder::bind(
        definition(ToolPolicy::All),
        AgentInvocation {
            role: AgentRole::AutomationRoot,
            available_tools: capabilities(),
        },
        &config,
    )
    .unwrap();
    assert_eq!(interactive.tools(), automation.tools());
    assert_eq!(interactive.fingerprint(), automation.fingerprint());
}

#[test]
fn delegated_role_removes_recursive_and_interactive_capabilities() {
    let bound = AgentBinder::bind(
        definition(ToolPolicy::All),
        AgentInvocation {
            role: AgentRole::Delegated,
            available_tools: capabilities(),
        },
        &Config::default(),
    )
    .unwrap();
    assert_eq!(
        bound.tools(),
        &["read_file", "write_file"]
            .into_iter()
            .map(String::from)
            .collect()
    );
}

#[test]
fn unavailable_explicit_tool_fails_before_execution() {
    let error = AgentBinder::bind(
        definition(ToolPolicy::Allow(vec!["write_file".into()])),
        AgentInvocation {
            role: AgentRole::AutomationRoot,
            available_tools: ["read_file"].into_iter().map(String::from).collect(),
        },
        &Config::default(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("write_file"));
}

fn definition_with_model(model: ModelPolicy) -> Arc<AgentDefinition> {
    Arc::new(AgentDefinition {
        model,
        ..definition(ToolPolicy::All).as_ref().clone()
    })
}

fn aliases(pairs: &[(&str, &str)]) -> crate::model_aliases::ModelAliases {
    crate::model_aliases::ModelAliases::from_entries(
        pairs
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect(),
    )
    .unwrap()
}

#[test]
fn agent_model_alias_resolves_to_concrete_provider_and_model() {
    let config = Config {
        model_aliases: aliases(&[("deep", "anthropic/claude-opus-4-8")]),
        ..Config::default()
    };
    let bound = AgentBinder::bind(
        definition_with_model(ModelPolicy::Select(crate::agent::ModelSelection {
            provider: None,
            model: "@deep".into(),
        })),
        AgentInvocation {
            role: AgentRole::Delegated,
            available_tools: capabilities(),
        },
        &config,
    )
    .unwrap();

    assert_eq!(bound.config().provider, "anthropic");
    assert_eq!(bound.config().model, "claude-opus-4-8");
    assert_eq!(bound.config().current_model_alias(), Some("deep"));
}

#[test]
fn agent_bare_model_alias_keeps_inherited_provider() {
    let config = Config {
        model_aliases: aliases(&[("fast", "gpt-5.5-mini")]),
        ..Config::default()
    };
    let bound = AgentBinder::bind(
        definition_with_model(ModelPolicy::Select(crate::agent::ModelSelection {
            provider: None,
            model: "@fast".into(),
        })),
        AgentInvocation {
            role: AgentRole::Delegated,
            available_tools: capabilities(),
        },
        &config,
    )
    .unwrap();

    assert_eq!(bound.config().provider, "openai");
    assert_eq!(bound.config().model, "gpt-5.5-mini");
}

#[test]
fn agent_model_alias_conflicting_with_pinned_provider_errors() {
    let config = Config {
        model_aliases: aliases(&[("deep", "anthropic/claude-opus-4-8")]),
        ..Config::default()
    };
    let error = AgentBinder::bind(
        definition_with_model(ModelPolicy::Select(crate::agent::ModelSelection {
            provider: Some("openai".into()),
            model: "@deep".into(),
        })),
        AgentInvocation {
            role: AgentRole::Delegated,
            available_tools: capabilities(),
        },
        &config,
    )
    .unwrap_err();

    assert!(
        error.to_string().contains(
            "model alias '@deep' resolves to provider 'anthropic', which conflicts with the agent's provider 'openai'"
        ),
        "{error:#}"
    );
}

#[test]
fn undefined_agent_model_alias_names_agent_and_reference() {
    let error = AgentBinder::bind(
        definition_with_model(ModelPolicy::Select(crate::agent::ModelSelection {
            provider: None,
            model: "@missing".into(),
        })),
        AgentInvocation {
            role: AgentRole::Delegated,
            available_tools: capabilities(),
        },
        &Config::default(),
    )
    .unwrap_err();

    assert!(
        error.to_string().contains(
            "agent 'test': model alias '@missing' is not defined; define it in [model.aliases] or use a concrete model reference"
        ),
        "{error:#}"
    );
}
