use std::path::PathBuf;

use pretty_assertions::assert_eq;

use super::*;

fn token(access: Access, allowed: &[&str]) -> Grant {
    Grant {
        holder: Holder::Token("remote".into()),
        access,
        workspaces: Workspaces::Only(allowed.iter().map(PathBuf::from).collect()),
    }
}

// Covers: a read-only grant may watch but not act, and its refusal names
// the holder, the action, and the access it needs.
// Owner: session-control policy.
#[test]
fn access_covers_watching_and_acting() {
    let read = token(Access::Read, &["/work"]);
    let write = token(Access::Write, &["/work"]);
    let permitted: Vec<(Action, bool, bool)> = Action::ALL
        .into_iter()
        .map(|action| {
            (
                action,
                read.permit(action).is_ok(),
                write.permit(action).is_ok(),
            )
        })
        .collect();

    assert_eq!(
        permitted,
        vec![
            (Action::List, true, true),
            (Action::Read, true, true),
            (Action::New, false, true),
            (Action::Prompt, false, true),
            (Action::Steer, false, true),
            (Action::Cancel, false, true),
            (Action::Wait, true, true),
        ]
    );
    assert_eq!(
        read.permit(Action::New).unwrap_err().to_string(),
        "token \"remote\" has read access; `new` needs write"
    );
}

// Covers: a grant reaches its directories and everything under them, not a
// sibling that shares a prefix, and the refusal names the allowed list.
// Owner: session-control policy.
#[test]
fn workspaces_reach_their_subtrees_only() {
    let grant = token(Access::Write, &["/work/a", "/work/b"]);
    let reached: Vec<bool> = ["/work/a", "/work/a/deep", "/work/b", "/work/ab", "/home"]
        .into_iter()
        .map(|workspace| grant.reach(Path::new(workspace)).is_ok())
        .collect();

    assert_eq!(reached, vec![true, true, true, false, false]);
    assert_eq!(
        grant.reach(Path::new("/work/ab")).unwrap_err().to_string(),
        "token \"remote\" may not reach workspace /work/ab; it may reach: /work/a, /work/b"
    );
    assert!(Grant::local().reach(Path::new("/anywhere")).is_ok());
}

// Covers: each cap's refusal names its budget, its limit, and the value
// asked for, so a caller that hits one can tell which and by how much.
// Owner: session-control policy.
#[test]
fn cap_refusals_name_budget_limit_and_asked_value() {
    assert_eq!(
        [
            Refusal::Depth { limit: 3, asked: 4 }.to_string(),
            Refusal::Sessions {
                holder: Holder::Token("remote".into()),
                limit: 2,
                asked: 3,
            }
            .to_string(),
            Refusal::Sessions {
                holder: Holder::Local,
                limit: 10,
                asked: 11,
            }
            .to_string(),
        ],
        [
            "session depth budget: limit 3, asked 4; a session at depth 3 may not start or prompt another"
                .to_owned(),
            "concurrent sessions budget for token \"remote\": limit 2, asked 3".to_owned(),
            "concurrent sessions budget for this server's caller: limit 10, asked 11".to_owned(),
        ]
    );
}
