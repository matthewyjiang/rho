use std::time::Duration;
use tokio::process::{Child, Command};

#[cfg(unix)]
pub(super) fn shell_command(command: &str) -> Command {
    let mut command_line = Command::new("bash");
    command_line.arg("-lc").arg(command).process_group(0);
    command_line
}
#[cfg(windows)]
pub(super) fn shell_command(command: &str) -> Command {
    let mut command_line = Command::new("powershell");
    command_line.args(["-NoProfile", "-NonInteractive", "-Command", command]);
    command_line
}

#[cfg(unix)]
pub(super) struct ProcessTree(i32);
#[cfg(unix)]
impl ProcessTree {
    pub(super) fn attach(child: &Child) -> Result<Self, String> {
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
    pub(super) async fn terminate(&self, child: &mut Child, grace: Duration) {
        self.signal(libc::SIGTERM);
        if tokio::time::timeout(grace, child.wait()).await.is_err() {
            self.signal(libc::SIGKILL);
            let _ = child.wait().await;
        } else {
            // The group leader can exit while descendants still own output pipes.
            self.signal(libc::SIGKILL);
        }
    }
    pub(super) fn kill(&self) {
        self.signal(libc::SIGKILL);
    }
}

#[cfg(windows)]
pub(super) struct ProcessTree(windows_sys::Win32::Foundation::HANDLE);
#[cfg(windows)]
unsafe impl Send for ProcessTree {}
#[cfg(windows)]
unsafe impl Sync for ProcessTree {}
#[cfg(windows)]
impl ProcessTree {
    pub(super) fn attach(child: &Child) -> Result<Self, String> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::{Foundation::CloseHandle, System::JobObjects::*};
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
            let assigned =
                configured != 0 && AssignProcessToJobObject(job, child.as_raw_handle()) != 0;
            if !assigned {
                let error = std::io::Error::last_os_error();
                CloseHandle(job);
                return Err(error.to_string());
            }
            Ok(Self(job))
        }
    }
    pub(super) async fn terminate(&self, child: &mut Child, _grace: Duration) {
        self.kill();
        let _ = child.wait().await;
    }
    pub(super) fn kill(&self) {
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
