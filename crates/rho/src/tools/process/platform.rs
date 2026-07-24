use std::time::Duration;

use tokio::process::{Child, Command};

/// Configure a command so the child owns an isolatable process tree.
///
/// On Unix the child becomes a new process-group leader before `exec`. On
/// Windows the job object is attached after spawn via [`ProcessTree::attach`].
pub(crate) fn prepare_child_command(command: &mut Command) {
    #[cfg(unix)]
    command.process_group(0);
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
pub(crate) struct ProcessTree(windows_sys::Win32::Foundation::HANDLE);
#[cfg(windows)]
unsafe impl Send for ProcessTree {}
#[cfg(windows)]
unsafe impl Sync for ProcessTree {}
#[cfg(windows)]
impl ProcessTree {
    pub(crate) fn attach(child: &Child) -> Result<Self, String> {
        use windows_sys::Win32::{Foundation::CloseHandle, System::JobObjects::*};
        let process = child
            .raw_handle()
            .ok_or_else(|| "spawned process has no handle".to_string())?;
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return Err(std::io::Error::last_os_error().to_string());
            }
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let configured = SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                (&raw const limits).cast(),
                std::mem::size_of_val(&limits) as u32,
            );
            let assigned = configured != 0 && AssignProcessToJobObject(job, process as _) != 0;
            if !assigned {
                let error = std::io::Error::last_os_error();
                CloseHandle(job);
                return Err(error.to_string());
            }
            Ok(Self(job))
        }
    }
    pub(crate) async fn terminate(&self, child: &mut Child, _grace: Duration) {
        self.kill();
        let _ = child.wait().await;
    }
    pub(crate) fn kill(&self) {
        unsafe {
            windows_sys::Win32::System::JobObjects::TerminateJobObject(self.0, 1);
        }
    }
}
#[cfg(windows)]
impl Drop for ProcessTree {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}
