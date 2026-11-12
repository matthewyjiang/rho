//! Git repositories for the tests whose sessions work in worktrees (#1673).

use std::{path::Path, process::Command};

/// Inherited variables that would point `git` at another repository, as
/// when the tests run inside a git hook.
const LOCATION_ENV: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_COMMON_DIR",
    "GIT_NAMESPACE",
];

/// Runs `git args` in `dir` with an identity and no signing, whatever the
/// machine's config, and returns its trimmed stdout.
pub(super) fn git(dir: &Path, args: &[&str]) -> String {
    let mut command = Command::new("git");
    for name in LOCATION_ENV {
        command.env_remove(name);
    }
    let output = command
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=rho",
            "-c",
            "user.email=rho@example.com",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

/// Makes `dir` a git repository whose one commit has `shared.txt` say
/// "base".
pub(super) fn init_repo(dir: &Path) {
    git(dir, &["init", "--quiet", "--initial-branch=main"]);
    std::fs::write(dir.join("shared.txt"), "base").unwrap();
    git(dir, &["add", "shared.txt"]);
    git(dir, &["commit", "--quiet", "-m", "base"]);
}
