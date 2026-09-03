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
    capability_set(&["read_file", "write", "agent", "agents", "questionnaire"])
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
        Some(&capability_set(&["read_file", "write", "questionnaire"]))
    );
}

// Covers: explicit questionnaire allowlists still bind when the host can answer.
// Owner: delegated agent binding.
#[test]
fn delegated_allowlist_keeps_selected_questionnaire_when_host_offers_it() {
    let bound = AgentBinder::bind(
        definition(ToolPolicy::Allow(
            ["read_file", "questionnaire"]
                .into_iter()
                .map(|name| ToolCapability::parse(name.to_string()))
                .collect(),
        )),
        AgentInvocation {
            role: AgentRole::Delegated,
            available_tools: capabilities(),
        },
        &Config::default(),
    )
    .unwrap();
    assert_eq!(
        bound.rho_capabilities(),
        Some(&capability_set(&["read_file", "questionnaire"]))
    );
}

// Covers: listing questionnaire must not fail bind when this launch has no bridge.
// Owner: delegated agent binding.
#[test]
fn delegated_allowlist_omits_questionnaire_when_host_does_not_offer_it() {
    let bound = AgentBinder::bind(
        definition(ToolPolicy::Allow(
            ["read_file", "questionnaire"]
                .into_iter()
                .map(|name| ToolCapability::parse(name.to_string()))
                .collect(),
        )),
        AgentInvocation {
            role: AgentRole::Delegated,
            available_tools: capability_set(&["read_file", "write"]),
        },
        &Config::default(),
    )
    .unwrap();
    assert_eq!(
        bound.rho_capabilities(),
        Some(&capability_set(&["read_file"]))
    );
}

// Covers: recursive tools listed on a child definition are ignored, not fatal.
// Owner: delegated agent binding.
#[test]
fn delegated_allowlist_omits_recursive_tools_when_listed() {
    let bound = AgentBinder::bind(
        definition(ToolPolicy::Allow(
            ["read_file", "agent", "agents", "advisor"]
                .into_iter()
                .map(|name| ToolCapability::parse(name.to_string()))
                .collect(),
        )),
        AgentInvocation {
            role: AgentRole::Delegated,
            available_tools: capability_set(&["read_file", "agent", "agents", "advisor"]),
        },
        &Config::default(),
    )
    .unwrap();
    assert_eq!(
        bound.rho_capabilities(),
        Some(&capability_set(&["read_file"]))
    );
}

#[test]
fn delegated_role_removes_recursive_capabilities() {
    let bound = AgentBinder::bind(
        definition(ToolPolicy::All),
        AgentInvocation {
            role: AgentRole::Delegated,
            available_tools: capability_set(&["read_file", "write", "agent", "agents"]),
        },
        &Config::default(),
    )
    .unwrap();
    assert_eq!(
        bound.rho_capabilities(),
        Some(&capability_set(&["read_file", "write"]))
    );
}

// Covers: workflow agents must not recurse or wait on an interactive question.
// Owner: workflow agent binding.
#[test]
fn workflow_role_removes_orchestration_and_questionnaire_capabilities() {
    let bound = AgentBinder::bind(
        definition(ToolPolicy::All),
        AgentInvocation {
            role: AgentRole::Workflow,
            available_tools: capability_set(&[
                "read_file",
                "write",
                "agent",
                "agents",
                "questionnaire",
                "rho",
                "workflow",
            ]),
        },
        &Config::default(),
    )
    .unwrap();
    assert_eq!(
        bound.rho_capabilities(),
        Some(&capability_set(&["read_file", "write"]))
    );
}

// Covers: resume must use frozen launch choices while current policy can only narrow them.
// Owner: frozen workflow agent binding.
#[test]
fn frozen_binding_does_not_rebind_and_narrows_current_policy() {
    let source = definition(ToolPolicy::All);
    let frozen = crate::workflow::ResolvedAgent {
        agent_id: source.id.to_string(),
        fingerprint: source.fingerprint().to_string(),
        runtime: crate::workflow::AgentRuntime::Rho,
        source_origin: "project".into(),
        trust_required: true,
        prompt_policy: "replace:frozen".into(),
        provider: Some("anthropic".into()),
        model: Some("claude-sonnet-4-6".into()),
        reasoning: None,
        step_limit: 17,
        capabilities: ["read_file", "write", "workflow"]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        permission_ceiling: "auto".into(),
        auth_profile: Some("anthropic".into()),
        executable: None,
        executable_identity: None,
        arguments: Vec::new(),
    };
    let current = Config {
        provider: "openai".into(),
        model: "changed".into(),
        permission_mode: crate::permission::PermissionMode::Plan,
        ..Config::default()
    };
    let bound = AgentBinder::bind_frozen(&frozen, &current, &capabilities()).unwrap();

    assert_eq!(bound.prompt(), &PromptPolicy::Replace("frozen".into()));
    assert_eq!(bound.step_limit(), 17);
    assert_eq!(bound.rho_config().unwrap().provider, "anthropic");
    assert_eq!(bound.rho_config().unwrap().model, "claude-sonnet-4-6");
    assert_eq!(
        bound.rho_config().unwrap().permission_mode,
        crate::permission::PermissionMode::Plan
    );
    assert_eq!(
        bound.rho_capabilities(),
        Some(&capability_set(&["read_file", "write"]))
    );
}

// Covers: a frozen Bypass ceiling plus current Auto narrows to Auto and
// binds a claude-cli agent instead of erroring at the mapping boundary.
// Owner: frozen workflow agent binding
#[test]
fn frozen_claude_cli_bypass_ceiling_narrows_to_current_auto() {
    let source = claude_definition(ModelPolicy::Inherit);
    let frozen = crate::workflow::ResolvedAgent {
        agent_id: source.id.to_string(),
        fingerprint: source.fingerprint().to_string(),
        runtime: crate::workflow::AgentRuntime::ClaudeCli,
        source_origin: "project".into(),
        trust_required: true,
        prompt_policy: "replace:frozen".into(),
        provider: None,
        model: Some("opus".into()),
        reasoning: None,
        step_limit: 9,
        capabilities: ["Read", "Bash(git status:*)"]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        permission_ceiling: "bypass".into(),
        auth_profile: None,
        executable: None,
        executable_identity: None,
        arguments: Vec::new(),
    };
    let current = Config {
        permission_mode: crate::permission::PermissionMode::Auto,
        ..Config::default()
    };
    let bound = AgentBinder::bind_frozen(&frozen, &current, &capabilities()).unwrap();
    match bound.runtime() {
        BoundRuntime::ClaudeCli {
            model,
            tools,
            permission_mode,
            max_turns,
            ..
        } => {
            assert_eq!(model.as_deref(), Some("opus"));
            assert_eq!(
                tools.as_slice(),
                ["Bash(git status:*)".to_string(), "Read".to_string()].as_slice()
            );
            assert_eq!(*permission_mode, crate::permission::PermissionMode::Auto);
            assert_eq!(*max_turns, 9);
        }
        BoundRuntime::Rho { .. } | BoundRuntime::Cursor { .. } => {
            panic!("expected Claude bound runtime")
        }
    }
}

#[test]
fn bind_drops_web_search_when_bound_path_cannot_search() {
    let host = Config {
        provider: "openai".into(),
        model: "gpt-5.5".into(),
        web_search_hosted: true,
        web_search_provider: crate::config::SearchProvider::Disabled,
        ..Config::default()
    };
    let available = capability_set(&["read_file", "web_search"]);

    let openai = AgentBinder::bind(
        definition(ToolPolicy::All),
        AgentInvocation {
            role: AgentRole::Delegated,
            available_tools: available.clone(),
        },
        &host,
    )
    .unwrap();
    assert!(openai
        .rho_capabilities()
        .unwrap()
        .contains(&ToolCapability::WebSearch));

    let anthropic = AgentBinder::bind(
        definition_with_model(ModelPolicy::Select(ModelSelection {
            provider: Some("anthropic".into()),
            model: "claude-opus-4-8".into(),
            auth: None,
        })),
        AgentInvocation {
            role: AgentRole::Delegated,
            available_tools: available,
        },
        &host,
    )
    .unwrap();
    assert!(!anthropic
        .rho_capabilities()
        .unwrap()
        .contains(&ToolCapability::WebSearch));
}

#[test]
fn unavailable_explicit_tool_fails_before_execution() {
    let error = AgentBinder::bind(
        definition(ToolPolicy::Allow(
            ["write".to_string()]
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
    assert!(error.to_string().contains("write"));
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
                auth: None,
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
            auth: None,
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
            auth: None,
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
            auth: None,
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
            reasoning,
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
            assert!(reasoning.is_none());
        }
        BoundRuntime::Rho { .. } | BoundRuntime::Cursor { .. } => {
            panic!("expected Claude bound runtime")
        }
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
            auth: None,
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
        BoundRuntime::ClaudeCli { reasoning, .. } => {
            assert_eq!(*reasoning, Some(rho_sdk::ReasoningLevel::High));
        }
        BoundRuntime::Rho { .. } | BoundRuntime::Cursor { .. } => {
            panic!("expected Claude bound runtime")
        }
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

// Covers: attach reads bound reasoning from the Starting snapshot; Claude
// inherit stays absent so the header can omit the field.
// Owner: agent bind identity
#[test]
fn artifact_identity_stamps_bound_reasoning_onto_starting_status() {
    let host = Config {
        reasoning: rho_sdk::ReasoningLevel::Low,
        ..Config::default()
    };
    let rho = AgentBinder::bind(
        definition(ToolPolicy::All),
        AgentInvocation {
            role: AgentRole::Delegated,
            available_tools: capabilities(),
        },
        &host,
    )
    .unwrap();
    let rho_identity = rho.artifact_identity();
    assert_eq!(rho_identity.reasoning, Some(rho_sdk::ReasoningLevel::Low));
    assert_eq!(
        rho_identity.starting_status().reasoning,
        Some(rho_sdk::ReasoningLevel::Low)
    );

    let inherit = AgentBinder::bind(
        claude_definition(ModelPolicy::Inherit),
        AgentInvocation {
            role: AgentRole::Delegated,
            available_tools: capabilities(),
        },
        &Config::default(),
    )
    .unwrap();
    let inherit_identity = inherit.artifact_identity();
    assert_eq!(inherit_identity.reasoning, None);
    assert_eq!(inherit_identity.starting_status().reasoning, None);

    let mut mapped = claude_definition(ModelPolicy::Inherit).as_ref().clone();
    if let AgentRuntimeSpec::ClaudeCli(config) = &mut mapped.runtime {
        config.reasoning = Some(rho_sdk::ReasoningLevel::High);
    }
    let high = AgentBinder::bind(
        Arc::new(mapped),
        AgentInvocation {
            role: AgentRole::Delegated,
            available_tools: capabilities(),
        },
        &Config::default(),
    )
    .unwrap();
    let high_identity = high.artifact_identity();
    assert_eq!(high_identity.reasoning, Some(rho_sdk::ReasoningLevel::High));
    assert_eq!(
        high_identity.starting_status().reasoning,
        Some(rho_sdk::ReasoningLevel::High)
    );
}

// Covers: pinning provider xai must not force API-key auth over host OAuth
// Owner: agent binding
#[test]
fn provider_without_auth_keeps_compatible_host_auth() {
    let host = Config {
        provider: "xai".into(),
        model: "grok-4.5".into(),
        auth: "xai-oauth".into(),
        ..Config::default()
    };
    let bound = AgentBinder::bind(
        definition_with_model(ModelPolicy::Prefer(ModelSelection {
            provider: Some("xai".into()),
            model: "grok-4.5".into(),
            auth: None,
        })),
        AgentInvocation {
            role: AgentRole::Delegated,
            available_tools: capabilities(),
        },
        &host,
    )
    .unwrap();
    let config = bound.rho_config().unwrap();
    assert_eq!(config.provider, "xai");
    assert_eq!(config.auth, "xai-oauth");
    assert_eq!(config.model, "grok-4.5");
}

// Covers: explicit auth pin overrides host auth for the bound run
// Owner: agent binding
#[test]
fn explicit_auth_pin_overrides_host_auth() {
    let host = Config {
        provider: "xai".into(),
        model: "grok-4.5".into(),
        auth: "xai-oauth".into(),
        ..Config::default()
    };
    let bound = AgentBinder::bind(
        definition_with_model(ModelPolicy::Select(ModelSelection {
            provider: Some("xai".into()),
            model: "grok-4.5".into(),
            auth: Some("xai-api-key".into()),
        })),
        AgentInvocation {
            role: AgentRole::Delegated,
            available_tools: capabilities(),
        },
        &host,
    )
    .unwrap();
    let config = bound.rho_config().unwrap();
    assert_eq!(config.provider, "xai");
    assert_eq!(config.auth, "xai-api-key");
}

// Covers: switching provider without auth falls back to that provider default
// Owner: agent binding
#[test]
fn provider_switch_without_auth_uses_provider_default() {
    let host = Config {
        provider: "openai".into(),
        model: "gpt-5.5".into(),
        auth: "api-key".into(),
        ..Config::default()
    };
    let bound = AgentBinder::bind(
        definition_with_model(ModelPolicy::Select(ModelSelection {
            provider: Some("xai".into()),
            model: "grok-4.5".into(),
            auth: None,
        })),
        AgentInvocation {
            role: AgentRole::Delegated,
            available_tools: capabilities(),
        },
        &host,
    )
    .unwrap();
    let config = bound.rho_config().unwrap();
    assert_eq!(config.provider, "xai");
    assert_eq!(config.auth, "xai-api-key");
}

// Covers: the model prediction matches the model binding actually picks.
// Owner: agent binding.
//
// `prompt_model_for_definition` answers "which model would this agent launch on"
// before any launch, so startup can prefetch that model's catalog name. Binding
// answers the same question at launch through the shared policy applicator.
// Drift means prefetching one model's name and running another.
#[test]
fn predicted_agent_model_matches_the_model_binding_picks() {
    use crate::model_identity::PromptModel;

    let host = Config {
        provider: "openai".into(),
        model: "gpt-5.5".into(),
        auth: "api-key".into(),
        model_aliases: aliases(&[("fast", "xai/grok-4.5"), ("bare", "gpt-5.6-sol")]),
        ..Config::default()
    };

    let policies = [
        ("inherit", ModelPolicy::Inherit),
        (
            "model only",
            ModelPolicy::Select(ModelSelection {
                provider: None,
                model: "gpt-5.6-sol".into(),
                auth: None,
            }),
        ),
        (
            "provider and model",
            ModelPolicy::Require(ModelSelection {
                provider: Some("xai".into()),
                model: "grok-4.5".into(),
                auth: None,
            }),
        ),
        (
            "alias that carries a provider",
            ModelPolicy::Prefer(ModelSelection {
                provider: None,
                model: "@fast".into(),
                auth: None,
            }),
        ),
        (
            "alias that keeps the host provider",
            ModelPolicy::Select(ModelSelection {
                provider: None,
                model: "@bare".into(),
                auth: None,
            }),
        ),
        (
            "auth pin without provider",
            ModelPolicy::Select(ModelSelection {
                provider: None,
                model: "claude-fable-5".into(),
                auth: Some("anthropic-api-key".into()),
            }),
        ),
    ];

    for (name, policy) in policies {
        let definition = definition_with_model(policy);
        let predicted = prompt_model_for_definition(&definition, &host)
            .expect("bindable policy should predict a model");
        let bound = AgentBinder::bind(
            Arc::clone(&definition),
            AgentInvocation {
                role: AgentRole::Delegated,
                available_tools: capabilities(),
            },
            &host,
        )
        .unwrap();

        assert_eq!(predicted, bound.prompt_model(), "{name}");
        if name == "auth pin without provider" {
            assert_eq!(
                predicted,
                PromptModel::Rho {
                    provider: "anthropic".into(),
                    model: "claude-fable-5".into(),
                },
                "{name}"
            );
        }
    }
}

// Covers: a policy that cannot bind is not inventing a prefetch key.
// Owner: agent binding.
#[test]
fn unbindable_agent_policy_predicts_no_model() {
    let host = Config {
        provider: "openai".into(),
        model: "gpt-5.5".into(),
        auth: "api-key".into(),
        ..Config::default()
    };
    let definition = definition_with_model(ModelPolicy::Select(ModelSelection {
        provider: None,
        model: "@missing-alias".into(),
        auth: None,
    }));

    assert_eq!(prompt_model_for_definition(&definition, &host), None);
    assert!(AgentBinder::bind(
        Arc::clone(&definition),
        AgentInvocation {
            role: AgentRole::Delegated,
            available_tools: capabilities(),
        },
        &host,
    )
    .is_err());
}

// Covers: a claude-cli agent reports its pass-through `--model`, not a Rho one.
// Owner: agent binding.
#[test]
fn predicted_claude_agent_model_is_the_pass_through_value() {
    use crate::model_identity::PromptModel;

    for model in [Some("opus".to_string()), None] {
        let definition = Arc::new(AgentDefinition {
            runtime: AgentRuntimeSpec::ClaudeCli(crate::agent::ClaudeAgentConfig {
                tools: crate::agent::ClaudeToolPolicy::None,
                inherit_claude_config: false,
                model: model.clone(),
                reasoning: None,
            }),
            ..definition(ToolPolicy::All).as_ref().clone()
        });

        let predicted = prompt_model_for_definition(&definition, &Config::default())
            .expect("claude-cli agents always predict");
        let bound = AgentBinder::bind(
            Arc::clone(&definition),
            AgentInvocation {
                role: AgentRole::Delegated,
                available_tools: capabilities(),
            },
            &Config::default(),
        )
        .unwrap();

        assert_eq!(
            predicted,
            PromptModel::ClaudeCli {
                requested: model,
                resolved: None,
            }
        );
        assert_eq!(predicted, bound.prompt_model());
    }
}

fn cursor_definition(
    model: Option<&str>,
    tools: &[crate::agent::CursorTool],
) -> Arc<AgentDefinition> {
    Arc::new(AgentDefinition {
        id: AgentId::new("cursor-test").unwrap(),
        description: "cursor".into(),
        prompt: PromptPolicy::Extend("instructions".into()),
        runtime: AgentRuntimeSpec::Cursor(crate::agent::CursorAgentConfig {
            tools: tools.to_vec(),
            model: model.map(str::to_string),
        }),
    })
}

// Covers: a cursor agent binds as delegated with pass-through model and tools.
// Owner: delegated agent binding
#[test]
fn cursor_binds_delegated_with_model_and_tools() {
    use crate::agent::CursorTool;
    use pretty_assertions::assert_eq;

    let bound = AgentBinder::bind(
        cursor_definition(
            Some("gpt-5.3-codex[effort=high]"),
            &[CursorTool::Read, CursorTool::Grep],
        ),
        AgentInvocation {
            role: AgentRole::Delegated,
            available_tools: capabilities(),
        },
        &Config::default(),
    )
    .unwrap();

    match bound.runtime() {
        BoundRuntime::Cursor {
            model,
            tools,
            permission_mode,
        } => {
            assert_eq!(model.as_deref(), Some("gpt-5.3-codex[effort=high]"));
            assert_eq!(tools.as_slice(), [CursorTool::Read, CursorTool::Grep]);
            assert_eq!(*permission_mode, crate::permission::PermissionMode::Bypass);
        }
        BoundRuntime::Rho { .. } | BoundRuntime::ClaudeCli { .. } => {
            panic!("expected Cursor bound runtime")
        }
    }
    assert!(bound.rho_config().is_none());
    assert!(bound.rho_capabilities().is_none());
    assert_eq!(bound.runtime().capacity_class(), CapacityClass::Cursor);
}

// Covers: Auto / Allow edits / Supervised cannot reach Cursor spawn.
// Owner: delegated agent binding
#[test]
fn cursor_refuses_auto_allow_edits_supervised_at_bind() {
    use crate::agent::CursorTool;

    let modes = [
        crate::permission::PermissionMode::Auto,
        crate::permission::PermissionMode::AllowEdits,
        crate::permission::PermissionMode::Supervised,
    ];
    for mode in modes {
        let error = AgentBinder::bind(
            cursor_definition(None, &[CursorTool::Read]),
            AgentInvocation {
                role: AgentRole::Delegated,
                available_tools: capabilities(),
            },
            &Config {
                permission_mode: mode,
                ..Config::default()
            },
        )
        .unwrap_err();
        let message = error.to_string();
        assert!(
            message.contains("Plan or Bypass"),
            "mode {mode:?}: {message}"
        );
    }
}

// Covers: frozen Cursor tools stay declared at bind; Plan spawn keeps only Read.
// Owner: frozen workflow agent binding
#[test]
fn cursor_frozen_bind_narrows_plan_to_read_only_tools() {
    use crate::agent::CursorTool;
    use pretty_assertions::assert_eq;

    let source = cursor_definition(
        Some("composer-2.5"),
        &[CursorTool::Read, CursorTool::Edit, CursorTool::Shell],
    );
    let frozen = crate::workflow::ResolvedAgent {
        agent_id: source.id.to_string(),
        fingerprint: source.fingerprint().to_string(),
        runtime: crate::workflow::AgentRuntime::Cursor,
        source_origin: "project".into(),
        trust_required: true,
        prompt_policy: "extend:frozen".into(),
        provider: None,
        model: Some("composer-2.5".into()),
        reasoning: None,
        step_limit: 9,
        capabilities: ["read_tool_call", "edit_tool_call", "shell_tool_call"]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        permission_ceiling: "bypass".into(),
        auth_profile: None,
        executable: None,
        executable_identity: None,
        arguments: Vec::new(),
    };
    let current = Config {
        permission_mode: crate::permission::PermissionMode::Plan,
        ..Config::default()
    };
    let bound = AgentBinder::bind_frozen(&frozen, &current, &capabilities()).unwrap();
    match bound.runtime() {
        BoundRuntime::Cursor {
            tools,
            permission_mode,
            ..
        } => {
            assert_eq!(
                tools.as_slice(),
                [CursorTool::Edit, CursorTool::Read, CursorTool::Shell]
            );
            assert_eq!(*permission_mode, crate::permission::PermissionMode::Plan);
            let allowed =
                crate::cursor_runtime::spawn::map_permission_mode(*permission_mode, tools)
                    .expect("Plan with Read must spawn");
            assert_eq!(allowed.tools(), &[CursorTool::Read]);
        }
        BoundRuntime::Rho { .. } | BoundRuntime::ClaudeCli { .. } => {
            panic!("expected Cursor bound runtime")
        }
    }
}

// Covers: frozen Cursor capabilities are a closed set; unknown names fail bind.
// Owner: frozen workflow agent binding
#[test]
fn cursor_frozen_capabilities_fail_closed_on_unknown_tool() {
    use crate::agent::CursorTool;

    let source = cursor_definition(Some("composer-2.5"), &[CursorTool::Read]);
    let frozen = crate::workflow::ResolvedAgent {
        agent_id: source.id.to_string(),
        fingerprint: source.fingerprint().to_string(),
        runtime: crate::workflow::AgentRuntime::Cursor,
        source_origin: "project".into(),
        trust_required: true,
        prompt_policy: "extend:frozen".into(),
        provider: None,
        model: Some("composer-2.5".into()),
        reasoning: None,
        step_limit: 9,
        capabilities: ["read_tool_call", "not_a_cursor_tool"]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        permission_ceiling: "bypass".into(),
        auth_profile: None,
        executable: None,
        executable_identity: None,
        arguments: Vec::new(),
    };
    let error = AgentBinder::bind_frozen(&frozen, &Config::default(), &capabilities()).unwrap_err();
    let message = error.to_string();
    assert!(message.contains("unknown Cursor tool"), "{message}");
    assert!(message.contains("not_a_cursor_tool"), "{message}");
}

// Covers: a pinned Cursor model missing from a non-empty cache warns at bind
// and still launches.
// Owner: delegated agent binding
#[test]
fn cursor_unknown_cached_model_warns_and_still_binds() {
    use crate::agent::CursorTool;
    use crate::cursor_runtime::models::{cache_models, CursorModel};
    use rho_providers::model::provider_models::{
        with_provider_models_cache_dir_for_tests, CliProviderRefreshContext,
    };

    let cache = tempfile::tempdir().unwrap();
    with_provider_models_cache_dir_for_tests(cache.path().to_path_buf(), || {
        cache_models(
            &[CursorModel {
                id: "composer-2.5".into(),
                display_name: "Composer 2.5".into(),
                is_default: false,
                is_current: true,
                zdr: true,
            }],
            CliProviderRefreshContext::default(),
        )
        .unwrap();

        let unknown = AgentBinder::bind(
            cursor_definition(Some("not-a-cursor-model"), &[CursorTool::Read]),
            AgentInvocation {
                role: AgentRole::Delegated,
                available_tools: capabilities(),
            },
            &Config::default(),
        )
        .unwrap();
        assert_eq!(
            unknown.bind_warnings(),
            ["cursor model 'not-a-cursor-model' is not in the cached list"]
        );

        let known = AgentBinder::bind(
            cursor_definition(Some("composer-2.5[effort=high]"), &[CursorTool::Read]),
            AgentInvocation {
                role: AgentRole::Delegated,
                available_tools: capabilities(),
            },
            &Config::default(),
        )
        .unwrap();
        assert!(known.bind_warnings().is_empty());
    });
}
