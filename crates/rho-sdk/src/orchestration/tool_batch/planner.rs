use crate::tool::{
    scheduling::{serialization_reason, SerializationReason},
    ToolExecutionPolicy,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct CallIndex(pub(super) usize);

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

#[cfg(test)]
#[path = "planner_tests.rs"]
mod tests;
