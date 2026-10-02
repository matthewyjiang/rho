use std::path::Path;

use crate::tool::{
    ToolAccessMode, ToolExecutionPolicy, ToolResource, ToolResourceAccess, ToolResourceKind,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SerializationReason {
    ExclusiveCall,
    ResourceConflict {
        earlier: ToolResourceKind,
        later: ToolResourceKind,
    },
}

pub(crate) fn serialization_reason(
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
