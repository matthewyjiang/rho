//! Windows job ownership shared by shell tools and the application host.
//!
//! Internal cross-crate support for Rho, not a supported downstream API.

use tokio::process::{Child, Command};
use windows_sys::Win32::Foundation::HANDLE;

/// Owns a kill-on-close job. Children start suspended so they cannot create
/// descendants before assignment. Prepare immediately before spawn, then attach
/// before awaiting anything. Callers must kill/reap the child if attachment fails.
pub struct WindowsJob {
    job: HANDLE,
}

// The job handle is owned here; Windows permits operations from any thread.
unsafe impl Send for WindowsJob {}
unsafe impl Sync for WindowsJob {}

impl WindowsJob {
    /// Suspend the primary thread at creation until `attach` assigns its job.
    pub fn prepare(command: &mut Command) {
        use std::os::windows::process::CommandExt;
        command
            .as_std_mut()
            .creation_flags(windows_sys::Win32::System::Threading::CREATE_SUSPENDED);
    }

    /// Assign a prepared child to its job, then resume its primary thread.
    pub fn attach(child: &Child) -> std::io::Result<Self> {
        use windows_sys::Win32::System::JobObjects::*;

        let pid = child
            .id()
            .ok_or_else(|| std::io::Error::other("spawned child process has no id"))?;
        let process = child
            .raw_handle()
            .ok_or_else(|| std::io::Error::other("spawned child process has no handle"))?;
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return Err(std::io::Error::last_os_error());
            }
            let owner = Self { job };
            let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let configured = SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                (&raw const limits).cast(),
                std::mem::size_of_val(&limits) as u32,
            );
            if configured == 0 || AssignProcessToJobObject(job, process as _) == 0 {
                return Err(std::io::Error::last_os_error());
            }
            resume_process_thread(pid)?;
            Ok(owner)
        }
    }

    /// Terminate every process in the job. Safe to call repeatedly.
    pub fn kill(&self) {
        unsafe {
            windows_sys::Win32::System::JobObjects::TerminateJobObject(self.job, 1);
        }
    }
}

impl Drop for WindowsJob {
    fn drop(&mut self) {
        self.kill();
        unsafe { windows_sys::Win32::Foundation::CloseHandle(self.job) };
    }
}

unsafe fn resume_process_thread(pid: u32) -> std::io::Result<()> {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, INVALID_HANDLE_VALUE},
        System::{
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD,
                THREADENTRY32,
            },
            Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME},
        },
    };

    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(std::io::Error::last_os_error());
    }
    let mut entry = THREADENTRY32 {
        dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
        ..Default::default()
    };
    let mut found = unsafe { Thread32First(snapshot, &raw mut entry) } != 0;
    while found && entry.th32OwnerProcessID != pid {
        found = unsafe { Thread32Next(snapshot, &raw mut entry) } != 0;
    }
    unsafe { CloseHandle(snapshot) };
    if !found {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "spawned child process has no thread",
        ));
    }

    let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) };
    if thread.is_null() {
        return Err(std::io::Error::last_os_error());
    }
    let resumed = unsafe { ResumeThread(thread) };
    let error = (resumed == u32::MAX).then(std::io::Error::last_os_error);
    unsafe { CloseHandle(thread) };
    error.map_or(Ok(()), Err)
}

#[cfg(test)]
#[path = "process_supervision_tests.rs"]
mod tests;
