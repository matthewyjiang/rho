//! The hooks files a session would load, each read on its own, for
//! `rho/hooks/list`: a settings view of what is configured, with no
//! session, runtime, or activity.
//!
//! [`HookCatalog::discover`](super::HookCatalog::discover) stops at the first
//! file a session cannot load, since a session then runs no hooks; this
//! reads every file so one file's error does not hide the other's hooks.

use std::path::Path;

use rho_protocol::{HooksFile, HooksFileScope, HooksFileStatus};

use super::{
    catalog::{read_definitions, spawn_contract},
    diagnostics::contract,
    HookSource,
};
use crate::workspace::ProjectTrust;

/// The user's hooks file under `rho_home`, then, given `project_root`, the
/// project's, loaded only under `trust` as a session loads it. Each is
/// listed whether or not it exists.
pub(crate) fn configured_files(
    rho_home: Option<&Path>,
    project_root: Option<&Path>,
    trust: ProjectTrust,
) -> Vec<HooksFile> {
    // The user's own file needs no trust.
    let user = rho_home.map(|home| {
        read_file(
            &home.join("hooks.toml"),
            HookSource::User,
            None,
            ProjectTrust::Trusted,
        )
    });
    let project = project_root.map(|root| {
        read_file(
            &root.join(".rho/hooks.toml"),
            HookSource::Project,
            Some(root),
            trust,
        )
    });
    user.into_iter().chain(project).collect()
}

/// One hooks file as a session treats it: loaded under `trust`, else
/// skipped with its hooks shown inactive.
fn read_file(
    path: &Path,
    source: HookSource,
    project_root: Option<&Path>,
    trust: ProjectTrust,
) -> HooksFile {
    let trusted = trust.is_trusted();
    let (status, definitions) = match (read_definitions(path, source, project_root), trusted) {
        (Ok(None), _) => (HooksFileStatus::Absent, Vec::new()),
        (Ok(Some(definitions)), true) => (HooksFileStatus::Loaded, definitions),
        (Ok(Some(definitions)), false) => (HooksFileStatus::Untrusted { error: None }, definitions),
        (Err(error), true) => (
            HooksFileStatus::Invalid {
                error: error.to_string(),
            },
            Vec::new(),
        ),
        (Err(error), false) => (
            HooksFileStatus::Untrusted {
                error: Some(error.to_string()),
            },
            Vec::new(),
        ),
    };
    let scope = match source {
        HookSource::User => HooksFileScope::User,
        HookSource::Project => HooksFileScope::Project,
    };
    let hooks = definitions
        .iter()
        .map(|hook| contract(&spawn_contract(hook, trusted)))
        .collect();
    HooksFile::new(path.to_path_buf(), scope, status, hooks)
}

#[cfg(test)]
#[path = "configured_tests.rs"]
mod tests;
