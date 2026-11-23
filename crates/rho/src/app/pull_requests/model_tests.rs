use pretty_assertions::assert_eq;

use super::{PrId, RepoId};

fn repo(host: &str, owner: &str, name: &str) -> RepoId {
    RepoId {
        host: host.to_owned(),
        owner: owner.to_owned(),
        name: name.to_owned(),
    }
}

fn pr(owner: &str, name: &str, number: u32) -> PrId {
    PrId {
        repo: repo("github.com", owner, name),
        number,
    }
}

// Covers: a workspace's origin remote names the repository `#N` resolves in.
// Owner: pure unit (model)
#[test]
fn remote_urls_name_their_repository() {
    let cases = [
        (
            "https://github.com/acme/widgets",
            Some(repo("github.com", "acme", "widgets")),
        ),
        (
            "https://github.com/acme/widgets.git/",
            Some(repo("github.com", "acme", "widgets")),
        ),
        (
            "git@github.com:acme/widgets.git",
            Some(repo("github.com", "acme", "widgets")),
        ),
        (
            "ssh://git@GHE.corp:2222/acme/widgets.git",
            Some(repo("ghe.corp", "acme", "widgets")),
        ),
        (
            "https://me:token@github.com/acme/widgets",
            Some(repo("github.com", "acme", "widgets")),
        ),
        ("/srv/git/widgets.git", None),
        ("file:///srv/git/widgets.git", None),
        ("https://github.com/widgets", None),
    ];
    for (url, expected) in cases {
        assert_eq!(RepoId::from_url(url), expected, "{url}");
    }
}

// Covers: the `pull_request` argument the agent tools take names the right PR,
// and a bare number needs the workspace's repository.
// Owner: pure unit (model)
#[test]
fn text_names_its_pull_request() {
    let widgets = repo("github.com", "acme", "widgets");
    let enterprise = repo("ghe.corp", "acme", "widgets");
    let cases = [
        (
            "https://github.com/up/stream/pull/7",
            Some(&widgets),
            Some(pr("up", "stream", 7)),
        ),
        (
            "https://GitHub.com/up/stream/pull/7/files?w=1",
            Some(&widgets),
            Some(pr("up", "stream", 7)),
        ),
        (
            "https://github.com/up/stream/pull/7#issuecomment-1",
            Some(&widgets),
            Some(pr("up", "stream", 7)),
        ),
        ("up/stream#7", Some(&widgets), Some(pr("up", "stream", 7))),
        ("up/stream#7", None, Some(pr("up", "stream", 7))),
        (
            "up/stream#7",
            Some(&enterprise),
            Some(PrId {
                repo: repo("ghe.corp", "up", "stream"),
                number: 7,
            }),
        ),
        (" #12 ", Some(&widgets), Some(pr("acme", "widgets", 12))),
        ("12", Some(&widgets), Some(pr("acme", "widgets", 12))),
        ("#12", None, None),
        (
            "https://github.com/up/stream/issues/7",
            Some(&widgets),
            None,
        ),
        ("#0", Some(&widgets), None),
        ("stream#7", Some(&widgets), None),
        ("widgets", Some(&widgets), None),
    ];
    for (text, repo, expected) in cases {
        assert_eq!(PrId::parse(text, repo).ok(), expected, "{text} in {repo:?}");
    }
}
