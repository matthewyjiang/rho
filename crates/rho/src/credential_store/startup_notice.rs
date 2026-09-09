//! Explain a potentially blocking keyring read before the TUI owns the terminal.

use std::{
    io::{self, IsTerminal, Write},
    sync::{Arc, Mutex},
};

use rho_providers::credentials::{CredentialResult, CredentialStore, CredentialStoreBackend};

static NOTICE: Mutex<NoticeState> = Mutex::new(NoticeState::Disabled);

enum NoticeState {
    Disabled,
    Pending,
    Shown,
}

impl NoticeState {
    fn before_access(&mut self, backend: CredentialStoreBackend, output: &mut impl Write) {
        if backend == CredentialStoreBackend::Os && matches!(self, Self::Pending) {
            // The keyring API does not tell us whether it is waiting for an
            // unlock dialog. Explain where to look without claiming it is locked.
            let _ = writeln!(
                output,
                "accessing keyring; check your desktop for an unlock prompt"
            );
            let _ = output.flush();
            *self = Self::Shown;
        }
    }
}

/// Keeps the startup hint enabled until terminal ownership passes to the TUI.
/// Drop also disables it on startup errors. No credential identifiers are printed.
pub(crate) struct StartupKeyringNotice;

impl StartupKeyringNotice {
    pub(crate) fn begin(interactive: bool) -> Self {
        let mut state = NOTICE.lock().unwrap_or_else(|error| error.into_inner());
        *state = if interactive && io::stdin().is_terminal() && io::stderr().is_terminal() {
            NoticeState::Pending
        } else {
            NoticeState::Disabled
        };
        Self
    }
}

impl Drop for StartupKeyringNotice {
    fn drop(&mut self) {
        // Serialize with the write so an in-flight background read cannot print
        // after this returns and the terminal enters raw mode.
        *NOTICE.lock().unwrap_or_else(|error| error.into_inner()) = NoticeState::Disabled;
    }
}

pub(super) struct NotifyingCredentialStore {
    pub(super) inner: Arc<dyn CredentialStore>,
    pub(super) backend: CredentialStoreBackend,
}

impl NotifyingCredentialStore {
    fn before_access(&self) {
        NOTICE
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .before_access(self.backend, &mut io::stderr());
    }
}

impl CredentialStore for NotifyingCredentialStore {
    fn get_secret(&self, account: &str) -> CredentialResult<Option<String>> {
        self.before_access();
        self.inner.get_secret(account)
    }

    fn set_secret(&self, account: &str, secret: &str) -> CredentialResult<()> {
        self.before_access();
        self.inner.set_secret(account, secret)
    }

    fn delete_secret(&self, account: &str) -> CredentialResult<bool> {
        self.before_access();
        self.inner.delete_secret(account)
    }
}

#[cfg(test)]
#[path = "startup_notice_tests.rs"]
mod tests;
