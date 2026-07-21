use pretty_assertions::assert_eq;

use super::{plan, CallIndex, Dependency, PlannedCall, SerializationReason};
use crate::tool::{ToolExecutionPolicy, ToolResource, ToolResourceAccess, ToolResourceKind};

fn aware(accesses: impl IntoIterator<Item = ToolResourceAccess>) -> ToolExecutionPolicy {
    ToolExecutionPolicy::resource_aware(accesses)
}

fn dependency(predecessor: usize, reason: SerializationReason) -> Dependency {
    Dependency {
        predecessor: CallIndex(predecessor),
        reason,
    }
}

fn planned(index: usize, dependencies: Vec<Dependency>) -> PlannedCall {
    PlannedCall {
        index: CallIndex(index),
        dependencies,
    }
}

#[test]
fn shared_accesses_and_empty_plans_can_overlap() {
    let shared = ToolResourceAccess::shared(ToolResource::workspace_path("/workspace/file"));

    assert_eq!(
        plan(&[aware([shared.clone()]), aware([shared]), aware([]),]),
        vec![planned(0, vec![]), planned(1, vec![]), planned(2, vec![])]
    );
}

#[test]
fn conflicting_resources_depend_on_every_earlier_conflict_in_model_order() {
    let path = || ToolResource::workspace_path("/workspace/file");
    let policies = [
        aware([ToolResourceAccess::exclusive(path())]),
        aware([ToolResourceAccess::shared(path())]),
        aware([ToolResourceAccess::exclusive(path())]),
    ];
    let conflict = SerializationReason::ResourceConflict {
        earlier: ToolResourceKind::WorkspacePath,
        later: ToolResourceKind::WorkspacePath,
    };

    assert_eq!(
        plan(&policies),
        vec![
            planned(0, vec![]),
            planned(1, vec![dependency(0, conflict)]),
            planned(2, vec![dependency(0, conflict), dependency(1, conflict)]),
        ]
    );
}

#[test]
fn distinct_resource_identities_do_not_conflict() {
    let policies = [
        aware([ToolResourceAccess::exclusive(
            ToolResource::managed_process("first"),
        )]),
        aware([ToolResourceAccess::exclusive(
            ToolResource::managed_process("second"),
        )]),
        aware([ToolResourceAccess::exclusive(ToolResource::opaque(
            "tool-a", "same-key",
        ))]),
        aware([ToolResourceAccess::exclusive(ToolResource::opaque(
            "tool-b", "same-key",
        ))]),
    ];

    assert!(plan(&policies)
        .iter()
        .all(|call| call.dependencies.is_empty()));
}

#[test]
fn exclusive_calls_are_model_order_barriers() {
    let policies = [
        aware([]),
        ToolExecutionPolicy::Exclusive,
        aware([]),
        aware([ToolResourceAccess::shared(ToolResource::session_state())]),
    ];

    assert_eq!(
        plan(&policies),
        vec![
            planned(0, vec![]),
            planned(1, vec![dependency(0, SerializationReason::ExclusiveCall)]),
            planned(2, vec![dependency(1, SerializationReason::ExclusiveCall)]),
            planned(3, vec![dependency(1, SerializationReason::ExclusiveCall)]),
        ]
    );
}

#[test]
fn directory_trees_conflict_with_descendants_and_ancestor_trees() {
    let policies = [
        aware([ToolResourceAccess::shared(ToolResource::directory_tree(
            "/workspace/src",
        ))]),
        aware([ToolResourceAccess::exclusive(ToolResource::workspace_path(
            "/workspace/src/new.rs",
        ))]),
        aware([ToolResourceAccess::exclusive(ToolResource::directory_tree(
            "/workspace",
        ))]),
        aware([ToolResourceAccess::exclusive(ToolResource::workspace_path(
            "/workspace/tests/test.rs",
        ))]),
    ];

    let calls = plan(&policies);

    assert_eq!(
        calls[1].dependencies,
        [dependency(
            0,
            SerializationReason::ResourceConflict {
                earlier: ToolResourceKind::DirectoryTree,
                later: ToolResourceKind::WorkspacePath,
            }
        )]
    );
    assert_eq!(
        calls[2].dependencies,
        [
            dependency(
                0,
                SerializationReason::ResourceConflict {
                    earlier: ToolResourceKind::DirectoryTree,
                    later: ToolResourceKind::DirectoryTree,
                }
            ),
            dependency(
                1,
                SerializationReason::ResourceConflict {
                    earlier: ToolResourceKind::WorkspacePath,
                    later: ToolResourceKind::DirectoryTree,
                }
            ),
        ]
    );
    assert_eq!(calls[3].dependencies.len(), 1);
    assert_eq!(calls[3].dependencies[0].predecessor, CallIndex(2));
}

#[test]
fn directory_membership_conflicts_with_direct_children_but_not_deeper_paths() {
    let policies = [
        aware([ToolResourceAccess::shared(
            ToolResource::directory_membership("/workspace/src"),
        )]),
        aware([ToolResourceAccess::exclusive(ToolResource::workspace_path(
            "/workspace/src/new.rs",
        ))]),
        aware([ToolResourceAccess::exclusive(ToolResource::workspace_path(
            "/workspace/src/nested/new.rs",
        ))]),
    ];

    let calls = plan(&policies);

    assert_eq!(calls[1].dependencies.len(), 1);
    assert!(calls[2].dependencies.is_empty());
}
