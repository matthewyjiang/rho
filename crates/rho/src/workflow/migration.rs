use super::{WorkflowError, WorkflowResult};

pub(crate) fn check_schema_version(
    kind: &'static str,
    found: u32,
    supported: u32,
) -> WorkflowResult<()> {
    if found == supported {
        Ok(())
    } else {
        Err(WorkflowError::UnsupportedVersion {
            kind,
            found,
            supported,
        })
    }
}
