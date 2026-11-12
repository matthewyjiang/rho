//! Handing memory the daemon freed back to the system.

/// Does nothing: Windows has no counterpart to the glibc trim the daemon
/// asks for on Linux.
pub(crate) fn release_freed_memory() {}
