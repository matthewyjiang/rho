use std::{fs::File, os::fd::AsRawFd as _, process::Stdio};

use super::configure_handle_inheritance;

// Covers: preparing descriptor inheritance must not expose the descriptor to
// concurrent process launches from the parent.
// Owner: secure filesystem process-handle adapter.
#[test]
fn configuring_inheritance_does_not_change_parent_flags() {
    let file = File::open("/").unwrap();
    let fd = file.as_raw_fd();
    // SAFETY: fcntl reads flags from this valid owned descriptor.
    let original_flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    assert_ne!(original_flags, -1);
    let mut command = tokio::process::Command::new("/bin/true");

    configure_handle_inheritance(&mut command, &[&file]).unwrap();

    // SAFETY: fcntl reads flags from this valid owned descriptor.
    let configured_flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    assert_eq!(configured_flags, original_flags);
}

// Covers: a child launched from a descriptor path must receive that descriptor
// while the parent keeps FD_CLOEXEC set before and after spawn.
// Owner: secure filesystem process-handle adapter.
#[tokio::test]
async fn child_receives_descriptor_without_changing_parent_flags() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("payload");
    std::fs::write(&path, b"inherited").unwrap();
    let file = File::open(path).unwrap();
    let fd = file.as_raw_fd();
    // SAFETY: fcntl reads flags from this valid owned descriptor.
    let original_flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    assert_ne!(original_flags & libc::FD_CLOEXEC, 0);
    let mut command = tokio::process::Command::new("/bin/cat");
    command
        .arg(format!("/proc/self/fd/{fd}"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    configure_handle_inheritance(&mut command, &[&file]).unwrap();
    let output = command.output().await.unwrap();

    // SAFETY: fcntl reads flags from this valid owned descriptor.
    let final_flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    assert_eq!(final_flags, original_flags);
    assert!(output.status.success(), "{:?}", output.stderr);
    assert_eq!(output.stdout, b"inherited");
}
