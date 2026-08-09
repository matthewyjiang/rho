//! A spawned `claude` child plus the process group it leads.
//!
//! Both Claude paths - delegated subagent sessions and Rho's own one-shot
//! calls - spawn through here, so no path can leave a live process tree.

use std::time::Duration;

use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};

use crate::tools::process::{prepare_child_command, ProcessTree};

// Give Claude a short grace period to exit before force-killing its process tree.
const TERMINATION_GRACE_PERIOD: Duration = Duration::from_millis(200);

/// Owns a Claude child and its group: dropping it kills both.
pub(crate) struct OwnedChild {
    child: Child,
    tree: ProcessTree,
}

impl OwnedChild {
    /// Spawn `command` and attach its process group.
    ///
    /// Callers map the error into their own user-facing text;
    /// [`std::io::ErrorKind::NotFound`] means the binary was not there.
    pub(crate) fn spawn(mut command: Command) -> Result<Self, std::io::Error> {
        prepare_child_command(&mut command);
        // Linux can return ETXTBSY when a just-written executable is still open
        // for write (or still being closed) under parallel test load. Retry with
        // cooperative yields only - no timed sleeps.
        let child = {
            let mut attempts = 0;
            loop {
                match command.spawn() {
                    Ok(child) => break child,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::ExecutableFileBusy
                            && attempts < 32 =>
                    {
                        attempts += 1;
                        std::thread::yield_now();
                    }
                    Err(error) => return Err(error),
                }
            }
        };
        let tree = match ProcessTree::attach(&child) {
            Ok(tree) => tree,
            Err(error) => {
                // Attach failed: best-effort kill the lone process.
                let mut child = child;
                let _ = child.start_kill();
                return Err(std::io::Error::other(error));
            }
        };
        Ok(Self { child, tree })
    }

    pub(crate) async fn terminate(&mut self) {
        self.tree
            .terminate(&mut self.child, TERMINATION_GRACE_PERIOD)
            .await;
    }

    pub(crate) async fn wait(&mut self) -> std::io::Result<std::process::ExitStatus> {
        let status = self.child.wait().await;
        // Ensure any leftover group members are cleaned after the leader exits.
        self.tree.kill();
        status
    }

    pub(crate) fn stdin(&mut self) -> Option<ChildStdin> {
        self.child.stdin.take()
    }

    pub(crate) fn stdout(&mut self) -> Option<ChildStdout> {
        self.child.stdout.take()
    }

    /// Piped stderr, when the caller has no log file to redirect it into.
    pub(crate) fn stderr(&mut self) -> Option<ChildStderr> {
        self.child.stderr.take()
    }
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        self.tree.kill();
        let _ = self.child.start_kill();
    }
}
