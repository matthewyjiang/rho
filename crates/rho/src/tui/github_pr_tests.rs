use pretty_assertions::assert_eq;

use super::{
    host_is_github, parse_gh_pr_view, remote_host, remote_is_github, GithubPr, GithubPrTone,
};

#[test]
fn remote_host_parses_git_url_shapes() {
    // Covers: GitHub detection must read host from scp, ssh, and https remotes
    // Owner: github pr probe
    let cases = [
        ("git@github.com:org/repo.git", Some("github.com")),
        ("org-123@github.com:org/repo.git", Some("github.com")),
        ("ssh://git@github.com/org/repo.git", Some("github.com")),
        ("https://github.com/org/repo.git", Some("github.com")),
        (
            "https://user:token@github.com/org/repo.git",
            Some("github.com"),
        ),
        (
            "ssh://git@github.example.com:22/org/repo.git",
            Some("github.example.com"),
        ),
        ("/local/path/to/repo", None),
        ("file:///local/path/to/repo", None),
    ];
    for (url, expected) in cases {
        assert_eq!(remote_host(url), expected, "{url}");
    }
}

#[test]
fn remote_is_github_accepts_github_and_ghe_hosts() {
    // Covers: GitHub.com, github-labeled hosts, and GHE Cloud must probe; others must not
    // Owner: github pr probe
    let github = [
        "git@github.com:org/repo.git",
        "https://github.com/org/repo.git",
        "https://gist.github.com/org/repo.git",
        "git@github.mycompany.com:org/repo.git",
        "https://company.ghe.com/org/repo.git",
    ];
    let other = [
        "git@gitlab.com:org/repo.git",
        "https://bitbucket.org/org/repo.git",
        "git@git.mycompany.com:org/repo.git",
        "/local/path/to/repo",
    ];
    for url in github {
        assert!(remote_is_github(url), "{url}");
    }
    for url in other {
        assert!(!remote_is_github(url), "{url}");
    }
    assert!(host_is_github("github.com"));
    assert!(!host_is_github("notgithub.com"));
}

#[test]
fn parse_gh_pr_view_maps_ready_and_issue_tones() {
    // Covers: statusline color is ready vs issues vs ambient from gh JSON
    // Owner: github pr probe
    let cases = [
        (
            r#"{"number":12,"reviewDecision":null,"mergeStateStatus":"CLEAN","statusCheckRollup":[]}"#,
            Some(GithubPr {
                number: 12,
                tone: Some(GithubPrTone::Ready),
            }),
        ),
        (
            r#"{"number":4,"reviewDecision":"CHANGES_REQUESTED","mergeStateStatus":"BLOCKED","statusCheckRollup":[]}"#,
            Some(GithubPr {
                number: 4,
                tone: Some(GithubPrTone::Issues),
            }),
        ),
        (
            r#"{"number":8,"reviewDecision":null,"mergeStateStatus":"DIRTY","statusCheckRollup":[]}"#,
            Some(GithubPr {
                number: 8,
                tone: Some(GithubPrTone::Issues),
            }),
        ),
        (
            r#"{"number":9,"reviewDecision":null,"mergeStateStatus":"UNSTABLE","statusCheckRollup":[{"conclusion":"FAILURE","state":""}]}"#,
            Some(GithubPr {
                number: 9,
                tone: Some(GithubPrTone::Issues),
            }),
        ),
        (
            r#"{"number":3,"reviewDecision":null,"mergeStateStatus":"BLOCKED","statusCheckRollup":null}"#,
            Some(GithubPr {
                number: 3,
                tone: None,
            }),
        ),
        ("not json", None),
    ];
    for (json, expected) in cases {
        assert_eq!(parse_gh_pr_view(json.as_bytes()), expected, "{json}");
    }
}
