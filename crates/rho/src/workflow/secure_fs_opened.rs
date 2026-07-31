use std::{io::Read as _, path::Path};

use super::{
    secure_fs::{identity_drift, inspect_absolute, OpenedExecutable, VerifiedPath},
    Budget, FrozenPathIdentity, FrozenPathKind, WorkflowError, WorkflowResult,
};

impl VerifiedPath {
    pub(crate) fn open(path: &Path, hash: bool) -> WorkflowResult<Self> {
        inspect_absolute(path, FrozenPathKind::File, hash)
    }

    pub(crate) fn read_utf8(mut self) -> WorkflowResult<String> {
        let mut text = String::new();
        self.file.read_to_string(&mut text)?;
        Ok(text)
    }

    pub(crate) fn read_utf8_bounded(
        mut self,
        budget: &Budget,
        retained: u64,
    ) -> WorkflowResult<String> {
        budget.check(retained.saturating_add(self.file.metadata()?.len()))?;
        let remaining = budget.limit.saturating_sub(retained);
        let mut bytes = Vec::with_capacity(usize::try_from(remaining.min(8 * 1024)).unwrap_or(0));
        self.file
            .by_ref()
            .take(remaining.saturating_add(1))
            .read_to_end(&mut bytes)?;
        budget.check(retained.saturating_add(bytes.len() as u64))?;
        String::from_utf8(bytes).map_err(|error| WorkflowError::Starlark(error.to_string()))
    }
}

impl OpenedExecutable {
    pub(crate) fn identity(&self) -> &FrozenPathIdentity {
        &self.executable.identity
    }

    pub(crate) fn into_binary(self) -> WorkflowResult<VerifiedPath> {
        if self.interpreter_request.is_some() {
            return Err(identity_drift(
                Path::new(&self.executable.identity.canonical_path),
                "nested script interpreters are not supported for frozen execution",
            ));
        }
        Ok(self.executable)
    }
}
