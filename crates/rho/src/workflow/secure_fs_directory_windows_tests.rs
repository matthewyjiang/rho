use std::{path::Path, process::Command};

use super::{open_verified_directory, open_verified_file_in_directory, ContentHash};

// Covers: catalog child names must not escape the held directory through traversal,
// alternate data streams, or intermediate/final junctions. No symlink privilege is needed.
// Owner: Windows secure filesystem reads.
#[test]
fn catalog_reads_reject_windows_path_redirection() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("agents");
    let outside = temporary.path().join("outside");
    std::fs::create_dir(&root).unwrap();
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("worker.md"), "outside").unwrap();
    std::fs::write(root.join("worker.md"), "authorized").unwrap();
    std::fs::write(root.join("worker.md:hidden"), "alternate stream").unwrap();
    let junction = root.join("redirect.md");
    let output = Command::new("cmd.exe")
        .args(["/D", "/C", "mklink", "/J"])
        .arg(&junction)
        .arg(&outside)
        .output()
        .unwrap();
    assert!(output.status.success(), "junction setup failed: {output:?}");
    let opened = open_verified_directory(&root).unwrap();

    let absolute = outside.join("worker.md");
    for relative in [
        Path::new(""),
        Path::new("..\\outside\\worker.md"),
        Path::new("C:worker.md"),
        absolute.as_path(),
        Path::new("worker.md:hidden"),
        Path::new("worker.md\0ignored"),
        Path::new("redirect.md"),
        Path::new("redirect.md\\worker.md"),
    ] {
        assert!(
            open_verified_file_in_directory(&opened, relative, ContentHash::Skip).is_err(),
            "accepted redirected catalog read: {relative:?}"
        );
    }
}
