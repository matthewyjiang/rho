//! Install source parsing tests.

use std::path::PathBuf;

use pretty_assertions::assert_eq;

use super::{GitSource, InstallSource};

fn git(url: &str, git_ref: Option<&str>, subdir: Option<&str>) -> InstallSource {
    InstallSource::Git(GitSource {
        url: url.to_string(),
        git_ref: git_ref.map(str::to_string),
        subdir: subdir.map(str::to_string),
    })
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
        assert_eq!(InstallSource::parse(arg).unwrap(), expected, "{arg}");
    }
}

// Covers: sources that could inject git options or escape the checkout fail.
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
    ];
    for arg in cases {
        assert!(InstallSource::parse(arg).is_err(), "{arg}");
    }

    // Recorded sources from plugins.toml go through the same gate.
    let recorded = GitSource {
        url: "ext::sh -c touch% /tmp/pwned".to_string(),
        git_ref: None,
        subdir: None,
    };
    assert!(recorded.validate().is_err());
}
