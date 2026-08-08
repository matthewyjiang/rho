//! Shared workspace mutation observation for SDK filesystem adapters.

use std::sync::Arc;

use rho_sdk::tool::{OperationKind, ToolError, ToolErrorKind, ToolMetadata, ToolOutput};

use crate::{
    file_mutation::FileMutationOutcome, sdk_support::map_app_error, tool::ToolError as AppToolError,
};

pub(super) async fn run_observed_mutation<T>(
    observer: Option<&Arc<dyn crate::WorkspaceMutationObserver>>,
    paths: &[&std::path::Path],
    op: impl std::future::Future<Output = Result<T, AppToolError>>,
) -> Result<T, ToolError> {
    if let Some(observer) = observer {
        observer
            .before_mutation(paths)
            .await
            .map_err(|error| ToolError::new(ToolErrorKind::Execution, error))?;
    }
    let op_result = op.await.map_err(map_app_error);
    let capture_result = match observer {
        Some(observer) => observer
            .after_mutation(paths)
            .await
            .map_err(|error| ToolError::new(ToolErrorKind::Execution, error)),
        None => Ok(()),
    };
    match (op_result, capture_result) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(capture_error)) => Err(ToolError::new(
            ToolErrorKind::Execution,
            format!(
                "mutation succeeded but capturing the resulting workspace state failed: {capture_error}"
            ),
        )),
        (Err(op_error), Err(capture_error)) => Err(ToolError::new(
            ToolErrorKind::Execution,
            format!("{op_error}; failed to capture resulting workspace state: {capture_error}"),
        )),
    }
}

pub(super) fn mutation_output(outcome: FileMutationOutcome) -> ToolOutput {
    let mut metadata = ToolMetadata::new()
        .operation(OperationKind::Write)
        .diff(outcome.diff);
    for path in outcome.display_paths {
        metadata = metadata.affected_path(path);
    }
    ToolOutput::text(outcome.content).metadata(metadata)
}
