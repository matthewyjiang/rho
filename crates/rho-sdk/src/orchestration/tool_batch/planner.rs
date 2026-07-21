use std::path::Path;

use crate::tool::{
    ToolAccessMode, ToolExecutionPolicy, ToolResource, ToolResourceAccess, ToolResourceKind,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct CallIndex(pub(super) usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SerializationReason {
    ExclusiveCall,
    ResourceConflict {
        earlier: ToolResourceKind,
        later: ToolResourceKind,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Dependency {
    pub(super) predecessor: CallIndex,
    pub(super) reason: SerializationReason,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PlannedCall {
    pub(super) index: CallIndex,
    pub(super) dependencies: Vec<Dependency>,
}

/// Builds model-order dependencies without inspecting tools or runtime state.
pub(super) fn plan(policies: &[ToolExecutionPolicy]) -> Vec<PlannedCall> {
    policies
        .iter()
        .enumerate()
        .map(|(later_index, later)| {
            let dependencies = policies[..later_index]
                .iter()
                .enumerate()
                .filter_map(|(earlier_index, earlier)| {
                    serialization_reason(earlier, later).map(|reason| Dependency {
                        predecessor: CallIndex(earlier_index),
                        reason,
                    })
                })
                .collect();
            PlannedCall {
                index: CallIndex(later_index),
                dependencies,
            }
        })
        .collect()
}

fn serialization_reason(
    earlier: &ToolExecutionPolicy,
    later: &ToolExecutionPolicy,
) -> Option<SerializationReason> {
    let (
        ToolExecutionPolicy::ResourceAware {
            accesses: earlier_accesses,
        },
        ToolExecutionPolicy::ResourceAware {
            accesses: later_accesses,
        },
    ) = (earlier, later)
    else {
        return Some(SerializationReason::ExclusiveCall);
    };

    earlier_accesses.iter().find_map(|earlier_access| {
        later_accesses.iter().find_map(|later_access| {
            accesses_conflict(earlier_access, later_access).then_some(
                SerializationReason::ResourceConflict {
                    earlier: earlier_access.resource().kind(),
                    later: later_access.resource().kind(),
                },
            )
        })
    })
}

fn accesses_conflict(earlier: &ToolResourceAccess, later: &ToolResourceAccess) -> bool {
    if matches!(
        (earlier.mode(), later.mode()),
        (ToolAccessMode::Shared, ToolAccessMode::Shared)
    ) {
        return false;
    }
    resources_overlap(earlier.resource(), later.resource())
}

fn resources_overlap(earlier: &ToolResource, later: &ToolResource) -> bool {
    match (earlier, later) {
        (ToolResource::WorkspacePath(earlier), ToolResource::WorkspacePath(later)) => {
            earlier == later
        }
        (ToolResource::DirectoryTree(tree), ToolResource::WorkspacePath(path))
        | (ToolResource::WorkspacePath(path), ToolResource::DirectoryTree(tree)) => {
            path.starts_with(tree)
        }
        (ToolResource::DirectoryTree(earlier), ToolResource::DirectoryTree(later)) => {
            paths_are_ancestors(earlier, later)
        }
        (ToolResource::DirectoryMembership(directory), ToolResource::WorkspacePath(path))
        | (ToolResource::WorkspacePath(path), ToolResource::DirectoryMembership(directory)) => {
            path == directory || path.parent() == Some(directory.as_path())
        }
        (ToolResource::DirectoryMembership(directory), ToolResource::DirectoryTree(tree))
        | (ToolResource::DirectoryTree(tree), ToolResource::DirectoryMembership(directory)) => {
            paths_are_ancestors(directory, tree)
        }
        (ToolResource::DirectoryMembership(earlier), ToolResource::DirectoryMembership(later)) => {
            earlier == later
        }
        _ => earlier == later,
    }
}

fn paths_are_ancestors(first: &Path, second: &Path) -> bool {
    first.starts_with(second) || second.starts_with(first)
}

#[cfg(test)]
#[path = "planner_tests.rs"]
mod tests;
