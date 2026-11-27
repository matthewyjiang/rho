//! Install source parsing tests.

use std::{collections::BTreeMap, ffi::OsStr, path::PathBuf};

use pretty_assertions::assert_eq;

use super::{Git, GitSource, InstallSource, SshCommand, BATCH_SSH_COMMAND};
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

// Covers: a git source the daemon parses from `rho/plugins/install`'s
// `url` reads as the CLI reads it, but a path never stands in for a URL,
// so a client's URL cannot name a directory on the daemon's machine.
// Owner: plugin install source parsing.
#[test]
fn git_urls_parse_without_reading_paths() {
    let repo = "https://github.com/owner/repo";
    for (url, expected) in [
        ("github.com/owner/repo", git(repo, None, None)),
        (
            "HTTPS://github.com/owner/repo/tree/main/plugins/demo",
            git(repo, Some("main"), Some("plugins/demo")),
        ),
        (
            "file:///srv/plugins.git",
            git("file:///srv/plugins.git", None, None),
        ),
    ] {
        assert_eq!(
            GitSource::parse(url).map(InstallSource::Git).unwrap(),
            expected,
            "{url}"
        );
    }
    for url in [
        "./plugins/demo",
        "/abs/plugin",
        "ftp://example.com/owner/repo.git",
        "https://ghp_secret@github.com/owner/repo",
    ] {
        assert!(GitSource::parse(url).is_err(), "{url}");
    }
}

// Covers: the git a daemon fetch runs has no way left to ask anyone
// anything: no terminal prompt, no askpass program from the environment or
// `core.askPass` (which also stops git falling back to `SSH_ASKPASS`), no
// ssh askpass or display, no Git Credential Manager window, and ssh in
// batch mode unless the user chose the ssh command, which is kept. The CLI's
// git is left free to prompt.
// Owner: plugin git fetch.
#[test]
fn a_git_that_may_not_prompt_has_nothing_to_prompt_with() {
    let path = std::path::Path::new("/plugins/repo");
    let removed = ["GIT_ASKPASS", "SSH_ASKPASS", "DISPLAY", "WAYLAND_DISPLAY"];
    let forbidding = |ssh_command: Option<&'static str>| {
        let mut env: BTreeMap<&str, Option<&str>> =
            removed.into_iter().map(|name| (name, None)).collect();
        env.extend([
            ("GIT_TERMINAL_PROMPT", Some("0")),
            ("SSH_ASKPASS_REQUIRE", Some("never")),
            ("GCM_INTERACTIVE", Some("never")),
        ]);
        if let Some(command) = ssh_command {
            env.insert("GIT_SSH_COMMAND", Some(command));
        }
        (env, vec!["-c", "core.askPass="])
    };
    let cases = [
        (None, (BTreeMap::new(), Vec::new())),
        (
            Some(SshCommand::Default),
            forbidding(Some(BATCH_SSH_COMMAND)),
        ),
        (Some(SshCommand::UserConfigured), forbidding(None)),
    ];
    for (prompts, (env, before_subcommand)) in cases {
        let git = Git {
            repo: path,
            no_hooks: path,
            prompts,
        };
        let command = git.command(&["fetch"]);
        let actual_env: BTreeMap<&str, Option<&str>> = command
            .get_envs()
            .map(|(name, value)| (name.to_str().unwrap(), value.map(|v| v.to_str().unwrap())))
            .filter(|(name, _)| !crate::git::LOCATION_ENV.contains(name))
            .collect();
        let args: Vec<&str> = command
            .get_args()
            .map(|arg| arg.to_str().unwrap())
            .collect();
        let mut expected_args = before_subcommand;
        expected_args.extend([
            "-C",
            "/plugins/repo",
            "-c",
            "core.hooksPath=/plugins/repo",
            "fetch",
        ]);
        assert_eq!((actual_env, args), (env, expected_args), "{prompts:?}");
    }
}
