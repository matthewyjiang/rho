use tempfile::TempDir;

use super::size;

// Covers: a worktree's size sums its files in nested directories, counts
// a file Cargo-style hard links twice only once, and never follows a
// symlink out of it, so `rho worktrees status` reports what removing it
// frees rather than more.
// Owner: worktree disk accounting.
#[cfg(unix)]
#[tokio::test]
async fn size_counts_each_file_once_and_stays_inside() {
    let home = TempDir::new().unwrap();
    let worktree = home.path().join("worktree");
    let outside = home.path().join("outside");
    std::fs::create_dir_all(worktree.join("target/debug/deps")).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(worktree.join("lib.rs"), [0; 100]).unwrap();
    std::fs::write(worktree.join("target/debug/deps/rho-1234"), [0; 1000]).unwrap();
    std::fs::hard_link(
        worktree.join("target/debug/deps/rho-1234"),
        worktree.join("target/debug/rho"),
    )
    .unwrap();
    std::fs::write(outside.join("big"), [0; 10_000]).unwrap();
    std::os::unix::fs::symlink(&outside, worktree.join("link")).unwrap();
    let link = std::fs::symlink_metadata(worktree.join("link"))
        .unwrap()
        .len();

    assert_eq!(size(&worktree).await, 100 + 1000 + link);
}
