use std::path::Path;

use pretty_assertions::assert_eq;

use super::parse_definition;
use crate::agent::{
    AgentRuntime, AgentTools, ModelPolicy, ModelSelection, PromptPolicy, ToolCapability, ToolPolicy,
};

fn parse(contents: &str) -> Result<crate::agent::AgentDefinition, crate::agent::AgentCatalogError> {
    parse_definition(Path::new("agent.md"), "agent", contents)
}

#[test]
fn defaults_runtime_to_rho() {
    let definition = parse("---\ndescription: demo\n---\nbody\n").unwrap();
    assert_eq!(definition.runtime, AgentRuntime::Rho);
    assert_eq!(definition.tools, AgentTools::Rho(ToolPolicy::All));
    assert!(!definition.inherit_claude_config);
}

#[test]
fn parses_explicit_rho_runtime() {
    let definition =
        parse("---\ndescription: demo\nruntime: rho\ntools: [read_file]\n---\n").unwrap();
    assert_eq!(definition.runtime, AgentRuntime::Rho);
    assert_eq!(
        definition.tools,
        AgentTools::Rho(ToolPolicy::Allow(
            [ToolCapability::ReadFile].into_iter().collect()
        ))
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

    assert_eq!(tools_first.runtime, AgentRuntime::ClaudeCli);
    assert_eq!(
        tools_first.tools,
        AgentTools::Claude(vec!["Read".into(), "Edit".into(), "Bash(git *)".into(),])
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
        definition.tools,
        AgentTools::Claude(vec![
            "Bash(git log --format=%(refname))".into(),
            "Bash(git *)".into(),
        ])
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
        .contains("inherit cannot specify model or provider"));
}

#[test]
fn omits_claude_tools_as_empty_allowlist() {
    let definition = parse("---\ndescription: demo\nruntime: claude-cli\n---\n").unwrap();
    assert_eq!(definition.tools, AgentTools::Claude(Vec::new()));
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
        definition.model,
        ModelPolicy::Select(ModelSelection {
            provider: None,
            model: "claude-opus-4-6".into(),
        })
    );

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
    assert!(definition.inherit_claude_config);

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
fn parses_claude_indented_tool_list_with_patterns() {
    let definition = parse(
        "---\ndescription: demo\nruntime: claude-cli\ntools:\n  - Read\n  - \"Bash(git *)\"\n---\n",
    )
    .unwrap();
    assert_eq!(
        definition.tools,
        AgentTools::Claude(vec!["Read".into(), "Bash(git *)".into()])
    );
    assert!(matches!(definition.prompt, PromptPolicy::Extend(_)));
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
                .reasoning
                .map(|value| value.to_string())
                .as_deref(),
            Some(level)
        );
    }
}
