//! Shared result shape for workspace file mutations (`write`, `edit`).

/// Model-facing content plus UI metadata for a completed file mutation.
#[derive(Debug)]
pub(crate) struct FileMutationOutcome {
    pub content: String,
    /// Display paths touched by the mutation, in document order.
    pub display_paths: Vec<String>,
    /// Unified diff for UI cards (not repeated in model-facing content).
    pub diff: String,
}
