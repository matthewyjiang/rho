//! The doctor row for the credential store the running daemon keeps
//! provider credentials in (#1662, #1674). `rho doctor` asks the daemon,
//! never starting one, since a daemon a client started from an SSH login
//! can lack what the OS store needs while this process has it.

use rho_protocol::{CredentialStoreKind, CredentialStoreStatus, StoreSelectedBy};

use super::report::{DoctorCheck, DoctorCheckId, DoctorStatus};

const LABEL: &str = "Daemon credential store";

/// What `rho doctor` learned about the daemon's credential store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DaemonStore {
    /// No daemon runs, so there is nothing to ask.
    NotRunning,
    /// A daemon runs but did not say: it speaks another protocol, or it
    /// failed or took too long to answer. The text says which.
    Unanswered(String),
    Reported(CredentialStoreStatus),
}

/// The row for `store`. A store the daemon cannot reach fails once someone
/// chose it, since signing in there cannot work; while nothing chose one it
/// warns, as the next login asks which store to use.
pub(crate) fn daemon_store_check(store: &DaemonStore) -> DoctorCheck {
    let row = |status, summary: String| {
        DoctorCheck::new(DoctorCheckId::DaemonCredentialStore, LABEL, status, summary)
    };
    match store {
        DaemonStore::NotRunning => row(DoctorStatus::Info, "daemon not running".into()).with_hint(
            "it starts when a client needs it; run `rho daemon start`, then `rho doctor` again to check the store it can reach",
        ),
        DaemonStore::Unanswered(reason) => {
            row(DoctorStatus::Warn, "daemon did not answer".into()).with_hint(reason.clone())
        }
        DaemonStore::Reported(status) => {
            let (name, chosen) = store_words(status);
            match &status.unreachable {
                None => row(DoctorStatus::Ok, format!("{name} store")).with_hint(format!("chosen {chosen}")),
                Some(reason) => {
                    let (severity, choose) = match status.selected_by {
                        StoreSelectedBy::Default => (
                            DoctorStatus::Warn,
                            "`rho login <provider>` asks which store to use",
                        ),
                        StoreSelectedBy::Environment => (
                            DoctorStatus::Fail,
                            "change RHO_CREDENTIAL_STORE where the daemon starts, then run `rho daemon stop`",
                        ),
                        _ => (
                            DoctorStatus::Fail,
                            "choose another with `rho credential-store set file`",
                        ),
                    };
                    row(severity, format!("{name} store unreachable")).with_hint(format!(
                        "{reason}; {choose}, or stop the daemon with `rho daemon stop` so a client where the store works starts it again"
                    ))
                }
            }
        }
    }
}

/// How `status`'s store and what chose it read in a sentence, as in
/// "os store, chosen by default".
pub(crate) fn store_words(status: &CredentialStoreStatus) -> (&'static str, &'static str) {
    let name = match status.store {
        CredentialStoreKind::Os => "os",
        CredentialStoreKind::File => "file",
        _ => "unknown",
    };
    let chosen = match status.selected_by {
        StoreSelectedBy::Default => "by default",
        StoreSelectedBy::Config => "in config",
        StoreSelectedBy::Environment => "by RHO_CREDENTIAL_STORE",
        _ => "by an unknown setting",
    };
    (name, chosen)
}

#[cfg(test)]
#[path = "daemon_store_tests.rs"]
mod tests;
