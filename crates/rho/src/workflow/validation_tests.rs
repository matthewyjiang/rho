use pretty_assertions::assert_eq;

use super::*;
use crate::workflow::{
    test_support::{agent_node, id, workflow},
    AgentNode, ObjectFieldSchema, OutputPath, OutputSchema, Template,
};

// Covers: persisted bindings cannot bypass export validation; missing values must
// not become null or successful partial outputs at scope closure.
// Owner: frozen scope contract.
#[test]
fn validates_and_resolves_required_scope_exports() {
    use crate::workflow::WorkflowValue;

    let mut node = agent_node("report", &[], WorkspaceAccess::ReadOnly);
    if let NodeExecution::Agent(agent) = &mut node.execution {
        agent.output = Some(OutputSchema::Object {
            fields: BTreeMap::from([(
                "field".to_owned(),
                ObjectFieldSchema {
                    schema: OutputSchema::Null,
                    required: false,
                },
            )]),
        });
    }
    let mut frozen = workflow(vec![node]);
    frozen.program.root.exports.insert(
        "result".to_owned().try_into().unwrap(),
        OutputReference {
            node: id("report"),
            path: OutputPath(vec!["field".to_owned()]),
        },
    );
    validate_workflow(&frozen).unwrap();
    for (outputs, expected) in [
        (BTreeMap::new(), None),
        (
            BTreeMap::from([(id("report"), WorkflowValue::Object(BTreeMap::new()))]),
            None,
        ),
        (
            BTreeMap::from([(
                id("report"),
                WorkflowValue::Object(BTreeMap::from([("field".to_owned(), WorkflowValue::Null)])),
            )]),
            Some(BTreeMap::from([("result".to_owned(), WorkflowValue::Null)])),
        ),
    ] {
        assert_eq!(
            frozen.program.root.resolve_exports(&outputs).unwrap(),
            expected
        );
    }
    let wrong_type = BTreeMap::from([(
        id("report"),
        WorkflowValue::Object(BTreeMap::from([(
            "field".to_owned(),
            WorkflowValue::Bool(false),
        )])),
    )]);
    assert!(matches!(
        frozen.program.root.resolve_exports(&wrong_type),
        Err(WorkflowError::Schema { .. })
    ));
    for (name, node, path) in [
        ("result", "unknown", vec![]),
        ("result", "report", vec!["field", "nested"]),
    ] {
        frozen.program.root.exports = BTreeMap::from([(
            name.to_owned().try_into().unwrap(),
            OutputReference {
                node: id(node),
                path: OutputPath(path.into_iter().map(str::to_owned).collect()),
            },
        )]);
        assert!(matches!(
            validate_workflow(&frozen),
            Err(WorkflowError::Schema { .. })
        ));
    }
}

// Covers: aliases cannot amplify a retained source beyond the scope-result byte
// budget. Owner: plan-time export bounds; fixture sizes are measured JSON.
#[test]
fn export_aliases_are_bounded_before_materializing_the_result() {
    use crate::workflow::{
        scope_result, NodeState, NodeTerminalState, ScopeInstanceId, ScopeResult, WorkflowOutcome,
        WorkflowState, WorkflowValue,
    };

    let mut node = agent_node("report", &[], WorkspaceAccess::ReadOnly);
    let NodeExecution::Agent(agent) = &mut node.execution else {
        unreachable!()
    };
    agent.output = Some(OutputSchema::String);
    let mut frozen = workflow(vec![node]);
    let value = WorkflowValue::String("retained \"value\"\n".to_owned());
    let expected = BTreeMap::from([
        ("first\"alias".to_owned(), value.clone()),
        ("second_alias".to_owned(), value.clone()),
    ]);
    for name in expected.keys() {
        frozen.program.root.exports.insert(
            name.clone().try_into().unwrap(),
            OutputReference {
                node: id("report"),
                path: OutputPath(vec![]),
            },
        );
    }
    let measured_source_bytes = serde_json::to_vec(&value).unwrap().len() as u64;
    let measured_result_bytes = serde_json::to_vec(&expected).unwrap().len() as u64;
    frozen
        .program
        .root
        .nodes
        .get_mut(&id("report"))
        .unwrap()
        .max_output_bytes = measured_source_bytes;
    let mut state = WorkflowState::new(&frozen);
    state.root_scope_mut().nodes.insert(
        id("report"),
        NodeState::Terminal {
            outcome: NodeTerminalState::Success,
        },
    );
    state.root_scope_mut().outputs.insert(id("report"), value);
    for limit in [measured_source_bytes, measured_result_bytes - 1] {
        frozen.runtime_limits.retained_output_total_bytes = limit;
        assert!(matches!(validate_workflow(&frozen),
            Err(WorkflowError::BudgetExceeded { budget: "scope export bytes", limit: actual_limit, actual })
                if actual_limit == limit && actual == measured_result_bytes
        ));
    }
    frozen.runtime_limits.retained_output_total_bytes = measured_result_bytes;
    validate_workflow(&frozen).unwrap();
    assert_eq!(
        scope_result(&frozen, &state, ScopeInstanceId::ROOT).unwrap(),
        Some(ScopeResult {
            outcome: WorkflowOutcome::Success,
            outputs: expected,
        })
    );
    // Missing required bindings return absence without materializing a result.
    assert_eq!(
        frozen
            .program
            .root
            .resolve_exports(&BTreeMap::new())
            .unwrap(),
        None
    );
}

// Covers: a persisted program must not accept missing, extra, or mistyped root
// bindings even when it bypasses the Starlark input validator.
// Owner: frozen program validation.
#[test]
fn validates_exact_typed_root_bindings() {
    use crate::workflow::{InputName, InputSchema, WorkflowValue};

    let target = InputName::new("target").unwrap();
    let parameters = BTreeMap::from([(target.clone(), InputSchema::String { default: None })]);
    for (inputs, valid) in [
        (BTreeMap::new(), false),
        (
            BTreeMap::from([(target.clone(), WorkflowValue::Bool(true))]),
            false,
        ),
        (
            BTreeMap::from([(
                InputName::new("other").unwrap(),
                WorkflowValue::String(".".to_owned()),
            )]),
            false,
        ),
        (
            BTreeMap::from([(target, WorkflowValue::String(".".to_owned()))]),
            true,
        ),
    ] {
        let mut frozen = workflow(vec![agent_node("inspect", &[], WorkspaceAccess::ReadOnly)]);
        frozen.program.root.parameters = parameters.clone();
        frozen.inputs = inputs;
        assert_eq!(validate_workflow(&frozen).is_ok(), valid);
    }
}

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

// Covers: source-controlled runtime values must not bypass fixed allocation and time budgets.
// Owner: frozen workflow runtime-budget validation.
#[test]
fn rejects_zero_and_over_budget_runtime_values() {
    let limits = crate::workflow::test_support::limits();
    let cases = [
        ("node timeout seconds", 0, 1),
        (
            "node timeout seconds",
            limits.node_timeout_seconds.limit + 1,
            1,
        ),
        ("retained output bytes per stream", 1, 0),
        (
            "retained output bytes per stream",
            1,
            limits.retained_output_per_stream_bytes.limit + 1,
        ),
    ];
    for (budget, timeout, output) in cases {
        let mut workflow = workflow(vec![agent_node("a", &[], WorkspaceAccess::Mutating)]);
        workflow
            .program
            .root
            .nodes
            .get_mut(&id("a"))
            .unwrap()
            .timeout_seconds = timeout;
        workflow
            .program
            .root
            .nodes
            .get_mut(&id("a"))
            .unwrap()
            .max_output_bytes = output;
        assert!(matches!(
            validate_runtime_budgets(&workflow, &limits),
            Err(WorkflowError::BudgetExceeded { budget: found, .. }) if found == budget
        ));
    }
}
