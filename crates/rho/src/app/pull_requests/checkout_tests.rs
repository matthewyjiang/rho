use std::{path::Path, process::Command};

use pretty_assertions::assert_eq;
use tempfile::TempDir;

use super::{resolve, Checkout};
use crate::app::pull_requests::model::{BranchId, RepoId};

/// Runs `git args` in `dir`, isolated from the caller's repository and
/// config, and returns its trimmed stdout.
fn git(dir: &Path, args: &[&str]) -> String {
    let mut command = Command::new("git");
    for name in crate::git::LOCATION_ENV {
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

/// How a test's workspace is made.
enum Given {
    /// A plain directory.
    Plain,
    /// A repository on branch `feature`, with `origin` at this URL if any.
    OnFeature { origin: Option<&'static str> },
    /// A repository with its head detached at its one commit.
    Detached,
}

/// Makes `workspace` as `given` says; returns the commit a detached head
/// is at, as `git rev-parse` abbreviates it.
fn make(workspace: &Path, given: &Given) -> Option<String> {
    match given {
        Given::Plain => None,
        Given::OnFeature { origin } => {
            git(workspace, &["init", "--quiet", "--initial-branch=feature"]);
            if let Some(url) = *origin {
                git(workspace, &["remote", "add", "origin", url]);
            }
            None
        }
        Given::Detached => {
            git(workspace, &["init", "--quiet", "--initial-branch=main"]);
            git(
                workspace,
                &["commit", "--quiet", "--allow-empty", "-m", "base"],
            );
            git(workspace, &["checkout", "--quiet", "--detach"]);
            Some(git(workspace, &["rev-parse", "--short", "HEAD"]))
        }
    }
}

// Covers: a workspace names its checkout: no repository, a branch of a
// github.com repository, a detached head, and a branch whose origin is
// missing, on another forge, or on none. A detached head is not "no
// repository", and a branch is looked up only in the github.com repository
// its origin names, so `gh` never sends a sign-in to another host.
// Owner: OS/process (git)
#[tokio::test]
async fn a_workspace_names_its_checkout() {
    let feature = || "feature".to_owned();
    let cases = [
        ("not a repository", Given::Plain, Checkout::NotGit),
        (
            "a branch with a forge origin",
            Given::OnFeature {
                origin: Some("git@github.com:acme/widgets.git"),
            },
            Checkout::Branch(BranchId {
                repo: RepoId {
                    host: "github.com".to_owned(),
                    owner: "acme".to_owned(),
                    name: "widgets".to_owned(),
                },
                branch: feature(),
            }),
        ),
        (
            "a detached head",
            Given::Detached,
            // At the commit `make` made, filled in below.
            Checkout::Detached {
                commit: String::new(),
            },
        ),
        (
            "no origin",
            Given::OnFeature { origin: None },
            Checkout::NoForge { branch: feature() },
        ),
        (
            "an origin on another forge than github.com",
            Given::OnFeature {
                origin: Some("https://gitlab.com/acme/widgets.git"),
            },
            Checkout::NoForge { branch: feature() },
        ),
        (
            "an origin that is a local path",
            Given::OnFeature {
                origin: Some("/srv/git/widgets.git"),
            },
            Checkout::NoForge { branch: feature() },
        ),
    ];
    for (name, given, mut expected) in cases {
        let dir = TempDir::new().unwrap();
        let head = make(dir.path(), &given);
        if let (Checkout::Detached { commit }, Some(head)) = (&mut expected, head) {
            *commit = head;
        }

        assert_eq!(resolve(dir.path()).await, expected, "{name}");
    }
}
