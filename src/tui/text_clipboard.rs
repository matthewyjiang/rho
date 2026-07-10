use std::io;

pub(super) fn copy_text(text: &str) -> io::Result<()> {
    copy_text_to_system_clipboard(text)
}

#[cfg(not(windows))]
fn copy_text_to_system_clipboard(text: &str) -> io::Result<()> {
    use crossterm::{clipboard::CopyToClipboard, execute};

    let mut stdout = io::stdout();
    execute!(stdout, CopyToClipboard::to_clipboard_from(text))
}

#[cfg(any(windows, test))]
fn utf16_with_nul(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
fn copy_text_to_system_clipboard(text: &str) -> io::Result<()> {
    use std::{ffi::c_void, ptr};

    use windows_sys::Win32::{
        Foundation::GlobalFree,
        System::{
            DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData},
            Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE},
            Ole::CF_UNICODETEXT,
        },
    };

    let utf16 = utf16_with_nul(text);
    let bytes = std::mem::size_of_val(utf16.as_slice());

    // SAFETY: GlobalAlloc allocates a movable memory block. Its ownership remains with this
    // function until SetClipboardData succeeds, after which the system owns it.
    let memory = unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes) };
    if memory.is_null() {
        return Err(io::Error::last_os_error());
    }

    // SAFETY: memory is a valid allocation of `bytes` bytes and utf16 has exactly `bytes` bytes.
    let destination = unsafe { GlobalLock(memory) }.cast::<u16>();
    if destination.is_null() {
        // SAFETY: SetClipboardData has not taken ownership of memory.
        unsafe { GlobalFree(memory) };
        return Err(io::Error::last_os_error());
    }
    // SAFETY: destination points to an allocation large enough for utf16 and does not overlap it.
    unsafe { ptr::copy_nonoverlapping(utf16.as_ptr(), destination, utf16.len()) };
    // SAFETY: GlobalLock succeeded for memory, so it must be unlocked before passing it on.
    unsafe { GlobalUnlock(memory) };

    // SAFETY: A null owner is valid for console applications. The clipboard is closed on every
    // branch after this call succeeds.
    if unsafe { OpenClipboard(ptr::null_mut()) } == 0 {
        // SAFETY: SetClipboardData has not taken ownership of memory.
        unsafe { GlobalFree(memory) };
        return Err(io::Error::last_os_error());
    }
    // SAFETY: This thread owns the open clipboard.
    if unsafe { EmptyClipboard() } == 0 {
        // SAFETY: This thread owns the open clipboard and SetClipboardData has not taken memory.
        unsafe { CloseClipboard() };
        // SAFETY: SetClipboardData has not taken ownership of memory.
        unsafe { GlobalFree(memory) };
        return Err(io::Error::last_os_error());
    }

    // SAFETY: This thread owns the open clipboard and memory contains a null-terminated UTF-16
    // string. On success, Windows owns memory.
    if unsafe { SetClipboardData(CF_UNICODETEXT as u32, memory.cast::<c_void>()) }.is_null() {
        // SAFETY: This thread owns the open clipboard and SetClipboardData has not taken memory.
        unsafe { CloseClipboard() };
        // SAFETY: SetClipboardData has not taken ownership of memory.
        unsafe { GlobalFree(memory) };
        return Err(io::Error::last_os_error());
    }

    // SAFETY: This thread owns the open clipboard. The copy is complete even if closing reports
    // an error, so do not turn a successful copy into a failed notification.
    unsafe { CloseClipboard() };
    Ok(())
}

#[cfg(test)]
#[path = "text_clipboard_tests.rs"]
mod tests;
