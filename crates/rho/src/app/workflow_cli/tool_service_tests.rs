use std::{collections::BTreeMap, path::Path};

use pretty_assertions::assert_eq;
use rho_sdk::{CapabilityKind, CapabilityOperation, PathScope};

use super::{AppWorkflowToolService, WorkflowToolRequest};

fn service() -> AppWorkflowToolService {
    AppWorkflowToolService {
        cwd: "/workspace".into(),
        config_path: None,
    }
}

// Covers: each model workflow action must declare its durable and process
// authority before service dispatch, including explicit recovery authority.
// Owner: workflow application tool adapter.
#[test]
fn action_preparation_declares_exact_capabilities() {
    let cases = [
        (
            WorkflowToolRequest::Validate {
                file: "flow.star".into(),
                inputs: BTreeMap::new(),
            },
            vec![CapabilityKind::Read, CapabilityKind::Process],
        ),
        (
            WorkflowToolRequest::Plan {
                file: "flow.star".into(),
                inputs: BTreeMap::new(),
            },
            vec![
                CapabilityKind::Read,
                CapabilityKind::Process,
                CapabilityKind::Write,
            ],
        ),
        (
            WorkflowToolRequest::Run {
                plan_id: "00000000-0000-0000-0000-000000000001".into(),
            },
            vec![CapabilityKind::Read, CapabilityKind::Write],
        ),
        (
            WorkflowToolRequest::Status {
                run_id: "00000000-0000-0000-0000-000000000002".into(),
            },
            vec![CapabilityKind::Read],
        ),
        (
            WorkflowToolRequest::Cancel {
                run_id: "00000000-0000-0000-0000-000000000002".into(),
            },
            vec![CapabilityKind::Read, CapabilityKind::Write],
        ),
        (
            WorkflowToolRequest::Resume {
                run_id: "00000000-0000-0000-0000-000000000002".into(),
                recover_uncertain: true,
            },
            vec![CapabilityKind::Read, CapabilityKind::Write],
        ),
    ];
    for (request, expected) in cases {
        let capabilities = service()
            .capabilities_for_paths(&request, Path::new("/rho"), Path::new("/bin/rho"))
            .unwrap();
        assert_eq!(
            capabilities
                .iter()
                .map(|request| request.kind())
                .collect::<Vec<_>>(),
            expected
        );
        assert!(capabilities.iter().all(
            |request| request.source() == &rho_sdk::CapabilitySource::built_in_tool("workflow")
        ));
    }
}

// Covers: action preparation must bind approval to the exact durable target
// and exact planner process facts, not only to a broad capability class.
// Owner: workflow application tool adapter.
#[test]
fn preparation_keeps_exact_durable_and_process_facts() {
    let plan = service()
        .capabilities_for_paths(
            &WorkflowToolRequest::Plan {
                file: "flow.star".into(),
                inputs: BTreeMap::new(),
            },
            Path::new("/rho"),
            Path::new("/bin/rho"),
        )
        .unwrap();
    assert!(matches!(
        plan[0].operation(),
        CapabilityOperation::ReadPath { path, scope }
            if path == Path::new("/workspace/flow.star") && *scope == PathScope::PrimaryWorkspace
    ));
    assert!(matches!(
        plan[1].operation(),
        CapabilityOperation::ExecuteProcess(process)
            if process.invocation().executable_path() == Path::new("/bin/rho")
                && process.invocation().arguments() == ["workflow", "validate", "worker.star"]
    ));
    assert!(matches!(
        plan[2].operation(),
        CapabilityOperation::WritePath { path, scope }
            if path == Path::new("/rho/workflows/plans")
                && *scope == PathScope::UnrestrictedFilesystem
    ));

    let recovery = service()
        .capabilities_for_paths(
            &WorkflowToolRequest::Resume {
                run_id: "00000000-0000-0000-0000-000000000002".into(),
                recover_uncertain: true,
            },
            Path::new("/rho"),
            Path::new("/bin/rho"),
        )
        .unwrap();
    assert!(matches!(
        recovery[1].operation(),
        CapabilityOperation::WritePath { path, .. }
            if path == Path::new("/rho/workflows/runs/00000000-0000-0000-0000-000000000002")
    ));
    assert!(service()
        .capabilities_for_paths(
            &WorkflowToolRequest::Validate {
                file: "../outside.star".into(),
                inputs: BTreeMap::new(),
            },
            Path::new("/rho"),
            Path::new("/bin/rho"),
        )
        .is_err());
}
