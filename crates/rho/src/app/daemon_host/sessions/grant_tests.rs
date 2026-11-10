use std::path::PathBuf;

use pretty_assertions::assert_eq;

use super::*;
use crate::caller_context::{Access, Scope, Workspaces};

fn grant(depth: u32, access: Access, roots: Option<&[&str]>) -> CallerContext {
    CallerContext {
        depth,
        scope: Scope {
            access,
            workspaces: roots.map_or(Workspaces::Any, |roots| {
                Workspaces::Only(roots.iter().map(PathBuf::from).collect())
            }),
        },
    }
}

// Covers: a session another session starts never opens with more than a
// session its parent's turn starts may have, whatever the request asks
// for: one level deeper than the parent at least, with no access or
// workspace the parent lacks; a request that asks for less keeps less; and
// a parent the daemon holds no grant for leaves what was asked (#1796).
// Owner: daemon session grants.
#[test]
fn a_child_opens_with_no_more_than_its_parents_turn_could_give() {
    let work = Some(["/work"].as_slice());
    let cases = [
        (
            "asking for everything under a limited parent",
            CallerContext::ROOT,
            Some(grant(1, Access::Write, work)),
            grant(2, Access::Write, work),
        ),
        (
            "asking for everything under a person's session",
            CallerContext::ROOT,
            Some(CallerContext::ROOT),
            grant(1, Access::Write, None),
        ),
        (
            "asking for less than the parent has",
            grant(4, Access::Read, Some(&["/work/deep"])),
            Some(grant(1, Access::Write, work)),
            grant(4, Access::Read, Some(&["/work/deep"])),
        ),
        (
            "asking for a workspace the parent does not reach",
            grant(2, Access::Write, Some(&["/home"])),
            Some(grant(1, Access::Read, work)),
            grant(2, Access::Read, Some(&[])),
        ),
        (
            "a parent no job of the daemon writes",
            grant(1, Access::Write, work),
            None,
            grant(1, Access::Write, work),
        ),
    ];
    for (name, asked, parents, limit) in cases {
        assert_eq!(opening(asked, parents.as_ref()), limit, "{name}");
    }
}
