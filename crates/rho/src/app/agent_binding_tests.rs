use super::*;
use crate::agent::{
    AgentRuntimeSpec, ModelPolicy, ModelSelection, PromptPolicy, ToolCapability, ToolPolicy,
};

fn capability_set(names: &[&str]) -> AgentCapabilities {
    AgentCapabilities::new(
        names
            .iter()
            .map(|name| ToolCapability::parse((*name).to_string()))
            .collect(),
    )
}

fn definition(tools: ToolPolicy) -> Arc<AgentDefinition> {
    Arc::new(AgentDefinition {
        id: AgentId::new("test").unwrap(),
        description: "test".into(),
        prompt: PromptPolicy::Extend("instructions".into()),
        runtime: AgentRuntimeSpec::Rho {
            tools,
            model: ModelPolicy::Inherit,
            reasoning: None,
        },
    })
}

fn capabilities() -> AgentCapabilities {
    capability_set(&[
        "read_file",
        "write_file",
        "agent",
        "agents",
        "questionnaire",
    ])
}

#[test]
fn delegated_role_keeps_questionnaire_when_host_offers_it() {
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
        bound.rho_capabilities(),
        Some(&capability_set(&[
            "read_file",
            "write_file",
            "questionnaire"
        ]))
    );
}

#[test]
fn delegated_role_removes_recursive_capabilities() {
    let bound = AgentBinder::bind(
        definition(ToolPolicy::All),
        AgentInvocation {
            role: AgentRole::Delegated,
            available_tools: capability_set(&["read_file", "write_file", "agent", "agents"]),
        },
        &Config::default(),
    )
    .unwrap();
    assert_eq!(
        bound.rho_capabilities(),
        Some(&capability_set(&["read_file", "write_file"]))
    );
}

#[test]
fn unavailable_explicit_tool_fails_before_execution() {
    let error = AgentBinder::bind(
        definition(ToolPolicy::Allow(
            ["write_file".to_string()]
                .into_iter()
                .map(crate::agent::ToolCapability::parse)
                .collect(),
        )),
        AgentInvocation {
            role: AgentRole::AutomationRoot,
            available_tools: capability_set(&["read_file"]),
        },
        &Config::default(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("write_file"));
}

fn definition_with_model(model: ModelPolicy) -> Arc<AgentDefinition> {
    Arc::new(AgentDefinition {
        runtime: AgentRuntimeSpec::Rho {
            tools: ToolPolicy::All,
            model,
            reasoning: None,
        },
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
fn agent_model_aliases_resolve_qualified_and_bare_targets() {
    let config = Config {
        model_aliases: aliases(&[
            ("deep", "anthropic/claude-opus-4-8"),
            ("fast", "gpt-5.5-mini"),
        ]),
        ..Config::default()
    };

    for (alias, provider, model) in [
        ("@deep", "anthropic", "claude-opus-4-8"),
        ("@fast", "openai", "gpt-5.5-mini"),
    ] {
        let bound = AgentBinder::bind(
            definition_with_model(ModelPolicy::Select(crate::agent::ModelSelection {
                provider: None,
                model: alias.into(),
            })),
            AgentInvocation {
                role: AgentRole::Delegated,
                available_tools: capabilities(),
            },
            &config,
        )
        .unwrap();

        let bound_config = bound.rho_config().expect("rho config");
        assert_eq!(bound_config.provider, provider, "{alias}");
        assert_eq!(bound_config.model, model, "{alias}");
    }
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

fn claude_definition(model: ModelPolicy) -> Arc<AgentDefinition> {
    let claude_model = match &model {
        ModelPolicy::Inherit => None,
        ModelPolicy::Select(selection)
        | ModelPolicy::Prefer(selection)
        | ModelPolicy::Require(selection) => Some(selection.model.clone()),
    };
    Arc::new(AgentDefinition {
        id: AgentId::new("claude-test").unwrap(),
        description: "claude".into(),
        prompt: PromptPolicy::Replace("plan".into()),
        runtime: AgentRuntimeSpec::ClaudeCli(crate::agent::ClaudeAgentConfig {
            tools: crate::agent::ClaudeToolPolicy::Allow(vec!["Read".into(), "Bash(git *)".into()]),
            inherit_claude_config: true,
            model: claude_model,
            reasoning: None,
        }),
    })
}

#[test]
fn claude_binding_is_typed_and_does_not_resolve_aliases_or_mutate_host_config() {
    let host = Config {
        provider: "openai".into(),
        model: "gpt-5.5".into(),
        model_aliases: aliases(&[("deep", "anthropic/claude-opus-4-8")]),
        permission_mode: crate::permission::PermissionMode::Plan,
        ..Config::default()
    };
    let bound = AgentBinder::bind(
        claude_definition(ModelPolicy::Select(ModelSelection {
            provider: None,
            model: "opus".into(),
        })),
        AgentInvocation {
            role: AgentRole::Delegated,
            available_tools: capabilities(),
        },
        &host,
    )
    .unwrap();

    match bound.runtime() {
        BoundRuntime::ClaudeCli {
            model,
            tools,
            inherit_claude_config,
            permission_mode,
            max_turns,
            effort,
        } => {
            assert_eq!(model.as_deref(), Some("opus"));
            assert_eq!(
                tools.as_slice(),
                ["Read".to_string(), "Bash(git *)".to_string()].as_slice()
            );
            assert!(*inherit_claude_config);
            assert_eq!(*permission_mode, crate::permission::PermissionMode::Plan);
            assert_eq!(
                *max_turns,
                crate::app::sdk_config::run_step_limit().get() as u64
            );
            assert!(effort.is_none());
        }
        BoundRuntime::Rho { .. } => panic!("expected Claude bound runtime"),
    }
    assert!(bound.rho_config().is_none());
    assert!(bound.rho_capabilities().is_none());
    // Host config is not mutated through binding.
    assert_eq!(host.provider, "openai");
    assert_eq!(host.model, "gpt-5.5");
}

#[test]
fn claude_runtime_rejects_root_roles() {
    for role in [AgentRole::InteractiveRoot, AgentRole::AutomationRoot] {
        let error = AgentBinder::bind(
            claude_definition(ModelPolicy::Inherit),
            AgentInvocation {
                role,
                available_tools: capabilities(),
            },
            &Config::default(),
        )
        .unwrap_err();
        let message = error.to_string();
        assert!(
            message.contains("delegated-only"),
            "role {role:?}: {message}"
        );
        assert!(message.contains("claude-cli"), "role {role:?}: {message}");
    }
}

#[test]
fn claude_runtime_rejects_alias_models_at_bind() {
    // Parse rejects aliases; bind re-checks constructed configs.
    let error = AgentBinder::bind(
        claude_definition(ModelPolicy::Select(ModelSelection {
            provider: None,
            model: "@deep".into(),
        })),
        AgentInvocation {
            role: AgentRole::Delegated,
            available_tools: capabilities(),
        },
        &Config::default(),
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("does not resolve Rho model aliases"),
        "{error:#}"
    );
    assert!(error.to_string().contains("@deep"), "{error:#}");
}

#[test]
fn claude_runtime_maps_supported_reasoning_and_rejects_unmapped() {
    let mut mapped = claude_definition(ModelPolicy::Inherit).as_ref().clone();
    if let AgentRuntimeSpec::ClaudeCli(config) = &mut mapped.runtime {
        config.reasoning = Some(rho_sdk::ReasoningLevel::High);
    }
    let bound = AgentBinder::bind(
        Arc::new(mapped),
        AgentInvocation {
            role: AgentRole::Delegated,
            available_tools: capabilities(),
        },
        &Config::default(),
    )
    .unwrap();
    match bound.runtime() {
        BoundRuntime::ClaudeCli { effort, .. } => assert_eq!(*effort, Some("high")),
        BoundRuntime::Rho { .. } => panic!("expected Claude bound runtime"),
    }

    let mut unmapped = claude_definition(ModelPolicy::Inherit).as_ref().clone();
    if let AgentRuntimeSpec::ClaudeCli(config) = &mut unmapped.runtime {
        config.reasoning = Some(rho_sdk::ReasoningLevel::Minimal);
    }
    let error = AgentBinder::bind(
        Arc::new(unmapped),
        AgentInvocation {
            role: AgentRole::Delegated,
            available_tools: capabilities(),
        },
        &Config::default(),
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("not a Claude Code effort level"),
        "{error:#}"
    );
}
