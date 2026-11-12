//! Who may open the daemon's pipes: the user who runs the daemon, and
//! nobody else.
//!
//! A pipe's name is visible to every account on the machine, so both ends
//! check the other. The server creates each pipe with a security descriptor
//! that grants access to its own user only, and owned by that user. A client
//! then verifies the pipe it opened is owned by its own user before it sends
//! anything: another account that created a pipe of the same name first
//! cannot make itself that pipe's owner.

use std::{io, os::windows::io::RawHandle, ptr};

use windows_sys::Win32::{
    Foundation::{CloseHandle, LocalFree, ERROR_SUCCESS, HANDLE},
    Security::{
        Authorization::{
            ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
            GetSecurityInfo, SDDL_REVISION_1, SE_KERNEL_OBJECT,
        },
        EqualSid, GetTokenInformation, TokenUser, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR,
        PSID, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
    },
    System::Threading::{GetCurrentProcess, OpenProcessToken},
};

/// The user this process runs as.
struct CurrentUser {
    /// A `TOKEN_USER` and the SID it points to, as `GetTokenInformation`
    /// wrote them. `u64` elements keep the structure aligned.
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

    /// The SID in the form a security descriptor string names it.
    fn sid_string(&self) -> io::Result<String> {
        let mut string = ptr::null_mut();
        // SAFETY: the SID is valid for as long as `self`, and `string` is a
        // valid place for the pointer.
        if unsafe { ConvertSidToStringSidW(self.sid(), &mut string) } == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the call returned a NUL-terminated string, which is read
        // up to its terminator and then freed as its documentation says.
        let sid = unsafe {
            let len = (0..).take_while(|&at| *string.add(at) != 0).count();
            let sid = String::from_utf16_lossy(std::slice::from_raw_parts(string, len));
            LocalFree(string.cast());
            sid
        };
        Ok(sid)
    }
}

/// A security descriptor that makes a pipe the current user's alone: owned
/// by that user, with full access for that user and none for anyone else.
pub(super) struct OwnerOnly {
    descriptor: PSECURITY_DESCRIPTOR,
}

// SAFETY: the descriptor is memory this value owns, never written after
// `new` and freed once, on drop.
unsafe impl Send for OwnerOnly {}
// SAFETY: as above; shared references only read it.
unsafe impl Sync for OwnerOnly {}

impl OwnerOnly {
    pub(super) fn new() -> io::Result<Self> {
        let sid = CurrentUser::query()?.sid_string()?;
        // Owner and group are the user; the protected DACL has one entry,
        // which allows the user everything, so nothing is inherited and no
        // one else is granted anything.
        let sddl: Vec<u16> = format!("O:{sid}G:{sid}D:P(A;;GA;;;{sid})")
            .encode_utf16()
            .chain([0])
            .collect();
        let mut descriptor = ptr::null_mut();
        // SAFETY: `sddl` is NUL-terminated, and `descriptor` is a valid
        // place for the pointer.
        let converted = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                ptr::null_mut(),
            )
        };
        if converted == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { descriptor })
    }

    /// The attributes to create a pipe with. They point into `self`, so
    /// they must not outlive it.
    pub(super) fn attributes(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.descriptor,
            bInheritHandle: 0,
        }
    }
}

impl Drop for OwnerOnly {
    fn drop(&mut self) {
        // SAFETY: the descriptor was allocated by the conversion in `new`,
        // which documents `LocalFree` as how to free it.
        unsafe { LocalFree(self.descriptor) };
    }
}

/// Whether the pipe open as `pipe` is owned by the user this process runs
/// as, so its server is that user's.
pub(super) fn owned_by_current_user(pipe: RawHandle) -> io::Result<bool> {
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
