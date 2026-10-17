//! Install source parsing tests.

use std::{ffi::OsStr, path::PathBuf};

use pretty_assertions::assert_eq;

use super::{GitSource, InstallSource};
use crate::plugins::state::GitRecord;

fn git(url: &str, git_ref: Option<&str>, subdir: Option<&str>) -> InstallSource {
    let source = GitSource::new(
        url.to_string(),
        git_ref.map(str::to_string),
        subdir.map(str::to_string),
    );
    InstallSource::Git(source.unwrap())
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
        ("www.github.com/owner/repo", git(repo, None, None)),
        (
            "https://github.com/owner/repo/tree/feature%2Fx/plugins/my%20plugin",
            git(repo, Some("feature/x"), Some("plugins/my plugin")),
        ),
        (
            "HTTP://WWW.GitHub.com/owner/repo/tree/main/plugins/demo",
            git(repo, Some("main"), Some("plugins/demo")),
        ),
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
            "git@github.com:1234/repo.git",
            git("git@github.com:1234/repo.git", None, None),
        ),
        (
            "SSH://git@example.com/owner/repo.git",
            git("ssh://git@example.com/owner/repo.git", None, None),
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
        "https://github.com/owner/repo/tree/main/%2e%2e/outside",
        "https://github.com/owner/repo/tree/%2Dbad",
        "ftp://example.com/owner/repo.git",
        "https://user:ghp_secret@github.com/owner/repo",
        "https://ghp_secret@github.com/owner/repo",
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
    let recorded = |url: &str, subdir: Option<&str>| GitRecord {
        url: url.to_string(),
        git_ref: None,
        subdir: subdir.map(str::to_string),
        commit: "0".repeat(40),
    };
    for record in [
        recorded("ext::sh -c touch% /tmp/pwned", None),
        recorded("https://github.com/owner/repo", Some("../escape")),
    ] {
        assert!(GitSource::try_from(&record).is_err(), "{record:?}");
    }
}

// Covers: an existing local directory wins over scheme-less GitHub parsing,
// but a planted `https:/...` directory never stands in for a real URL.
// Owner: plugin install source parsing.
#[test]
fn local_paths_win_only_without_a_scheme() {
    let cwd = tempfile::TempDir::new().unwrap();
    std::fs::create_dir_all(cwd.path().join("github.com/owner/repo")).unwrap();
    assert_eq!(
        InstallSource::parse(OsStr::new("github.com/owner/repo"), cwd.path()).unwrap(),
        InstallSource::Local(PathBuf::from("github.com/owner/repo"))
    );

    #[cfg(unix)]
    {
        for arg in [
            "https://github.com/owner/repo",
            "HTTPS://github.com/owner/repo",
        ] {
            let planted = arg.replacen("://", ":/", 1);
            std::fs::create_dir_all(cwd.path().join(planted)).unwrap();
            assert_eq!(
                InstallSource::parse(OsStr::new(arg), cwd.path()).unwrap(),
                git("https://github.com/owner/repo", None, None),
                "{arg}"
            );
        }
    }
}
