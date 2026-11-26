//! The hooks the daemon's hooks files define, read without a session.

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{ClientEnvironment, HookContract};

/// List the hooks files a session would load and the hooks they define,
/// without loading a session or running a hook: the user's
/// `~/.rho/hooks.toml` and, given a workspace, its project's
/// `.rho/hooks.toml`. A file that cannot be read or parsed is listed with
/// its error. Each request reads the files again; what hooks did is per
/// session, in [`super::SessionReport::Hooks`]. A daemon that predates it
/// answers [`crate::ErrorCode::MethodNotFound`].
pub const HOOKS_LIST: &str = "rho/hooks/list";

/// Params of [`HOOKS_LIST`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct HooksListParams {
    /// The directory a session would open in, whose project's hooks file
    /// to list. Absent, only the user's is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<PathBuf>,
    /// The asking client's environment, whose `RHO_TRUST_PROJECT_HOOKS`
    /// decides whether the project's hooks file loads, as for a session that
    /// client opens. Absent, it does not.
    #[serde(default, skip_serializing_if = "ClientEnvironment::is_empty")]
    pub environment: ClientEnvironment,
}

impl HooksListParams {
    /// Builds it from each of its fields.
    pub fn new(workspace: Option<PathBuf>, environment: ClientEnvironment) -> Self {
        Self {
            workspace,
            environment,
        }
    }
}

/// Result of [`HOOKS_LIST`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct HooksListResponse {
    /// The user's hooks file, then the project's when the params name a
    /// workspace, each listed whether or not it exists. Their hooks run in
    /// this order.
    pub files: Vec<HooksFile>,
}

impl HooksListResponse {
    /// Builds it from each of its fields.
    pub fn new(files: Vec<HooksFile>) -> Self {
        Self { files }
    }
}

/// One hooks file and the hooks it defines.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct HooksFile {
    pub path: PathBuf,
    pub scope: HooksFileScope,
    pub status: HooksFileStatus,
    /// The hooks it defines, in file order: [`super::HookStatus::Active`]
    /// when it loads, [`super::HookStatus::Untrusted`] when it is skipped.
    /// Empty when it is absent or could not be read or parsed.
    pub hooks: Vec<HookContract>,
}

impl HooksFile {
    /// Builds it from each of its fields.
    pub fn new(
        path: PathBuf,
        scope: HooksFileScope,
        status: HooksFileStatus,
        hooks: Vec<HookContract>,
    ) -> Self {
        Self {
            path,
            scope,
            status,
            hooks,
        }
    }
}

/// Whose hooks file it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum HooksFileScope {
    /// The user's, which always loads.
    User,
    /// The workspace's project's, which loads only when the client trusts
    /// the project.
    Project,
    /// A value a newer daemon sends that this crate does not know; daemons
    /// never send it.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}

/// Whether a session would load a hooks file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum HooksFileStatus {
    /// No file is there.
    Absent,
    /// A session loads it and runs its hooks.
    Loaded,
    /// The project's file, which a session skips because the client does
    /// not trust the project; `error` says why it could not be read or
    /// parsed, when it could not.
    Untrusted {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// A file a session would load could not be read or parsed. A session
    /// then runs no hooks at all, those of the other file included.
    Invalid { error: String },
    /// A value a newer daemon sends that this crate does not know; daemons
    /// never send it.
    #[serde(other)]
    #[schemars(extend("x-rho-other" = true))]
    Unknown,
}
