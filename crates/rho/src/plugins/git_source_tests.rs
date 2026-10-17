//! Install source parsing tests.

use std::{ffi::OsStr, path::PathBuf};

use pretty_assertions::assert_eq;

use super::{GitSource, InstallSource};

fn git(url: &str, git_ref: Option<&str>, subdir: Option<&str>) -> InstallSource {
    InstallSource::Git(GitSource {
        url: url.to_string(),
        git_ref: git_ref.map(str::to_string),
        subdir: subdir.map(str::to_string),
    })
}

/// A cwd with no entries, so URL-shaped arguments never match a local path.
fn missing_cwd() -> PathBuf {
    std::env::temp_dir().join("rho-plugin-source-tests-no-such-dir")
}

// Covers: GitHub browser links, git URLs, and local paths classify correctly.
// Owner: plugin install source parsing.
#[test]
fn parses_install_sources() {
    let repo = "https://github.com/owner/repo";
    let cases = [
        ("https://github.com/owner/repo", git(repo, None, None)),
        ("https://github.com/owner/repo.git", git(repo, None, None)),
        ("github.com/owner/repo/", git(repo, None, None)),
        (
            "https://github.com/owner/repo?tab=readme-ov-file",
            git(repo, None, None),
        ),
        (
            "https://github.com/owner/repo/tree/v1.2.0",
            git(repo, Some("v1.2.0"), None),
        ),
        (
            "https://github.com/owner/repo/tree/main/plugins/demo#readme",
            git(repo, Some("main"), Some("plugins/demo")),
        ),
        (
            "git@gitlab.com:owner/repo.git",
            git("git@gitlab.com:owner/repo.git", None, None),
        ),
        (
            "ssh://git@example.com/owner/repo.git",
            git("ssh://git@example.com/owner/repo.git", None, None),
        ),
        (
            "file:///srv/plugins.git",
            git("file:///srv/plugins.git", None, None),
        ),
        (
            "./plugins/demo",
            InstallSource::Local(PathBuf::from("./plugins/demo")),
        ),
        (
            "/abs/plugin",
            InstallSource::Local(PathBuf::from("/abs/plugin")),
        ),
    ];
    for (arg, expected) in cases {
        assert_eq!(
            InstallSource::parse(OsStr::new(arg), &missing_cwd()).unwrap(),
            expected,
            "{arg}"
        );
    }
}

// Covers: sources that could inject git options, escape the checkout, or
// persist credentials into plugins.toml fail.
// Owner: plugin install source validation.
#[test]
fn rejects_unsafe_or_malformed_sources() {
    let cases = [
        "https://github.com/owner",
        "https://github.com/owner/repo/blob/main/plugin.json",
        "https://github.com/owner/repo/tree/",
        "https://github.com/owner/repo/tree/--upload-pack=evil",
        "https://github.com/owner/repo/tree/main/../outside",
        "https://github.com/owner/repo/tree/main/./plugin",
        "https://user:ghp_secret@github.com/owner/repo",
        "https://ghp_secret@gitlab.com/owner/repo.git",
        "ssh://git:hunter2@example.com/owner/repo.git",
    ];
    for arg in cases {
        assert!(
            InstallSource::parse(OsStr::new(arg), &missing_cwd()).is_err(),
            "{arg}"
        );
    }

    // Recorded sources from plugins.toml go through the same gate.
    let recorded = GitSource {
        url: "ext::sh -c touch% /tmp/pwned".to_string(),
        git_ref: None,
        subdir: None,
    };
    assert!(recorded.validate().is_err());
}

// Covers: an existing local directory wins over URL-shaped parsing.
// Owner: plugin install source parsing.
#[test]
fn existing_local_path_is_never_a_remote() {
    let cwd = tempfile::TempDir::new().unwrap();
    std::fs::create_dir_all(cwd.path().join("github.com/owner/repo")).unwrap();
    assert_eq!(
        InstallSource::parse(OsStr::new("github.com/owner/repo"), cwd.path()).unwrap(),
        InstallSource::Local(PathBuf::from("github.com/owner/repo"))
    );
}
