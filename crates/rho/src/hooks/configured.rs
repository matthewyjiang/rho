//! The hooks files a session would load, each read on its own, for
//! `rho/hooks/list`: a settings view of what is configured, with no
//! session, runtime, or activity.
//!
//! Each file is read as [`HookCatalog::discover`](super::HookCatalog::discover)
//! reads it ([`HooksFileRead`]). Where `discover` stops at the first file a
//! session cannot load, since a session then runs no hooks, this reads
//! every file, so one file's error does not hide the other's hooks, and
//! marks the file that would load `disabled`.

use std::path::Path;

use rho_protocol::{HookStatus, HooksFile, HooksFileScope, HooksFileStatus};

use super::{
    catalog::{spawn_contract, HooksFileOutcome, HooksFileRead},
    diagnostics::contract,
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
    let user = rho_home.map(|home| (HooksFileScope::User, HooksFileRead::user(home)));
    let project =
        project_root.map(|root| (HooksFileScope::Project, HooksFileRead::project(root, trust)));
    let files = user.into_iter().chain(project).collect::<Vec<_>>();
    // The first invalid file is the one a session fails on.
    let invalid = files
        .iter()
        .find(|(_, file)| matches!(file.outcome, HooksFileOutcome::Invalid(_)))
        .map(|(_, file)| file.path.clone());
    files
        .into_iter()
        .map(|(scope, file)| listed(scope, file, invalid.as_deref()))
        .collect()
}

/// `file` as `rho/hooks/list` lists it, `disabled` by the file at
/// `invalid` when it would load.
fn listed(scope: HooksFileScope, file: HooksFileRead, invalid: Option<&Path>) -> HooksFile {
    let (status, definitions, hook_status) = match file.outcome {
        HooksFileOutcome::Absent => (HooksFileStatus::Absent, Vec::new(), HookStatus::Active),
        HooksFileOutcome::Loaded(hooks) => match invalid {
            None => (HooksFileStatus::Loaded, hooks, HookStatus::Active),
            Some(by) => (
                HooksFileStatus::Disabled {
                    by: by.to_path_buf(),
                },
                hooks,
                HookStatus::Disabled,
            ),
        },
        HooksFileOutcome::Untrusted { definitions, error } => (
            HooksFileStatus::Untrusted {
                error: error.map(|error| error.to_string()),
            },
            definitions,
            HookStatus::Untrusted,
        ),
        HooksFileOutcome::Invalid(error) => (
            HooksFileStatus::Invalid {
                error: error.to_string(),
            },
            Vec::new(),
            HookStatus::Active,
        ),
    };
    let hooks = definitions
        .iter()
        .map(|hook| {
            contract(
                &spawn_contract(hook, hook_status == HookStatus::Active),
                hook_status,
            )
        })
        .collect();
    HooksFile::new(file.path, scope, status, hooks)
}

#[cfg(test)]
#[path = "configured_tests.rs"]
mod tests;
