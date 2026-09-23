use std::collections::BTreeMap;

use pretty_assertions::assert_eq;

use crate::workflow::*;

use super::{PreparedExecution, PreparedInvocation};

fn node_id(value: &str) -> NodeId {
    NodeId::new(value).unwrap()
}

fn agent_leaf() -> (Node, ResolvedNode) {
    let node = Node {
        id: node_id("inspect"),
        display_name: "inspect".into(),
        needs: vec![node_id("source")],
        condition: None,
        execution: NodeExecution::Agent(AgentNode {
            agent: "reviewer".into(),
            prompt: Template(vec![TemplatePart::Output {
                reference: OutputReference {
                    node: node_id("source"),
                    path: OutputPath(vec!["text".into()]),
                },
            }]),
            output: Some(OutputSchema::Bool),
        }),
        access: WorkspaceAccess::ReadOnly,
        allow_failure: false,
        timeout_seconds: 5,
        max_output_bytes: 1024,
    };
    let agent = ResolvedNode::Agent(Box::new(ResolvedAgent {
        agent_id: "frozen-reviewer".into(),
        fingerprint: "frozen-fingerprint".into(),
        runtime: AgentRuntime::Rho,
        source_origin: "builtin".into(),
        trust_required: false,
        prompt_policy: "review".into(),
        provider: None,
        model: None,
        reasoning: None,
        step_limit: 1,
        capabilities: Default::default(),
        permission_ceiling: "auto".into(),
        auth_profile: None,
        executable: None,
        executable_identity: None,
        arguments: Vec::new(),
    }));
    (node, agent)
}

fn outputs() -> BTreeMap<NodeId, WorkflowValue> {
    BTreeMap::from([(
        node_id("source"),
        WorkflowValue::from_json(serde_json::json!({"text": "bound text"})).unwrap(),
    )])
}

// Covers: binding snapshots frozen authority and the selected referenced value;
// later scheduler mutations cannot change a queued invocation.
// Owner: workflow invocation preparation.
#[test]
fn preparation_snapshots_selected_leaf_and_rejects_missing_bindings() {
    let (mut node, mut resolved) = agent_leaf();
    let mut outputs = outputs();
    let limits = crate::workflow::test_support::runtime_limits();
    let prepared =
        PreparedInvocation::prepare_node(Leaf::new(&node, &resolved).unwrap(), &limits, &outputs)
            .unwrap();
    let expected_agent = resolved.clone();
    let NodeExecution::Agent(agent_node) = &mut node.execution else {
        unreachable!()
    };
    agent_node.prompt = Template(vec![]);
    let ResolvedNode::Agent(agent) = &mut resolved else {
        unreachable!()
    };
    agent.agent_id = "changed".into();
    outputs.clear();

    let PreparedExecution::Agent(super::AgentInvocation { agent, prompt }) = prepared.execution
    else {
        panic!("expected agent")
    };
    assert_eq!(ResolvedNode::Agent(agent), expected_agent);
    // The prefix is bound data, not instructional copy.
    assert_eq!(prompt.lines().next(), Some("bound text"));
    assert_eq!(
        (
            prepared.output,
            prepared.timeout_seconds,
            prepared.max_output_bytes
        ),
        (
            Some(OutputSchema::Bool),
            node.timeout_seconds,
            node.max_output_bytes
        )
    );
    let (node, resolved) = agent_leaf();
    assert!(matches!(
        PreparedInvocation::prepare_node(Leaf::new(&node, &resolved).unwrap(), &limits, &outputs),
        Err(super::RuntimeError::Data(_))
    ));
}

// Covers: schema suffixes and expanded argv must count toward existing frozen
// budgets before launch. Owner: workflow invocation preparation.
#[test]
fn preparation_enforces_expansion_budgets() {
    let (node, resolved) = agent_leaf();
    let outputs = outputs();
    let mut limits = crate::workflow::test_support::runtime_limits();
    let prepared =
        PreparedInvocation::prepare_node(Leaf::new(&node, &resolved).unwrap(), &limits, &outputs)
            .unwrap();
    let PreparedExecution::Agent(super::AgentInvocation { prompt, .. }) = prepared.execution else {
        panic!("expected agent")
    };
    // Size the tripwire from the real prepared prompt, including its schema.
    limits.prompt_expansion_bytes = prompt.len() as u64 - 1;
    let error =
        PreparedInvocation::prepare_node(Leaf::new(&node, &resolved).unwrap(), &limits, &outputs)
            .err()
            .unwrap();
    assert!(
        matches!(error, super::RuntimeError::Workflow(WorkflowError::BudgetExceeded { budget: "prompt expansion bytes", limit, actual }) if limit == limits.prompt_expansion_bytes && actual == prompt.len() as u64)
    );

    let command = CommandNode::Direct {
        executable: "unresolved-name".into(),
        arguments: vec![Template(vec![TemplatePart::Output {
            reference: OutputReference {
                node: node_id("source"),
                path: OutputPath(vec!["text".into()]),
            },
        }])],
        cwd: ".".into(),
        output: None,
    };
    let executable = std::path::Path::new("/frozen/tool");
    let expected_bytes =
        executable.as_os_str().as_encoded_bytes().len() as u64 + "bound text".len() as u64;
    limits.argv_expansion_bytes = expected_bytes;
    let invocation = super::invocation(&command, executable, &outputs, &limits).unwrap();
    assert_eq!(
        (invocation.executable_path(), invocation.arguments()),
        (executable, &["bound text".to_owned()][..])
    );
    limits.argv_expansion_bytes -= 1;
    let error = super::invocation(&command, executable, &outputs, &limits)
        .err()
        .unwrap();
    assert!(
        matches!(error, super::RuntimeError::Workflow(WorkflowError::BudgetExceeded { budget: "argv expansion bytes", limit, actual }) if limit == limits.argv_expansion_bytes && actual == expected_bytes)
    );
}
