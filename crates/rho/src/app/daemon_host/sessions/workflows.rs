//! Read which workflow runs of the loaded sessions still owe their session
//! a result, without waiting for any session's turn.

use std::collections::HashSet;

use super::Sessions;

impl Sessions {
    /// The workflow runs, by id, that finished and whose result a loaded
    /// session has yet to be given.
    pub(in crate::app::daemon_host) fn undelivered_workflow_runs(&self) -> HashSet<String> {
        self.loaded()
            .values()
            .flat_map(|session| {
                session
                    .background
                    .workflows()
                    .undelivered_runs(session.id.as_str())
            })
            .collect()
    }
}
