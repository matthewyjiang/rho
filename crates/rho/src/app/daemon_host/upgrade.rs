//! Upgrading the daemon in place (#1713): noticing that its executable was
//! replaced, waiting until no job is in progress, then restarting, which
//! re-executes the replacement as the same process.
//!
//! The daemon checks its executable every [`UPGRADE_CHECK_INTERVAL`], and at
//! once when a client newer than the daemon says hello. A replacement must
//! print its rho version before the daemon trusts it (see
//! [`Executable::probe`]); from then on `rho/daemon/status` reports the
//! upgrade pending, and the daemon keeps serving and taking new work until a
//! check finds nothing the restart would cut short: no job in progress, no
//! login under way, and no loaded session with work of its own (see
//! `Sessions::upgrade_blockers`). Jobs waiting for input count as in
//! progress until a restarted daemon can ask again (#1660, #1661).
//!
//! A newer client whose daemon's executable was not replaced, as when an
//! installer put the new version at another path, cannot upgrade it; the
//! daemon logs that once per client version.

use std::{
    sync::{Mutex, MutexGuard, PoisonError},
    time::Duration,
};

use rho_protocol::ClientInfo;
use tokio::sync::Notify;

use crate::daemon::{Executable, FileIdentity, ProbeError};

/// How often the daemon checks whether its executable was replaced and,
/// once it was, whether it is idle.
//
// Each check is one `stat(2)` of the executable (2.4 µs median over 10,000
// runs on ext4, measured through Python's `os.stat`, so an upper bound) and,
// while an upgrade waits, one count of the job records under their lock. 1 s keeps that negligible while bounding how long an
// idle daemon runs a replaced binary, and how long an upgrade waits after
// the last job ends.
pub(super) const UPGRADE_CHECK_INTERVAL: Duration = Duration::from_secs(1);

/// What the daemon knows of its upgrade.
#[derive(Default)]
pub(super) struct Upgrades {
    /// The version of the replacement the daemon waits to re-execute.
    pending: Mutex<Option<String>>,
    /// The newest version of a client newer than the daemon.
    newer_client: Mutex<Option<String>>,
    /// Wakes the watch once such a client said hello.
    heard_newer: Notify,
}

/// Where the watch stands between checks.
#[derive(Default)]
pub(super) struct Watch {
    /// The replacement last probed to an answer, whether it proved itself
    /// rho or not, so each is run once; one that could not run is tried
    /// again at the next check.
    probed: Option<FileIdentity>,
    /// The newer client version last warned about.
    warned: Option<String>,
}

/// What a check found the daemon should do.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Check {
    /// Keep running this binary.
    Wait,
    /// Restart once no job is in progress.
    Upgrade,
}

impl Upgrades {
    /// Notes the client a connection's hello named: a newer one wakes the
    /// watch, so the daemon looks for its replacement at once.
    pub(super) fn heard(&self, client: &ClientInfo) {
        if !crate::update::version_is_newer(&client.version, env!("CARGO_PKG_VERSION")) {
            return;
        }
        let mut newest = lock(&self.newer_client);
        if newest
            .as_deref()
            .is_none_or(|newest| crate::update::version_is_newer(&client.version, newest))
        {
            *newest = Some(client.version.clone());
        }
        drop(newest);
        self.heard_newer.notify_one();
    }

    /// The version of the replacement the daemon waits to re-execute.
    pub(super) fn pending(&self) -> Option<String> {
        lock(&self.pending).clone()
    }

    /// Resolves after `interval`, or sooner once a newer client said hello.
    pub(super) async fn next_check(&self, interval: &mut tokio::time::Interval) {
        tokio::select! {
            _ = interval.tick() => {}
            () = self.heard_newer.notified() => {}
        }
    }

    /// Checks `executable` for a replacement, running a new one with
    /// `--version`, and records what the upgrade waits for.
    pub(super) async fn check(&self, executable: &Executable, watch: &mut Watch) -> Check {
        let Some(replacement) = executable.replacement() else {
            *lock(&self.pending) = None;
            watch.probed = None;
            let newer_client = lock(&self.newer_client).clone();
            if newer_client.is_some() && watch.warned != newer_client {
                tracing::warn!(
                    client = newer_client.as_deref(),
                    executable = %executable.path().display(),
                    "a newer rho client connected, but the daemon's executable was not \
                     replaced; restart the daemon from the newer rho to upgrade it"
                );
                watch.warned = newer_client;
            }
            return Check::Wait;
        };
        if watch.probed != Some(replacement.identity) {
            let probed = executable.probe().await;
            match &probed {
                Ok(version) => tracing::info!(
                    %version,
                    executable = %executable.path().display(),
                    "the daemon's executable was replaced; upgrading once nothing would be cut short"
                ),
                Err(error) if error.is_transient() => tracing::debug!(
                    %error,
                    executable = %executable.path().display(),
                    "could not probe the daemon's replaced executable; trying again"
                ),
                Err(error) => tracing::warn!(
                    %error,
                    executable = %executable.path().display(),
                    "the daemon's executable was replaced by one that is not rho; not upgrading"
                ),
            }
            if !probed.as_ref().is_err_and(ProbeError::is_transient) {
                watch.probed = Some(replacement.identity);
            }
            *lock(&self.pending) = probed.ok();
        }
        match *lock(&self.pending) {
            Some(_) => Check::Upgrade,
            None => Check::Wait,
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // Each value is replaced whole, so a panic cannot leave one half-set.
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
