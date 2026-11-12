//! What rho's own `git` commands share.

/// Inherited variables that would point `git` at another repository than
/// the one a command names, as when rho runs inside a git hook. Commands
/// rho runs on a repository of its choosing remove them.
pub(crate) const LOCATION_ENV: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_COMMON_DIR",
    "GIT_NAMESPACE",
];
