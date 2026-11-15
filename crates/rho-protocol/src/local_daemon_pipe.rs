//! Opening a daemon's named pipe on Windows for [`crate::LocalDaemon`], the
//! way the `rho` CLI does: wait while every instance is taken, and send
//! nothing to a pipe that is not the current user's. The owner check and the
//! busy wait mirror the CLI's `daemon/platform/windows/{pipe,security}.rs`,
//! which this crate cannot depend on; change both.
//!
//! A pipe's name is visible to every account on the machine, so another
//! account could create a pipe of the daemon's name first. It cannot make
//! the current user that pipe's owner, so a client that checks the owner
//! before it writes never hands its hello, prompts, or keys to someone
//! else's server. The daemon creates its pipe owned by its user.

use std::{
    io,
    os::windows::io::{AsRawHandle, RawHandle},
    path::Path,
    ptr,
    time::{Duration, Instant},
};

use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient};
use windows_sys::Win32::{
    Foundation::{CloseHandle, LocalFree, ERROR_PIPE_BUSY, ERROR_SUCCESS, HANDLE},
    Security::{
        Authorization::{GetSecurityInfo, SE_KERNEL_OBJECT},
        EqualSid, GetTokenInformation, TokenUser, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR,
        PSID, TOKEN_QUERY, TOKEN_USER,
    },
    System::Threading::{GetCurrentProcess, OpenProcessToken},
};

use crate::CloseOnShutdown;

/// How long a client waits for a pipe whose every instance is taken.
//
// Tripwire, the `rho` CLI's own (`PIPE_BUSY_TIMEOUT` in its
// `daemon/platform/windows/pipe.rs`). The daemon makes the next instance as
// soon as a client takes the last, so the wait is one task wake-up and one
// `CreateNamedPipe`. Measured there with 256 `rho daemon status` started at
// once: the longest any client waited was 71 ms idle and 324 ms with all 16
// cores loaded. Hitting the limit means the daemon stopped accepting without
// closing its pipe.
const PIPE_BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// Interval between attempts to open a busy pipe. Each attempt is one
/// `CreateFile` on a local pipe.
const PIPE_BUSY_POLL_INTERVAL: Duration = Duration::from_millis(5);

/// Connects to the daemon's pipe `name`, waiting while every instance is
/// taken. `None` when no pipe of that name exists: no daemon runs, since a
/// daemon's pipe goes with its process. The server learns who connected and
/// cannot impersonate them: `ClientOptions` opens with the identification
/// level.
pub(crate) async fn open(name: &Path) -> io::Result<Option<CloseOnShutdown<NamedPipeClient>>> {
    let started = Instant::now();
    loop {
        match ClientOptions::new().open(name) {
            Ok(client) if owned_by_current_user(client.as_raw_handle())? => {
                return Ok(Some(CloseOnShutdown::new(client)))
            }
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!(
                        "{} was created by another user, so it is not this user's rho daemon",
                        name.display()
                    ),
                ))
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) if error.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) => {}
            Err(error) => return Err(error),
        }
        let elapsed = started.elapsed();
        if elapsed >= PIPE_BUSY_TIMEOUT {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!(
                    "every instance of {} stayed taken for PIPE_BUSY_TIMEOUT ({PIPE_BUSY_TIMEOUT:?}; waited {elapsed:?})",
                    name.display()
                ),
            ));
        }
        tokio::time::sleep(PIPE_BUSY_POLL_INTERVAL).await;
    }
}

/// The user this process runs as: a `TOKEN_USER` and the SID it points to,
/// as `GetTokenInformation` wrote them. `u64` elements keep the structure
/// aligned.
struct CurrentUser {
    token_user: Vec<u64>,
}

impl CurrentUser {
    fn query() -> io::Result<Self> {
        let mut token: HANDLE = ptr::null_mut();
        // SAFETY: `GetCurrentProcess` returns a pseudo handle that needs no
        // closing, and `token` is a valid place for the handle.
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut len = 0;
        // SAFETY: a null buffer of length 0 is how the needed length is
        // asked for; the call fails and writes only `len`.
        unsafe { GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut len) };
        let mut token_user = vec![0_u64; (len as usize).div_ceil(size_of::<u64>())];
        // SAFETY: the buffer is writable for the `len` bytes the call asked
        // for, and aligned for a `TOKEN_USER`.
        let read = unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                token_user.as_mut_ptr().cast(),
                len,
                &mut len,
            )
        };
        let error = io::Error::last_os_error();
        // SAFETY: `token` is the open handle `OpenProcessToken` returned.
        unsafe { CloseHandle(token) };
        if read == 0 {
            return Err(error);
        }
        Ok(Self { token_user })
    }

    fn sid(&self) -> PSID {
        // SAFETY: `query` filled the buffer with a `TOKEN_USER`, and the
        // buffer is aligned for one.
        unsafe { (*self.token_user.as_ptr().cast::<TOKEN_USER>()).User.Sid }
    }
}

/// Whether the pipe open as `pipe` is owned by the user this process runs
/// as, so its server is that user's.
fn owned_by_current_user(pipe: RawHandle) -> io::Result<bool> {
    let user = CurrentUser::query()?;
    let mut owner: PSID = ptr::null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
    // SAFETY: `pipe` is an open handle for the duration of the call, and
    // the out pointers are valid; the unused ones may be null.
    let status = unsafe {
        GetSecurityInfo(
            pipe as HANDLE,
            SE_KERNEL_OBJECT,
            OWNER_SECURITY_INFORMATION,
            &mut owner,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            &mut descriptor,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    // SAFETY: both SIDs are valid: `owner` points into `descriptor`, which
    // is freed only after the comparison, as its documentation says.
    let same = unsafe {
        let same = EqualSid(owner, user.sid()) != 0;
        LocalFree(descriptor);
        same
    };
    Ok(same)
}
