//! The `#!` line of a frozen workflow executable: its interpreter, found on
//! the `PATH` of the environment the run starts in for `#!/usr/bin/env`
//! (#1659).

use std::{
    ffi::OsStr,
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

use super::{secure_fs::identity_drift, WorkflowResult};

/// The interpreter line of a script: `#!<path> <arguments>`.
pub(super) struct Shebang {
    pub(super) path: PathBuf,
    pub(super) arguments: Vec<String>,
}

/// One interpreter path fact needed before a script identity can be frozen.
pub(crate) enum ExecutableInterpreterRequest {
    /// `#!<absolute path>`.
    Absolute(PathBuf),
    /// `#!/usr/bin/env <program>`: `program`, to be found on a `PATH`.
    Search(String),
}

/// The interpreter `file`'s shebang asks for and the arguments after it,
/// if `file` has one. `#!/usr/bin/env` takes exactly a program name, no
/// options; any other interpreter must be absolute.
pub(super) fn interpreter_request(
    file: &File,
) -> WorkflowResult<Option<(ExecutableInterpreterRequest, Vec<String>)>> {
    let Some(spec) = raw_shebang(file)? else {
        return Ok(None);
    };
    if spec.path.file_name().is_some_and(|name| name == "env") {
        let mut arguments = spec.arguments.into_iter();
        let target = arguments.next().ok_or_else(|| {
            identity_drift(Path::new("<script>"), "env shebang has no interpreter")
        })?;
        if target.starts_with('-') {
            return Err(identity_drift(
                &spec.path,
                "env shebang options are not supported for frozen execution",
            ));
        }
        return Ok(Some((
            ExecutableInterpreterRequest::Search(target),
            arguments.collect(),
        )));
    }
    if !spec.path.is_absolute() {
        return Err(identity_drift(
            &spec.path,
            "shebang must name an absolute interpreter",
        ));
    }
    Ok(Some((
        ExecutableInterpreterRequest::Absolute(spec.path),
        spec.arguments,
    )))
}

/// The interpreter `file`'s shebang names, an `env` target found on
/// `search_path`.
pub(super) fn shebang_interpreter(
    file: &File,
    search_path: Option<&OsStr>,
) -> WorkflowResult<Option<Shebang>> {
    let Some((request, arguments)) = interpreter_request(file)? else {
        return Ok(None);
    };
    let path = match request {
        ExecutableInterpreterRequest::Absolute(path) => path,
        // `find_on` yields only absolute paths: it skips relative `PATH`
        // entries.
        ExecutableInterpreterRequest::Search(target) => {
            crate::executable::find_on(&target, search_path).ok_or_else(|| {
                identity_drift(
                    Path::new(&target),
                    "env shebang interpreter was not found on PATH",
                )
            })?
        }
    };
    Ok(Some(Shebang { path, arguments }))
}

/// The shebang `file` starts with, if any, leaving its offset where it was.
pub(super) fn raw_shebang(file: &File) -> WorkflowResult<Option<Shebang>> {
    let mut file = file.try_clone()?;
    let original_position = file.stream_position()?;
    file.seek(SeekFrom::Start(0))?;
    let result = (|| {
        // Receipt: Linux BINPRM_BUF_SIZE is 256 bytes; longer shebangs are not portable.
        let mut bytes = [0_u8; 256];
        let read = file.read(&mut bytes)?;
        if !bytes[..read].starts_with(b"#!") {
            return Ok(None);
        }
        let line = bytes[2..read]
            .split(|byte| *byte == b'\n')
            .next()
            .unwrap_or_default();
        let line = std::str::from_utf8(line)
            .map_err(|_| identity_drift(Path::new("<script>"), "shebang is not UTF-8"))?;
        let mut words = line.split_ascii_whitespace();
        let interpreter = words
            .next()
            .ok_or_else(|| identity_drift(Path::new("<script>"), "shebang has no interpreter"))?;
        Ok(Some(Shebang {
            path: Path::new(interpreter).to_owned(),
            arguments: words.map(str::to_owned).collect(),
        }))
    })();
    let restore_result = file.seek(SeekFrom::Start(original_position));
    match result {
        Ok(shebang) => {
            restore_result?;
            Ok(shebang)
        }
        Err(error) => {
            let _ = restore_result;
            Err(error)
        }
    }
}

#[cfg(test)]
#[path = "secure_fs_shebang_tests.rs"]
mod tests;
