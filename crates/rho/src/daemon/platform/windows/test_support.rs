//! Test seams for the Windows side of the boundary.

use super::process::StopRequests;
use crate::daemon::paths::DaemonPaths;

/// The stop pipe a stand-in daemon listens on, as a real one does.
pub(crate) struct StopTarget(StopRequests);

impl StopTarget {
    pub(crate) fn listen(paths: &DaemonPaths) -> Self {
        Self(StopRequests::listen(paths).expect("the stop pipe is created"))
    }

    /// The pid a stand-in daemon records. The stop pipe, not the pid, is
    /// what a `request_stop` reaches.
    pub(crate) fn pid(&self) -> u32 {
        std::process::id()
    }

    /// Resolves once the stand-in was asked to stop.
    pub(crate) async fn asked(&mut self) {
        assert_eq!(self.0.recv().await, "stop request");
    }
}
