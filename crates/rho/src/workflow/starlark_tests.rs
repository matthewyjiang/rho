use std::{
    collections::BTreeMap,
    sync::{atomic::AtomicBool, Arc},
};

use super::*;
use crate::workflow::{test_support::limits, Digest, SourceFile, SourceManifest};

fn collected(source: &str) -> CollectedSources {
    CollectedSources {
        entry_label: "//workflow.star".to_owned(),
        sources: BTreeMap::from([("//workflow.star".to_owned(), source.to_owned())]),
        manifest: SourceManifest {
            entry_label: "//workflow.star".to_owned(),
            modules: BTreeMap::from([(
                "//workflow.star".to_owned(),
                SourceFile {
                    digest: Digest("sha256:test".to_owned()),
                    bytes: source.len() as u64,
                },
            )]),
        },
    }
}

// Covers: planning could call build more than once or retain interpreter values.
// Owner: Starlark planning frontend.
#[test]
fn validates_inputs_and_builds_one_owned_graph() {
    let source = r#"
calls = []
def build(inputs):
    calls.append(inputs["target"])
    return workflow(name = "review", nodes = [agent(
        name = "inspect",
        agent = "reviewer",
        prompt = template(["Inspect ", inputs["target"]]),
        access = "read_only",
        timeout_seconds = 60,
        max_output_bytes = 4096,
    )])
WORKFLOW = define(inputs = {"target": input.string(default = ".")}, build = build)
"#;
    let limits = limits();
    let planner = StarlarkPlanner::new(&limits);
    assert_eq!(
        planner.isolation_decision(),
        IsolationDecision::SupervisedProcessRequired
    );
    let planned = planner
        .plan_in_process_prototype(
            &collected(source),
            &BTreeMap::new(),
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
    assert_eq!(planned.graph.name, WorkflowName::new("review").unwrap());
    assert_eq!(planned.graph.nodes.len(), 1);
    assert_eq!(
        planned.inputs[&InputName::new("target").unwrap()],
        WorkflowValue::String(".".to_owned())
    );
}

// Covers: cancellation must stop evaluator work rather than only its caller.
// Owner: Starlark planning frontend safety prototype.
#[test]
fn evaluator_observes_preexisting_cancellation() {
    let source = r#"
for item in range(100000000):
    value = item
WORKFLOW = define(inputs = {}, build = lambda inputs: workflow(name = "x", nodes = []))
"#;
    let limits = limits();
    let error = StarlarkPlanner::new(&limits)
        .plan_in_process_prototype(
            &collected(source),
            &BTreeMap::new(),
            Arc::new(AtomicBool::new(true)),
        )
        .unwrap_err();
    assert!(matches!(error, WorkflowError::Starlark(_)));
}

// Covers: command exit references could be parsed as untyped output values.
// Owner: Starlark planning frontend.
#[test]
fn builds_typed_command_exit_condition() {
    let source = r#"
def build(inputs):
    return workflow(name = "checks", nodes = [
        command(name = "check", argv = ["git", "status"], cwd = ".", timeout_seconds = 60, max_output_bytes = 4096),
        agent(
            name = "review",
            agent = "reviewer",
            prompt = "review",
            needs = ["check"],
            when = equals(exit_code("check"), 0),
            timeout_seconds = 60,
            max_output_bytes = 4096,
        ),
    ])
WORKFLOW = define(inputs = {}, build = build)
"#;
    let limits = limits();
    let planned = StarlarkPlanner::new(&limits)
        .plan_in_process_prototype(
            &collected(source),
            &BTreeMap::new(),
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
    assert!(matches!(
        planned.graph.nodes[&NodeId::new("review").unwrap()]
            .condition
            .as_ref(),
        Some(Condition::CommandExit {
            predicate: ExitCodePredicate::Equals(0),
            ..
        })
    ));
}
