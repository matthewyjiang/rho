use std::path::Path;

use pretty_assertions::assert_eq;

use super::serialize_definition;
use crate::agent::{
    parse_definition, AgentDefinition, AgentId, AgentRuntimeSpec, ClaudeAgentConfig,
    ClaudeToolPolicy, ModelPolicy, ModelSelection, PromptPolicy, ReasoningLevel, ToolCapability,
    ToolCapabilitySet, ToolPolicy,
};

fn parse(contents: &str) -> AgentDefinition {
    parse_definition(Path::new("agent.md"), "agent", contents).expect("fixture parses")
}

// Covers: every supported runtime and policy shape survives canonical file serialization.
// Owner: agent definition serializer
#[test]
fn supported_definitions_round_trip() {
    let cases = [
        "---\nid: demo\ndescription: demo\ntools: [read_file, write, shell]\n---\nDo the work.\n",
        "---\ndescription: demo\n---\nbody\n",
        "---\ndescription: demo\nmodel-policy: select\nmodel: gpt-5.5\nprovider: openai\nreasoning: high\n---\nbody\n",
        "---\ndescription: demo\nmodel-policy: prefer\nmodel: claude-opus-5\n---\nbody\n",
        "---\ndescription: demo\nmodel-policy: require\nmodel: sonnet\nprovider: anthropic\n---\nbody\n",
        "---\ndescription: demo\nruntime: claude-cli\nmodel: claude-opus-4-6\ntools: [Read, Edit, \"Bash(git *)\"]\n---\nbody\n",
        "---\ndescription: demo\nruntime: claude-cli\ninherit_claude_config: true\nreasoning: high\ntools: [Read]\n---\nbody\n",
        "---\ndescription: demo\nruntime: claude-cli\ntools: []\n---\nbody\n",
        "---\ndescription: demo\nprompt: replace\n---\nReplacement body.\n",
    ];

    for contents in cases {
        let first = parse(contents);
        let serialized = serialize_definition(&first);
        let second = parse(&serialized);
        assert_eq!(first, second, "round trip changed:\n{serialized}");
        assert_eq!(serialized, serialize_definition(&second));
    }
}

// Covers: an empty extend body remains empty instead of gaining prompt text.
// Owner: agent definition serializer
#[test]
fn empty_extend_body_stays_empty() {
    let definition = AgentDefinition {
        id: AgentId::new("demo").unwrap(),
        description: "demo".into(),
        prompt: PromptPolicy::Extend(String::new()),
        runtime: AgentRuntimeSpec::Rho {
            tools: ToolPolicy::All,
            model: ModelPolicy::Inherit,
            reasoning: None,
        },
    };

    let serialized = serialize_definition(&definition);

    assert!(serialized.ends_with("---\n"));
    assert!(!serialized.ends_with("---\n\n"));
    assert_eq!(definition, parse(&serialized));
}

// Covers: Claude tool patterns that need list quoting remain one tool after parsing.
// Owner: agent definition serializer
#[test]
fn quotes_claude_tool_patterns() {
    let definition = AgentDefinition {
        id: AgentId::new("demo").unwrap(),
        description: "demo".into(),
        prompt: PromptPolicy::Extend("body".into()),
        runtime: AgentRuntimeSpec::ClaudeCli(ClaudeAgentConfig {
            tools: ClaudeToolPolicy::Allow(vec!["Bash(git *)".into()]),
            inherit_claude_config: false,
            model: None,
            reasoning: None,
        }),
    };

    let serialized = serialize_definition(&definition);

    assert!(serialized.contains(r#"tools: ["Bash(git *)"]"#));
    assert_eq!(definition, parse(&serialized));
}

// Covers: multiline Markdown body content is not flattened or trimmed.
// Owner: agent definition serializer
#[test]
fn preserves_multiline_body() {
    let body = "First line.\n\nSecond paragraph.\n- bullet\n- bullet\n";
    let definition = parse(&format!("---\ndescription: demo\n---\n{body}"));

    let serialized = serialize_definition(&definition);

    assert!(serialized.ends_with(body));
    assert_eq!(definition, parse(&serialized));
}

// Covers: deterministic tool ordering and provider emission in canonical output.
// Owner: agent definition serializer
#[test]
fn canonicalizes_ordered_fields() {
    let definition = AgentDefinition {
        id: AgentId::new("demo").unwrap(),
        description: "demo".into(),
        prompt: PromptPolicy::Extend("body".into()),
        runtime: AgentRuntimeSpec::Rho {
            tools: ToolPolicy::Allow(
                [
                    ToolCapability::ReadFile,
                    ToolCapability::Bash,
                    ToolCapability::Grep,
                ]
                .into_iter()
                .collect::<ToolCapabilitySet>(),
            ),
            model: ModelPolicy::Select(ModelSelection {
                provider: Some("openai".into()),
                model: "model-x".into(),
                auth: None,
            }),
            reasoning: Some(ReasoningLevel::Low),
        },
    };

    let serialized = serialize_definition(&definition);

    assert!(serialized.contains("model: model-x\nprovider: openai\nreasoning: low"));
    assert!(serialized.contains("tools: [bash, grep, read_file]"));
    assert_eq!(definition, parse(&serialized));
}
