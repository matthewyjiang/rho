use std::path::Path;

use pretty_assertions::assert_eq;

use super::parse_definition;
use crate::agent::{
    AgentRuntimeSpec, AntigravityAgentConfig, AntigravityTool, CursorAgentConfig, CursorTool,
    ModelPolicy, ModelSelection, ToolPolicy,
};

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

// Covers: invalid frontmatter fails discovery for that file and names the
// offending field, so the catalog can report it instead of loading a guess.
// Owner: agent parser
#[test]
fn rejects_invalid_frontmatter_with_the_offending_field() {
    let fm = |extra: &str| format!("---\ndescription: demo\n{extra}\n---\n");
    let claude = |extra: &str| fm(&format!("runtime: claude-cli\n{extra}"));
    let cases = [
        ("runtime", fm("runtime: bun")),
        ("tools", fm("runtime: rho\ntools: [Read]")),
        ("tools", claude("tools: [read_file]")),
        ("tools", claude("tools: all")),
        ("tools", claude("tools: [\"Bash(git\"]")),
        ("tools", claude("tools: [\"Edit(path with , comma)\"]")),
        ("tools", claude("tools: [\"mcp__server__tool(a,b)\"]")),
        ("tools", claude("tools: [\"(no-name)\"]")),
        ("tools", claude("tools: [\"Bad Name\"]")),
        ("tools", claude("tools: [\"Tool)\"]")),
        ("tools", claude("tools: [\"\"]")),
        ("model", claude("model: \"\"\ntools: [Read]")),
        ("model", claude("model: @deep\ntools: [Read]")),
        ("reasoning", claude("reasoning: off\ntools: [Read]")),
        ("reasoning", claude("reasoning: minimal\ntools: [Read]")),
        (
            "model-policy",
            claude("model-policy: inherit\nmodel: opus\ntools: [Read]"),
        ),
        ("model", claude("model-policy: select\ntools: [Read]")),
        (
            "model-policy",
            fm("runtime: rho\nmodel-policy: inherit\nmodel: gpt-5.5"),
        ),
        (
            "auth",
            fm("model: grok-4.5\nprovider: xai\nauth: not-a-real-auth"),
        ),
        (
            "auth",
            fm("model: grok-4.5\nprovider: xai\nauth: anthropic-api-key"),
        ),
        (
            "auth",
            claude("model: opus\nauth: xai-oauth\ntools: [Read]"),
        ),
        ("model-policy", fm("model-policy: inherit\nauth: xai-oauth")),
    ];
    for (field, contents) in cases {
        let error = parse(&contents).expect_err(&contents);
        assert_eq!(error.field.as_deref(), Some(field), "{contents}: {error}");
    }
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
            fast: false,
        })
    );
    match &definition.runtime {
        AgentRuntimeSpec::ClaudeCli(config) => {
            assert_eq!(config.model.as_deref(), Some("claude-opus-4-6"));
            assert!(config.reasoning.is_none());
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
        assert!(matches!(definition.runtime, AgentRuntimeSpec::ClaudeCli(_)));
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

// Covers: closed-vocabulary runtimes (cursor, antigravity) take a nonempty
// known tool allow list with no reasoning, no replace prompt, and no
// Claude-only fields; a model passes through byte-for-byte.
// Owner: agent parser
#[test]
fn closed_tool_runtime_frontmatter_rules() {
    let runtimes = [
        (
            "cursor",
            "read_tool_call",
            "task_tool_call",
            AgentRuntimeSpec::Cursor(CursorAgentConfig {
                tools: vec![CursorTool::Read],
                model: Some("gpt-5.3-codex[effort=high,fast=false]".into()),
            }),
        ),
        (
            "antigravity",
            "view_file",
            "start_subagent",
            AgentRuntimeSpec::Antigravity(AntigravityAgentConfig {
                tools: vec![AntigravityTool::ViewFile],
                model: Some("gpt-5.3-codex[effort=high,fast=false]".into()),
            }),
        ),
    ];
    for (runtime, tool, unknown, expected) in runtimes {
        let definition = parse(&format!(
            "---\ndescription: demo\nruntime: {runtime}\nmodel: \"gpt-5.3-codex[effort=high,fast=false]\"\ntools: [{tool}]\n---\nbody\n"
        ))
        .unwrap_or_else(|error| panic!("{runtime}: {error}"));
        assert_eq!(definition.runtime, expected, "{runtime}");
        assert_eq!(definition.reasoning(), None, "{runtime}");

        let rejected = [
            ("tools: all", "tools: all".to_owned(), "tools"),
            ("empty tools", "tools: []".to_owned(), "tools"),
            ("missing tools", String::new(), "tools"),
            ("unknown tool", format!("tools: [{unknown}]"), "tools"),
            (
                "duplicate tool",
                format!("tools: [{tool}, {tool}]"),
                "tools",
            ),
            (
                "reasoning",
                format!("reasoning: high\ntools: [{tool}]"),
                "reasoning",
            ),
            (
                "inherit_claude_config",
                format!("inherit_claude_config: true\ntools: [{tool}]"),
                "inherit_claude_config",
            ),
        ];
        for (name, extra, field) in rejected {
            let error = parse(&format!(
                "---\ndescription: demo\nruntime: {runtime}\n{extra}\n---\n"
            ))
            .expect_err(name);
            assert_eq!(
                error.field.as_deref(),
                Some(field),
                "{runtime} {name}: {error}"
            );
        }
        let error = parse(&format!(
            "---\ndescription: demo\nruntime: {runtime}\nprompt: replace\ntools: [{tool}]\n---\nReplacement\n"
        ))
        .expect_err("replace prompt");
        assert_eq!(error.field.as_deref(), Some("prompt"), "{runtime}: {error}");
    }
}

// Covers: `fast` is Rho-only, needs a pinned model, and round-trips through
// the serializer so the editor can save it.
// Owner: agent parser
#[test]
fn fast_requires_rho_runtime_and_pinned_model() {
    let frontmatter = |extra: &str| format!("---\ndescription: d\n{extra}\n---\nbody\n");
    for (extra, expected) in [
        (
            "model: gpt-5.5\nprovider: openai-codex\nfast: true",
            Ok(true),
        ),
        (
            "model: gpt-5.5\nprovider: openai-codex\nfast: false",
            Ok(false),
        ),
        ("model: gpt-5.5\nprovider: openai-codex", Ok(false)),
        ("fast: true", Err("fast")),
        ("model-policy: inherit\nfast: true", Err("fast")),
        ("runtime: claude-cli\nmodel: opus\nfast: true", Err("fast")),
        ("model: gpt-5.5\nfast: yes", Err("fast")),
    ] {
        let actual = parse(&frontmatter(extra))
            .map(|definition| definition.fast())
            .map_err(|error| error.field.unwrap_or_default());
        assert_eq!(actual, expected.map_err(str::to_string), "{extra}");
    }

    let parsed = parse(&frontmatter(
        "model: gpt-5.5\nprovider: openai-codex\nfast: true",
    ))
    .unwrap();
    let reparsed = parse(&crate::agent::serialize_definition(&parsed)).unwrap();
    assert_eq!(reparsed, parsed);
    assert_ne!(
        parsed.fingerprint(),
        parse(&frontmatter("model: gpt-5.5\nprovider: openai-codex"))
            .unwrap()
            .fingerprint()
    );
}
