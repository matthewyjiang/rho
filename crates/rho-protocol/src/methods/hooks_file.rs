//! The user's hooks file as text, read and replaced whole, so a client can
//! copy it from one daemon to another.

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::ConfigRevision;

/// Read the user's `~/.rho/hooks.toml` as text, with the revision a
/// [`HOOKS_FILE_SET`] sends back. A project's hooks file is never read. A
/// daemon that predates it answers [`crate::ErrorCode::MethodNotFound`].
pub const HOOKS_FILE_GET: &str = "rho/hooks/file/get";
/// Replace or delete the user's `~/.rho/hooks.toml`. A daemon that predates
/// it answers [`crate::ErrorCode::MethodNotFound`].
pub const HOOKS_FILE_SET: &str = "rho/hooks/file/set";

/// Params of [`HOOKS_FILE_GET`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct HooksFileGetParams {}

impl HooksFileGetParams {
    /// Builds it; it has no fields.
    pub fn new() -> Self {
        Self {}
    }
}

/// Result of [`HOOKS_FILE_GET`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct HooksFileGetResponse {
    /// Where the file is, or would be, on the daemon's machine, with its
    /// links resolved.
    pub path: PathBuf,
    /// Send it with the next [`HOOKS_FILE_SET`]. It changes with any byte of
    /// the file, comments included, and an absent file has its own.
    pub revision: ConfigRevision,
    /// The file's whole text; absent when there is no file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

impl HooksFileGetResponse {
    /// Builds it from each of its fields.
    pub fn new(path: PathBuf, revision: ConfigRevision, text: Option<String>) -> Self {
        Self {
            path,
            revision,
            text,
        }
    }
}

/// Params of [`HOOKS_FILE_SET`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct HooksFileSetParams {
    /// The revision the text replaces. A stale one is refused with
    /// [`crate::ErrorCode::ConfigConflict`].
    pub revision: ConfigRevision,
    /// The file's new text, written atomically once it loads as a session
    /// would load it, which includes finding each hook's program when it is
    /// named by an absolute path. Text that does not load is refused with
    /// [`crate::ErrorCode::InvalidParams`] naming the problem. Absent or
    /// `null` deletes the file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

impl HooksFileSetParams {
    /// Builds it from each of its fields.
    pub fn new(revision: ConfigRevision, text: Option<String>) -> Self {
        Self { revision, text }
    }
}

/// Result of [`HOOKS_FILE_SET`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct HooksFileSetResponse {
    /// The file's revision with the new text.
    pub revision: ConfigRevision,
}

impl HooksFileSetResponse {
    /// Builds it from each of its fields.
    pub fn new(revision: ConfigRevision) -> Self {
        Self { revision }
    }
}
