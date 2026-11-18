use std::process::{Command, Stdio};

use tempfile::TempDir;

use super::scan;

// Covers: a process working in a directory shows up with that directory,
// which is what keeps cleanup from removing a worktree a shell or build
// still works in.
// Owner: process working-directory scan (Linux, Android, macOS).
#[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
#[tokio::test]
async fn a_running_process_is_found_in_its_working_directory() {
    let dir = TempDir::new().unwrap();
    let dir = crate::paths::canonicalize(dir.path()).unwrap();
    // Waits on its stdin, so it works there until the scan is done.
    let mut child = Command::new("cat")
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .unwrap();

    let found = scan().await.map(|cwds| cwds.contains(&dir));
    drop(child.stdin.take());
    child.wait().unwrap();

    assert_eq!(found, Some(true));
}
