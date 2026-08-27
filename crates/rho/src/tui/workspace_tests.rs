use std::fs;

use pretty_assertions::assert_eq;
use tempfile::TempDir;

use super::{git_remote_urls, remote_urls_from_config};

#[test]
fn remote_urls_from_config_reads_every_remote() {
    // Covers: GitHub probe must see origin and other remotes from git config
    // Owner: git workspace probe
    let config = r#"
[core]
	repositoryformatversion = 0
[remote "origin"]
	url = git@github.com:org/repo.git
	fetch = +refs/heads/*:refs/remotes/origin/*
[remote "upstream"]
	url = https://github.com/upstream/repo.git
[branch "main"]
	remote = origin
"#;
    assert_eq!(
        remote_urls_from_config(config),
        vec![
            "git@github.com:org/repo.git".to_string(),
            "https://github.com/upstream/repo.git".to_string(),
        ]
    );
}

#[test]
fn git_remote_urls_reads_worktree_commondir_config() {
    // Covers: linked worktrees keep remotes in the common git dir
    // Owner: git workspace probe
    let root = TempDir::new().unwrap();
    let main_git = root.path().join("repo/.git");
    let worktree_git = main_git.join("worktrees/feat");
    fs::create_dir_all(&worktree_git).unwrap();
    fs::write(
        main_git.join("config"),
        "[remote \"origin\"]\n\turl = https://github.com/org/repo.git\n",
    )
    .unwrap();
    fs::write(worktree_git.join("commondir"), "../..\n").unwrap();

    let worktree = root.path().join("feat");
    fs::create_dir(&worktree).unwrap();
    fs::write(
        worktree.join(".git"),
        format!("gitdir: {}\n", worktree_git.display()),
    )
    .unwrap();

    assert_eq!(
        git_remote_urls(&worktree),
        vec!["https://github.com/org/repo.git".to_string()]
    );
}
