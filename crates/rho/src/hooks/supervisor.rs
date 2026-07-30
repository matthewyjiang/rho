//! Per-platform supervision of a hook child and everything it starts.
//!
//! A hook that forks a background process must not outlive its timeout. Unix
//! gets a process group, Windows a job object; both are killed on completion,
//! timeout, cancellation, and drop.

use tokio::process::Command;

/// Terminates a supervised process tree.
///
/// `kill` must be idempotent and safe to call repeatedly: the run loop calls it
/// on every exit path and `Drop` calls it again.
pub(super) trait ProcessTree: Sized + Send {
    /// Configures the command before spawn.
    fn prepare(command: &mut Command);

    fn attach(child: &tokio::process::Child) -> std::io::Result<Self>;

    fn kill(&mut self);
}

#[cfg(unix)]
pub(super) struct SupervisedTree {
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
        // descendants die with the hook rather than surviving it.
        let _ = unsafe { libc::kill(-pid, libc::SIGKILL) };
    }
}

#[cfg(windows)]
pub(super) struct SupervisedTree {
    job: Option<windows_sys::Win32::Foundation::HANDLE>,
}

#[cfg(windows)]
unsafe impl Send for SupervisedTree {}

#[cfg(windows)]
impl ProcessTree for SupervisedTree {
    fn prepare(_command: &mut Command) {}

    fn attach(child: &tokio::process::Child) -> std::io::Result<Self> {
        use windows_sys::Win32::{Foundation::CloseHandle, System::JobObjects::*};

        let process = child
            .raw_handle()
            .ok_or_else(|| std::io::Error::other("spawned hook process has no handle"))?;
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return Err(std::io::Error::last_os_error());
            }
            let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let configured = SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                (&raw const limits).cast(),
                std::mem::size_of_val(&limits) as u32,
            );
            if configured == 0 || AssignProcessToJobObject(job, process as _) == 0 {
                let error = std::io::Error::last_os_error();
                CloseHandle(job);
                return Err(error);
            }
            Ok(Self { job: Some(job) })
        }
    }

    fn kill(&mut self) {
        if let Some(job) = self.job.take() {
            unsafe {
                windows_sys::Win32::System::JobObjects::TerminateJobObject(job, 1);
                windows_sys::Win32::Foundation::CloseHandle(job);
            }
        }
    }
}

impl Drop for SupervisedTree {
    fn drop(&mut self) {
        self.kill();
    }
}
