//! Test seams for the Unix side of the boundary.

use std::os::unix::process::ExitStatusExt;

use tokio::process::{Child, Command};

use crate::daemon::paths::DaemonPaths;

/// The process a stand-in daemon records as itself, so a `request_stop`
/// lands there and not on the test process.
pub(crate) struct StopTarget(Child);

impl StopTarget {
    pub(crate) fn listen(_paths: &DaemonPaths) -> Self {
        Self(
            Command::new("sleep")
                .arg("600")
                .kill_on_drop(true)
                .spawn()
                .expect("sleep starts"),
        )
    }

    /// The pid a stand-in daemon records.
    pub(crate) fn pid(&self) -> u32 {
        self.0.id().expect("the process is running")
    }

    /// Resolves once the stand-in was asked to stop, by SIGTERM.
    pub(crate) async fn asked(&mut self) {
        let ended = self.0.wait().await.expect("the process is awaited");
        assert_eq!(ended.signal(), Some(libc::SIGTERM));
    }
}
