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
