//! Explain a blocked keyring read before the TUI owns the terminal.

use std::{
    io::{self, IsTerminal, Write},
    sync::{
        mpsc::{self, RecvTimeoutError},
        Arc, Mutex,
    },
    thread,
    time::Duration,
};

use rho_providers::credentials::{CredentialResult, CredentialStore, CredentialStoreBackend};

/// How long a keyring call may run before the hint prints. An unlocked Secret
/// Service lookup measured ~5 ms cold and under 1 ms warm, while an unlock
/// dialog waits on the user for seconds, so a quick open never shows the hint.
const NOTICE_DELAY: Duration = Duration::from_millis(250);

static NOTICE: Mutex<NoticeState> = Mutex::new(NoticeState::Disabled);

enum NoticeState {
    Disabled,
    Pending,
    Shown,
}

impl NoticeState {
    fn show(&mut self, output: &mut impl Write) {
        if matches!(self, Self::Pending) {
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
        let mut state = lock(&NOTICE);
        *state = if interactive && crate::stdio::stdin_is_terminal() && io::stderr().is_terminal() {
            NoticeState::Pending
        } else {
            NoticeState::Disabled
        };
        Self
    }
}

impl Drop for StartupKeyringNotice {
    fn drop(&mut self) {
        // Serialize with the write so a watcher cannot print after this
        // returns and the terminal enters raw mode.
        *lock(&NOTICE) = NoticeState::Disabled;
    }
}

fn lock(notice: &Mutex<NoticeState>) -> std::sync::MutexGuard<'_, NoticeState> {
    notice.lock().unwrap_or_else(|error| error.into_inner())
}

/// Runs `access`, showing the pending notice on `output` only if `access` is
/// still running after `delay`. Returns once both `access` and the watcher finish.
fn access_with_notice<T>(
    notice: &'static Mutex<NoticeState>,
    delay: Duration,
    mut output: impl Write + Send + 'static,
    access: impl FnOnce() -> T,
) -> T {
    if !matches!(*lock(notice), NoticeState::Pending) {
        return access();
    }
    let (finished, wait_for_finish) = mpsc::channel::<()>();
    let watcher = thread::Builder::new()
        .name("keyring-notice".into())
        .spawn(move || {
            if wait_for_finish.recv_timeout(delay) == Err(RecvTimeoutError::Timeout) {
                lock(notice).show(&mut output);
            }
        });
    let result = access();
    drop(finished);
    if let Ok(watcher) = watcher {
        let _ = watcher.join();
    }
    result
}

pub(super) struct NotifyingCredentialStore {
    pub(super) inner: Arc<dyn CredentialStore>,
    pub(super) backend: CredentialStoreBackend,
}

impl NotifyingCredentialStore {
    fn notify_if_slow<T>(&self, access: impl FnOnce() -> T) -> T {
        match self.backend {
            CredentialStoreBackend::Os => {
                access_with_notice(&NOTICE, NOTICE_DELAY, io::stderr(), access)
            }
            CredentialStoreBackend::File => access(),
        }
    }
}

impl CredentialStore for NotifyingCredentialStore {
    fn get_secret(&self, account: &str) -> CredentialResult<Option<String>> {
        self.notify_if_slow(|| self.inner.get_secret(account))
    }

    fn set_secret(&self, account: &str, secret: &str) -> CredentialResult<()> {
        self.notify_if_slow(|| self.inner.set_secret(account, secret))
    }

    fn delete_secret(&self, account: &str) -> CredentialResult<bool> {
        self.notify_if_slow(|| self.inner.delete_secret(account))
    }
}

#[cfg(test)]
#[path = "startup_notice_tests.rs"]
mod tests;
