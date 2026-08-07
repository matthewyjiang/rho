use std::path::Path;

use pretty_assertions::assert_eq;

use super::parse_definition;
use crate::agent::{AgentRuntimeSpec, ModelPolicy, ModelSelection, ToolPolicy};

fn parse(contents: &str) -> Result<crate::agent::AgentDefinition, crate::agent::AgentCatalogError> {
    parse_definition(Path::new("agent.md"), "agent", contents)
}

#[test]
fn defaults_runtime_to_rho() {
    let definition = parse("---\ndescription: demo\n---\nbody\n").unwrap();
    assert_eq!(
        definition.runtime,
        AgentRuntimeSpec::Rho {
            tools: ToolPolicy::All,
            model: ModelPolicy::Inherit,
            reasoning: None,
        }
    );
}

#[test]
fn parses_claude_cli_runtime_and_tools_independent_of_key_order() {
    let tools_first = parse(
        "---\ndescription: demo\ntools: [Read, Edit, \"Bash(git *)\"]\nruntime: claude-cli\n---\n",
    )
    .unwrap();
    let runtime_first = parse(
        "---\ndescription: demo\nruntime: claude-cli\ntools: [Read, Edit, \"Bash(git *)\"]\n---\n",
    )
    .unwrap();

    assert_eq!(
        tools_first.runtime,
        AgentRuntimeSpec::ClaudeCli(crate::agent::ClaudeAgentConfig {
            tools: crate::agent::ClaudeToolPolicy::Allow(vec![
                "Read".into(),
                "Edit".into(),
                "Bash(git *)".into()
            ]),
            inherit_claude_config: false,
            model: None,
            reasoning: None,
        })
    );
    assert_eq!(tools_first.fingerprint(), runtime_first.fingerprint());
}

#[test]
fn rejects_unknown_runtime_values() {
    let error = parse("---\ndescription: demo\nruntime: bun\n---\n").unwrap_err();
    assert_eq!(error.field.as_deref(), Some("runtime"));
    assert!(error.to_string().contains("expected rho or claude-cli"));
}

#[test]
fn rejects_claude_tools_on_rho_runtime() {
    let error = parse("---\ndescription: demo\nruntime: rho\ntools: [Read]\n---\n").unwrap_err();
    assert_eq!(error.field.as_deref(), Some("tools"));
    assert!(error.to_string().contains("Claude Code tool name"));
    assert!(error.to_string().contains("runtime is rho"));
    assert!(error.to_string().contains("tools: [read_file, shell]"));
}

#[test]
fn rejects_rho_tools_on_claude_runtime() {
    let error = parse("---\ndescription: demo\nruntime: claude-cli\ntools: [read_file]\n---\n")
        .unwrap_err();
    assert_eq!(error.field.as_deref(), Some("tools"));
    assert!(error.to_string().contains("Rho capability"));
    assert!(error.to_string().contains("runtime is claude-cli"));
    assert!(error
        .to_string()
        .contains("tools: [Read, Edit, \"Bash(git *)\"]"));
}

#[test]
fn rejects_invalid_claude_tool_shape() {
    let error = parse("---\ndescription: demo\nruntime: claude-cli\ntools: [\"Bash(git\"]\n---\n")
        .unwrap_err();
    assert_eq!(error.field.as_deref(), Some("tools"));
    assert!(error.to_string().contains("specifier must end the name"));
}

#[test]
fn allows_nested_parentheses_inside_claude_tool_specifier() {
    let definition = parse(
        "---\ndescription: demo\nruntime: claude-cli\ntools: [\"Bash(git log --format=%(refname))\", \"Bash(git *)\"]\n---\n",
    )
    .unwrap();
    assert_eq!(
        definition.runtime,
        AgentRuntimeSpec::ClaudeCli(crate::agent::ClaudeAgentConfig {
            tools: crate::agent::ClaudeToolPolicy::Allow(vec![
                "Bash(git log --format=%(refname))".into(),
                "Bash(git *)".into(),
            ]),
            inherit_claude_config: false,
            model: None,
            reasoning: None,
        })
    );
}

#[test]
fn rejects_claude_tool_patterns_with_commas() {
    for contents in [
        "---\ndescription: demo\nruntime: claude-cli\ntools: [\"Edit(path with , comma)\"]\n---\n",
        "---\ndescription: demo\nruntime: claude-cli\ntools: [\"mcp__server__tool(a,b)\"]\n---\n",
    ] {
        let error = parse(contents).unwrap_err();
        assert_eq!(error.field.as_deref(), Some("tools"), "{contents}");
        assert!(
            error.to_string().contains("commas cannot round-trip"),
            "{contents}: {error}"
        );
    }
}

#[test]
fn rejects_malformed_outer_claude_tool_names() {
    for contents in [
        "---\ndescription: demo\nruntime: claude-cli\ntools: [\"(no-name)\"]\n---\n",
        "---\ndescription: demo\nruntime: claude-cli\ntools: [\"Bad Name\"]\n---\n",
        "---\ndescription: demo\nruntime: claude-cli\ntools: [\"Tool)\"]\n---\n",
        "---\ndescription: demo\nruntime: claude-cli\ntools: [\"\"]\n---\n",
    ] {
        let error = parse(contents).unwrap_err();
        assert_eq!(error.field.as_deref(), Some("tools"), "{contents}");
    }
}

#[test]
fn rejects_empty_quoted_claude_model() {
    let error =
        parse("---\ndescription: demo\nruntime: claude-cli\nmodel: \"\"\ntools: [Read]\n---\n")
            .unwrap_err();
    assert_eq!(error.field.as_deref(), Some("model"));
    assert!(error.to_string().contains("must not be empty"));
}

#[test]
fn rejects_claude_inherit_with_explicit_model_and_select_without_model() {
    let inherit_with_model = parse(
        "---\ndescription: demo\nruntime: claude-cli\nmodel-policy: inherit\nmodel: opus\ntools: [Read]\n---\n",
    )
    .unwrap_err();
    assert_eq!(inherit_with_model.field.as_deref(), Some("model-policy"));
    assert!(inherit_with_model
        .to_string()
        .contains("inherit cannot specify model"));

    let select_without_model = parse(
        "---\ndescription: demo\nruntime: claude-cli\nmodel-policy: select\ntools: [Read]\n---\n",
    )
    .unwrap_err();
    assert_eq!(select_without_model.field.as_deref(), Some("model"));
    assert!(select_without_model
        .to_string()
        .contains("required by model-policy 'select'"));
}

#[test]
fn rho_model_policy_inherit_with_model_still_rejected() {
    let error =
        parse("---\ndescription: demo\nruntime: rho\nmodel-policy: inherit\nmodel: gpt-5.5\n---\n")
            .unwrap_err();
    assert_eq!(error.field.as_deref(), Some("model-policy"));
    assert!(error
        .to_string()
        .contains("inherit cannot specify model, provider, or auth"));
}

#[test]
fn omits_claude_tools_as_empty_allowlist() {
    let definition = parse("---\ndescription: demo\nruntime: claude-cli\n---\n").unwrap();
    assert_eq!(
        definition.runtime,
        AgentRuntimeSpec::ClaudeCli(crate::agent::ClaudeAgentConfig {
            tools: crate::agent::ClaudeToolPolicy::None,
            inherit_claude_config: false,
            model: None,
            reasoning: None,
        })
    );
}

#[test]
fn rejects_tools_all_on_claude_runtime() {
    let error =
        parse("---\ndescription: demo\nruntime: claude-cli\ntools: all\n---\n").unwrap_err();
    assert_eq!(error.field.as_deref(), Some("tools"));
    assert!(error.to_string().contains("does not support tools: all"));
}

#[test]
fn allows_model_and_rejects_provider_on_claude_runtime() {
    let definition = parse(
        "---\ndescription: demo\nruntime: claude-cli\nmodel: claude-opus-4-6\ntools: [Read]\n---\n",
    )
    .unwrap();
    assert_eq!(
        *definition.model_policy(),
        ModelPolicy::Select(ModelSelection {
            provider: None,
            model: "claude-opus-4-6".into(),
            auth: None,
        })
    );
    match &definition.runtime {
        AgentRuntimeSpec::ClaudeCli(config) => {
            assert_eq!(config.model.as_deref(), Some("claude-opus-4-6"));
            assert!(config.reasoning.is_none());
            assert!(config.effort().is_none());
        }
        _ => panic!("expected claude runtime"),
    }

    let error = parse(
        "---\ndescription: demo\nruntime: claude-cli\nprovider: anthropic\nmodel: claude-opus-4-6\ntools: [Read]\n---\n",
    )
    .unwrap_err();
    assert_eq!(error.field.as_deref(), Some("provider"));
    assert!(error
        .to_string()
        .contains("not valid with runtime: claude-cli"));
}

#[test]
fn inherit_claude_config_is_opt_in_for_claude_runtime_only() {
    let definition = parse(
        "---\ndescription: demo\nruntime: claude-cli\ninherit_claude_config: true\ntools: [Read]\n---\n",
    )
    .unwrap();
    assert!(matches!(
        definition.runtime,
        AgentRuntimeSpec::ClaudeCli(crate::agent::ClaudeAgentConfig {
            inherit_claude_config: true,
            ..
        })
    ));

    let rejected =
        parse("---\ndescription: demo\nruntime: rho\ninherit_claude_config: true\n---\n")
            .unwrap_err();
    assert_eq!(rejected.field.as_deref(), Some("inherit_claude_config"));
    assert!(rejected
        .to_string()
        .contains("only valid with runtime: claude-cli"));

    let bad_bool = parse(
        "---\ndescription: demo\nruntime: claude-cli\ninherit_claude_config: yes\ntools: [Read]\n---\n",
    )
    .unwrap_err();
    assert_eq!(bad_bool.field.as_deref(), Some("inherit_claude_config"));
    assert!(bad_bool.to_string().contains("expected true or false"));
}

#[test]
fn fingerprint_includes_runtime_tools_and_inherit_flag() {
    let base = parse(
        "---\ndescription: demo\nruntime: claude-cli\nmodel: opus\ntools: [Read]\n---\nbody\n",
    )
    .unwrap();
    let runtime_change = parse(
        "---\ndescription: demo\nruntime: rho\nmodel: opus\nprovider: anthropic\ntools: [read_file]\n---\nbody\n",
    )
    .unwrap();
    let tools_change = parse(
        "---\ndescription: demo\nruntime: claude-cli\nmodel: opus\ntools: [Read, Edit]\n---\nbody\n",
    )
    .unwrap();
    let inherit_change = parse(
        "---\ndescription: demo\nruntime: claude-cli\nmodel: opus\ninherit_claude_config: true\ntools: [Read]\n---\nbody\n",
    )
    .unwrap();
    let same = parse(
        "---\nid: agent\ndescription: demo\nruntime: claude-cli\nmodel: opus\ntools:\n  - Read\n---\n\nbody\n",
    )
    .unwrap();

    assert_ne!(base.fingerprint(), runtime_change.fingerprint());
    assert_ne!(base.fingerprint(), tools_change.fingerprint());
    assert_ne!(base.fingerprint(), inherit_change.fingerprint());
    assert_eq!(base.fingerprint(), same.fingerprint());
}

#[test]
fn rejects_rho_style_alias_and_unsupported_reasoning_on_claude_runtime() {
    let alias =
        parse("---\ndescription: demo\nruntime: claude-cli\nmodel: @deep\ntools: [Read]\n---\n")
            .unwrap_err();
    assert_eq!(alias.field.as_deref(), Some("model"));
    assert!(alias
        .to_string()
        .contains("does not resolve Rho model aliases"));

    for level in ["off", "minimal"] {
        let reasoning = parse(&format!(
            "---\ndescription: demo\nruntime: claude-cli\nreasoning: {level}\ntools: [Read]\n---\n"
        ))
        .unwrap_err();
        assert_eq!(reasoning.field.as_deref(), Some("reasoning"));
        assert!(
            reasoning
                .to_string()
                .contains("not a Claude Code effort level"),
            "{level}: {reasoning}"
        );
    }
}

#[test]
fn accepts_claude_effort_reasoning_levels() {
    for level in ["low", "medium", "high", "xhigh", "max"] {
        let definition = parse(&format!(
            "---\ndescription: demo\nruntime: claude-cli\nreasoning: {level}\ntools: [Read]\n---\n"
        ))
        .unwrap_or_else(|error| panic!("expected {level} to parse: {error}"));
        assert_eq!(
            definition
                .reasoning()
                .map(|value| value.to_string())
                .as_deref(),
            Some(level)
        );
        match &definition.runtime {
            AgentRuntimeSpec::ClaudeCli(config) => {
                assert_eq!(config.effort(), Some(level));
            }
            _ => panic!("expected claude runtime"),
        }
    }
}

// Covers: auth frontmatter is parsed and validated against provider
// Owner: agent parser
#[test]
fn parses_auth_profile_with_provider() {
    let definition = parse(
        "---\ndescription: demo\nmodel-policy: prefer\nmodel: grok-4.5\nprovider: xai\nauth: xai-oauth\n---\nbody\n",
    )
    .unwrap();
    match definition.model_policy().as_ref() {
        ModelPolicy::Prefer(selection) => {
            assert_eq!(selection.provider.as_deref(), Some("xai"));
            assert_eq!(selection.model, "grok-4.5");
            assert_eq!(selection.auth.as_deref(), Some("xai-oauth"));
        }
        other => panic!("unexpected policy: {other:?}"),
    }
}

// Covers: unknown or mismatched auth fails before execution
// Owner: agent parser
#[test]
fn rejects_unknown_and_mismatched_auth() {
    let unknown = parse(
        "---\ndescription: demo\nmodel: grok-4.5\nprovider: xai\nauth: not-a-real-auth\n---\n",
    )
    .unwrap_err();
    assert_eq!(unknown.field.as_deref(), Some("auth"));
    assert!(unknown.message.contains("unknown auth profile"));

    let mismatched = parse(
        "---\ndescription: demo\nmodel: grok-4.5\nprovider: xai\nauth: anthropic-api-key\n---\n",
    )
    .unwrap_err();
    assert_eq!(mismatched.field.as_deref(), Some("auth"));
    assert!(
        mismatched.message.contains("not valid for provider")
            || mismatched.message.contains("belongs to"),
        "{}",
        mismatched.message
    );

    let on_claude = parse(
        "---\ndescription: demo\nruntime: claude-cli\nmodel: opus\nauth: xai-oauth\ntools: [Read]\n---\n",
    )
    .unwrap_err();
    assert_eq!(on_claude.field.as_deref(), Some("auth"));

    let with_inherit =
        parse("---\ndescription: demo\nmodel-policy: inherit\nauth: xai-oauth\n---\n").unwrap_err();
    assert_eq!(with_inherit.field.as_deref(), Some("model-policy"));
}

// Covers: unset auth keeps legacy fingerprints; pinned auth changes them
// Owner: agent definition fingerprint
#[test]
fn fingerprint_changes_only_when_auth_is_pinned() {
    let without =
        parse("---\ndescription: demo\nmodel: grok-4.5\nprovider: xai\n---\nbody\n").unwrap();
    let with_auth = parse(
        "---\ndescription: demo\nmodel: grok-4.5\nprovider: xai\nauth: xai-oauth\n---\nbody\n",
    )
    .unwrap();
    let same_without = parse(
        "---\ndescription: demo\nmodel-policy: select\nmodel: grok-4.5\nprovider: xai\n---\nbody\n",
    )
    .unwrap();
    assert_eq!(without.fingerprint(), same_without.fingerprint());
    assert_ne!(without.fingerprint(), with_auth.fingerprint());
}

// Covers: auth frontmatter round-trips through serialize/parse
// Owner: agent serializer
#[test]
fn auth_selection_round_trips_through_serialize() {
    let original = parse(
        "---\ndescription: demo\nmodel-policy: prefer\nmodel: grok-4.5\nprovider: xai\nauth: xai-oauth\n---\nbody\n",
    )
    .unwrap();
    let serialized = crate::agent::serialize_definition(&original);
    assert!(serialized.contains("auth: xai-oauth\n"));
    let again = parse(&serialized).unwrap();
    assert_eq!(again, original);
}
