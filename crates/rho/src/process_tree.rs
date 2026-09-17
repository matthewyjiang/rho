//! Per-platform supervision of a child and everything it starts.
//!
//! Unix gets a process group, Windows a kill-on-close job. Windows starts the
//! child suspended, assigns it to the job, and only then resumes its primary
//! thread. Dropping the owner terminates descendants, including on cancellation.

use tokio::process::Command;

/// Terminates a supervised process tree.
///
/// `kill` must be idempotent and safe to call repeatedly: the run loop calls it
/// on every exit path and `Drop` calls it again.
pub(crate) trait ProcessTree: Sized + Send {
    /// Configures the command before spawn.
    fn prepare(command: &mut Command);

    fn attach(child: &tokio::process::Child) -> std::io::Result<Self>;

    fn kill(&mut self);
}

#[cfg(unix)]
pub(crate) struct SupervisedTree {
    pid: Option<u32>,
}

#[cfg(unix)]
impl ProcessTree for SupervisedTree {
    fn prepare(command: &mut Command) {
        command.process_group(0);
    }

    fn attach(child: &tokio::process::Child) -> std::io::Result<Self> {
        Ok(Self { pid: child.id() })
    }

    fn kill(&mut self) {
        let Some(pid) = self.pid.take().and_then(|pid| i32::try_from(pid).ok()) else {
            return;
        };
        // A negative PID targets the group created by `process_group(0)`, so
        // descendants die with the child rather than surviving it.
        let _ = unsafe { libc::kill(-pid, libc::SIGKILL) };
    }
}

#[cfg(windows)]
pub(crate) struct SupervisedTree(rho_tools::process_supervision::WindowsJob);

#[cfg(windows)]
impl ProcessTree for SupervisedTree {
    fn prepare(command: &mut Command) {
        rho_tools::process_supervision::WindowsJob::prepare(command);
    }

    fn attach(child: &tokio::process::Child) -> std::io::Result<Self> {
        rho_tools::process_supervision::WindowsJob::attach(child).map(Self)
    }

    fn kill(&mut self) {
        self.0.kill();
    }
}

impl Drop for SupervisedTree {
    fn drop(&mut self) {
        self.kill();
    }
}
