use std::{io::Read as _, path::Path};

use super::{
    secure_fs::{identity_drift, inspect_absolute, OpenedExecutable, VerifiedPath},
    Budget, FrozenPathIdentity, FrozenPathKind, WorkflowError, WorkflowResult,
};

pub(crate) fn open_verified_file(path: &Path, hash: bool) -> WorkflowResult<VerifiedPath> {
    inspect_absolute(path, FrozenPathKind::File, hash)
}

pub(crate) fn read_opened_utf8(mut opened: VerifiedPath) -> WorkflowResult<String> {
    let mut text = String::new();
    opened.file.read_to_string(&mut text)?;
    Ok(text)
}

pub(crate) fn read_opened_utf8_bounded(
    mut opened: VerifiedPath,
    budget: &Budget,
    retained: u64,
) -> WorkflowResult<String> {
    budget.check(retained.saturating_add(opened.file.metadata()?.len()))?;
    let remaining = budget.limit.saturating_sub(retained);
    let mut bytes = Vec::with_capacity(usize::try_from(remaining.min(8 * 1024)).unwrap_or(0));
    opened
        .file
        .by_ref()
        .take(remaining.saturating_add(1))
        .read_to_end(&mut bytes)?;
    budget.check(retained.saturating_add(bytes.len() as u64))?;
    String::from_utf8(bytes).map_err(|error| WorkflowError::Starlark(error.to_string()))
}

pub(crate) fn opened_binary(opened: OpenedExecutable) -> WorkflowResult<VerifiedPath> {
    if opened.interpreter_request.is_some() {
        return Err(identity_drift(
            Path::new(&opened.executable.identity.canonical_path),
            "nested script interpreters are not supported for frozen execution",
        ));
    }
    Ok(opened.executable)
}

pub(crate) fn opened_executable_identity(opened: &OpenedExecutable) -> &FrozenPathIdentity {
    &opened.executable.identity
}
