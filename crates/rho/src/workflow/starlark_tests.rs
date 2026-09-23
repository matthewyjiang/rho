use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{atomic::AtomicBool, Arc},
};

use pretty_assertions::assert_eq;

use super::*;
use crate::workflow::{
    test_support::limits, Condition, Digest, ExitCodePredicate, NodeId, ObjectFieldSchema,
    OutputSchema, SourceFile, SourceManifest, WorkflowName,
};

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

// Covers: source exports must lower to typed references, not arbitrary schema dictionaries.
// Owner: Starlark compiler boundary.
#[test]
fn compiles_typed_exports_and_rejects_invalid_bindings() {
    use crate::workflow::{OutputPath, OutputReference};

    for (exports, valid) in [
        (r#"{"report title": output("report", ["field"])}"#, true),
        (r#"{"": output("report", ["field"])}"#, false),
        (r#"{"report": output("missing", [])}"#, false),
        (
            r#"{"report": output("report", ["field", "deeper"])}"#,
            false,
        ),
        (r#"{"report": output("untyped", [])}"#, false),
        (r#"{"report": schema.string()}"#, false),
        (
            r#"{"report": {"__rho_type": "output_ref", "node": "report", "path": [], "extra": True}}"#,
            false,
        ),
    ] {
        let source = format!(
            r#"
def build(inputs):
    return workflow(name = "exports", nodes = [
        agent(name = "report", agent = "reviewer", prompt = "review", access = "read_only", timeout_seconds = 5, max_output_bytes = 4096, output = schema.record({{"field": schema.string()}})),
        agent(name = "untyped", agent = "reviewer", prompt = "review", access = "read_only", timeout_seconds = 5, max_output_bytes = 4096),
    ], exports = {exports})
WORKFLOW = define(inputs = {{}}, build = build)
"#
        );
        let limits = limits();
        let result = StarlarkPlanner::new(&limits).plan_in_process_prototype(
            &collected(&source),
            &BTreeMap::new(),
            Arc::new(AtomicBool::new(false)),
        );
        assert_eq!(result.is_ok(), valid, "{exports}: {result:?}");
        if let Ok(planned) = result {
            assert_eq!(
                planned.program.root.exports,
                BTreeMap::from([(
                    "report title".to_owned().try_into().unwrap(),
                    OutputReference {
                        node: NodeId::new("report").unwrap(),
                        path: OutputPath(vec!["field".to_owned()]),
                    },
                )])
            );
        }
    }
}

// Covers: planning could call build more than once or retain interpreter values.
// Owner: Starlark planning frontend.
#[test]
fn validates_inputs_and_lowers_one_owned_root_scope() {
    let source = r#"
calls = []
def build(inputs):
    calls.append(inputs["target"])
    return workflow(name = "review", nodes = [agent(
        name = "inspect_" + str(len(calls)),
        agent = "reviewer",
        prompt = template(["Inspect ", inputs["target"]]),
        access = "read_only",
        timeout_seconds = 5,
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
    assert_eq!(planned.program.name, WorkflowName::new("review").unwrap());
    assert_eq!(planned.program.root.nodes.len(), 1);
    assert_eq!(
        planned.program.root.parameters,
        BTreeMap::from([(
            InputName::new("target").unwrap(),
            InputSchema::String {
                default: Some(".".to_owned())
            },
        )])
    );
    assert!(planned
        .program
        .root
        .nodes
        .contains_key(&NodeId::new("inspect_1").unwrap()));
    assert_eq!(
        planned.inputs[&InputName::new("target").unwrap()],
        WorkflowValue::String(".".to_owned())
    );
}

// Covers: unimplemented control-flow and scope declarations must not silently
// compile into a different, static program.
// Owner: Starlark compiler boundary.
#[test]
fn rejects_unsupported_program_shapes() {
    for built in [
        r#"{"__rho_type": "workflow", "name": "x", "nodes": [], "unexpected": {}}"#,
        r#"{"__rho_type": "workflow", "name": "x", "nodes": [{"__rho_type": "map"}]}"#,
        r#"{"__rho_type": "workflow", "name": "x", "nodes": [{"__rho_type": "iterate"}]}"#,
    ] {
        let source = format!("WORKFLOW = define(inputs = {{}}, build = lambda inputs: {built})");
        let limits = limits();
        let result = StarlarkPlanner::new(&limits).plan_in_process_prototype(
            &collected(&source),
            &BTreeMap::new(),
            Arc::new(AtomicBool::new(false)),
        );
        assert!(matches!(result, Err(WorkflowError::Starlark(_))));
    }
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

// Covers: workflow constructors must not replace standard Starlark globals.
// Owner: Starlark API registration.
#[test]
fn namespaces_condition_and_schema_constructors() {
    let source = r#"
def build(inputs):
    standard_globals_work = (
        bool(1) == True and
        list([1, 2]) == [1, 2] and
        all([True, True]) == True and
        any([False, True]) == True
    )
    check_name = "check" if standard_globals_work else "wrong"
    return workflow(name = "namespaces", nodes = [
        command(
            name = check_name,
            argv = ["git", "status"],
            timeout_seconds = 5,
            max_output_bytes = 4096,
        ),
        agent(
            name = "review",
            agent = "reviewer",
            prompt = "review",
            needs = [check_name],
            when = condition.all([
                condition.is_one_of(exit_code(check_name), [0, 1]),
            ]),
            output = schema.record({
                "enabled": schema.bool(),
                "items": schema.list(schema.integer()),
                "note": schema.optional(schema.string()),
            }),
            timeout_seconds = 5,
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

    assert!(planned
        .program
        .root
        .nodes
        .contains_key(&NodeId::new("check").unwrap()));
    let review = &planned.program.root.nodes[&NodeId::new("review").unwrap()];
    assert!(matches!(
        review.condition.as_ref(),
        Some(Condition::All { conditions })
            if matches!(
                conditions.as_slice(),
                [Condition::CommandExit {
                    predicate: ExitCodePredicate::IsOneOf(values),
                    ..
                }] if values == &BTreeSet::from([0, 1])
            )
    ));
    assert_eq!(
        review.output_schema(),
        Some(&OutputSchema::Object {
            fields: BTreeMap::from([
                (
                    "enabled".to_owned(),
                    ObjectFieldSchema {
                        schema: OutputSchema::Bool,
                        required: true,
                    },
                ),
                (
                    "items".to_owned(),
                    ObjectFieldSchema {
                        schema: OutputSchema::List {
                            item: Box::new(OutputSchema::Integer),
                        },
                        required: true,
                    },
                ),
                (
                    "note".to_owned(),
                    ObjectFieldSchema {
                        schema: OutputSchema::String,
                        required: false,
                    },
                ),
            ]),
        })
    );
}

// Covers: command exit references could be parsed as untyped output values.
// Owner: Starlark planning frontend.
#[test]
fn builds_typed_command_exit_condition() {
    let source = r#"
def build(inputs):
    return workflow(name = "checks", nodes = [
        command(name = "check", argv = ["git", "status"], cwd = ".", timeout_seconds = 5, max_output_bytes = 4096),
        agent(
            name = "review",
            agent = "reviewer",
            prompt = "review",
            needs = ["check"],
            when = condition.equals(exit_code("check"), 0),
            timeout_seconds = 5,
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
        planned.program.root.nodes[&NodeId::new("review").unwrap()]
            .condition
            .as_ref(),
        Some(Condition::CommandExit {
            predicate: ExitCodePredicate::Equals(0),
            ..
        })
    ));
}
