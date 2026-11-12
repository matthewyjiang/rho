//! Handing memory the daemon freed back to the system.

/// Asks the allocator to return the memory it holds free to the system.
/// glibc keeps freed memory in its arenas for reuse, so after a session
/// with a long transcript unloads, its pages stay resident although nothing
/// uses them. Other allocators return large blocks themselves, so this does
/// nothing elsewhere.
pub(crate) fn release_freed_memory() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    // SAFETY: malloc_trim takes no pointers and only rearranges glibc's own
    // free lists, which are locked while it runs.
    unsafe {
        libc::malloc_trim(0);
    }
}
