use super::*;
use crate::workflow::{
    test_support::{agent_node, id, workflow},
    AgentNode, ObjectFieldSchema, OutputPath, OutputSchema, Template,
};

// Covers: malformed DAGs could deadlock or read data outside declared ordering.
// Owner: workflow graph validation.
#[test]
fn rejects_missing_dependencies_cycles_and_non_ancestor_references() {
    let mut invalid = workflow(vec![agent_node("a", &["gone"], WorkspaceAccess::Mutating)]);
    assert!(matches!(
        validate_workflow(&invalid),
        Err(WorkflowError::MissingDependency { .. })
    ));

    invalid = workflow(vec![
        agent_node("a", &["b"], WorkspaceAccess::Mutating),
        agent_node("b", &["a"], WorkspaceAccess::Mutating),
    ]);
    assert!(matches!(
        validate_workflow(&invalid),
        Err(WorkflowError::Cycle { .. })
    ));

    let mut a = agent_node("a", &[], WorkspaceAccess::Mutating);
    a.execution = NodeExecution::Agent(AgentNode {
        agent: "reviewer".to_owned(),
        prompt: Template(vec![TemplatePart::Output {
            reference: OutputReference {
                node: id("b"),
                path: OutputPath(vec!["result".to_owned()]),
            },
        }]),
        output: None,
    });
    let mut b = agent_node("b", &[], WorkspaceAccess::Mutating);
    if let NodeExecution::Agent(agent) = &mut b.execution {
        agent.output = Some(OutputSchema::Object {
            fields: [(
                "result".to_owned(),
                ObjectFieldSchema {
                    schema: OutputSchema::String,
                    required: true,
                },
            )]
            .into_iter()
            .collect(),
        });
    }
    assert!(matches!(
        validate_workflow(&workflow(vec![a, b])),
        Err(WorkflowError::NonAncestorReference { .. })
    ));
}
