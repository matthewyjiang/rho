use std::time::Duration;

use tokio::process::{Child, Command};

/// Isolate the child before it can execute or create descendants.
pub(crate) fn prepare_child_command(command: &mut Command) {
    #[cfg(unix)]
    command.process_group(0);
    #[cfg(windows)]
    rho_tools::process_supervision::WindowsJob::prepare(command);
}

#[cfg(unix)]
pub(crate) struct ProcessTree(i32);
#[cfg(unix)]
impl ProcessTree {
    pub(crate) fn attach(child: &Child) -> Result<Self, String> {
        child
            .id()
            .and_then(|pid| i32::try_from(pid).ok())
            .map(Self)
            .ok_or_else(|| "spawned process has no pid".into())
    }
    fn signal(&self, signal: i32) {
        unsafe {
            libc::kill(-self.0, signal);
        }
    }
    pub(crate) async fn terminate(&self, child: &mut Child, grace: Duration) {
        self.signal(libc::SIGTERM);
        if tokio::time::timeout(grace, child.wait()).await.is_err() {
            self.signal(libc::SIGKILL);
            let _ = child.wait().await;
        } else {
            // The group leader can exit while descendants still own output pipes.
            self.signal(libc::SIGKILL);
        }
    }
    pub(crate) fn kill(&self) {
        self.signal(libc::SIGKILL);
    }
}

#[cfg(windows)]
pub(crate) struct ProcessTree(rho_tools::process_supervision::WindowsJob);
#[cfg(windows)]
impl ProcessTree {
    pub(crate) fn attach(child: &Child) -> Result<Self, String> {
        rho_tools::process_supervision::WindowsJob::attach(child)
            .map(Self)
            .map_err(|error| error.to_string())
    }
    // Windows terminates the job immediately; unlike Unix, there is no SIGTERM
    // grace period for these console-independent child processes.
    pub(crate) async fn terminate(&self, child: &mut Child, _grace: Duration) {
        self.kill();
        let _ = child.wait().await;
    }
    pub(crate) fn kill(&self) {
        self.0.kill();
    }
}
