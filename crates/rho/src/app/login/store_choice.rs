//! Asking at the terminal where provider credentials are kept, when nothing
//! chose a store yet (#1662).
//!
//! Whoever writes the credentials judges which stores it can reach: the
//! rho daemon on unix (`daemon_client.rs`), this process elsewhere. Rho
//! never moves credentials to the file store on its own; when the OS store
//! cannot be reached, the file store becomes the answer Enter takes, and the
//! user still has to take it.

use std::io::{self, IsTerminal, Write};

use rho_providers::credentials::CredentialStoreBackend;

/// The two stores offered, as whoever writes the credentials sees them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::app) struct StoreOffers {
    /// Why the OS store cannot be reached; `None` when it can.
    pub(in crate::app) os_unavailable: Option<String>,
    /// Why the file store cannot be used; `None` when it can, or when only
    /// choosing it will tell.
    pub(in crate::app) file_unavailable: Option<String>,
}

impl StoreOffers {
    /// What Enter takes: the OS store when it can be reached, else the file
    /// store when it can. `None` when neither can.
    fn default_choice(&self) -> Option<CredentialStoreBackend> {
        match (&self.os_unavailable, &self.file_unavailable) {
            (None, _) => Some(CredentialStoreBackend::Os),
            (Some(_), None) => Some(CredentialStoreBackend::File),
            (Some(_), Some(_)) => None,
        }
    }

    fn unavailable(&self, backend: CredentialStoreBackend) -> Option<&str> {
        match backend {
            CredentialStoreBackend::Os => self.os_unavailable.as_deref(),
            CredentialStoreBackend::File => self.file_unavailable.as_deref(),
        }
    }

    /// What Enter takes, or why nothing can be chosen.
    fn require_default(&self) -> anyhow::Result<CredentialStoreBackend> {
        self.default_choice().ok_or_else(|| {
            anyhow::anyhow!(
                "no credential store backend is available (os: {}; file: {})",
                self.os_unavailable.as_deref().unwrap_or_default(),
                self.file_unavailable.as_deref().unwrap_or_default()
            )
        })
    }

    /// The store `answer` picks, Enter taking [`Self::default_choice`].
    fn pick(&self, answer: &str) -> anyhow::Result<CredentialStoreBackend> {
        let default = self.require_default()?;
        let backend = match answer.trim() {
            "" => default,
            "1" | "os" | "OS" => CredentialStoreBackend::Os,
            "2" | "file" | "FILE" => CredentialStoreBackend::File,
            other => {
                anyhow::bail!(
                    "unrecognized credential store choice '{other}'; expected 1/os or 2/file"
                )
            }
        };
        if let Some(detail) = self.unavailable(backend) {
            anyhow::bail!(
                "{} credential store is unavailable: {detail}",
                backend.as_str()
            );
        }
        Ok(backend)
    }
}

/// Asks on stderr which store `offers` to keep provider credentials in, and
/// reads the answer from stdin. Blocks on the terminal, so call it off the
/// async workers. Without a terminal it refuses, naming how to choose.
pub(in crate::app) fn ask(offers: &StoreOffers) -> anyhow::Result<CredentialStoreBackend> {
    if !crate::stdio::stdin_is_terminal() || !io::stderr().is_terminal() {
        anyhow::bail!(
            "credential store is unset; set it before non-interactive login with \
`rho credential-store set os|file`, behavior.credential_store in config.toml, \
or RHO_CREDENTIAL_STORE=os|file"
        );
    }
    let default = offers.require_default()?;
    eprintln!("Choose where Rho stores provider credentials:");
    eprintln!("This is saved to config and used for future logins on this machine.");
    match &offers.os_unavailable {
        None => eprintln!("  [1] OS credential store (recommended)"),
        Some(detail) => eprintln!("  [1] OS credential store (unavailable: {detail})"),
    }
    match &offers.file_unavailable {
        None => eprintln!("  [2] Local file under ~/.rho/credentials (not encrypted at rest)"),
        Some(detail) => eprintln!("  [2] Local file (unavailable: {detail})"),
    }
    let default_hint = match default {
        CredentialStoreBackend::Os => "1",
        CredentialStoreBackend::File => "2",
    };
    eprint!("Choice [1/2 or os/file] (default {default_hint}): ");
    io::stderr().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    offers.pick(&answer)
}

#[cfg(test)]
#[path = "store_choice_tests.rs"]
mod tests;
