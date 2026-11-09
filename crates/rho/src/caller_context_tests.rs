use std::{collections::HashMap, ffi::OsString, path::PathBuf};

use pretty_assertions::assert_eq;

use super::*;

fn only(roots: &[&str]) -> Workspaces {
    Workspaces::Only(roots.iter().map(PathBuf::from).collect())
}

fn scope(access: Access, workspaces: Workspaces) -> Scope {
    Scope { access, workspaces }
}

// Covers: a scope passed down only narrows: the lesser access, and of two
// nested workspace roots the deeper one, so neither side's limit is lost.
// Owner: caller-context scope.
#[test]
fn scopes_intersect_to_what_both_allow() {
    let cases = [
        (
            Scope::FULL,
            scope(Access::Read, only(&["/work"])),
            scope(Access::Read, only(&["/work"])),
        ),
        (
            scope(Access::Write, only(&["/work/a", "/work/b"])),
            scope(Access::Write, only(&["/work/a/deep", "/work", "/home"])),
            scope(Access::Write, only(&["/work/a", "/work/a/deep", "/work/b"])),
        ),
        (
            scope(Access::Write, only(&["/work/a"])),
            scope(Access::Write, only(&["/work/ab"])),
            scope(Access::Write, only(&[])),
        ),
    ];
    for (left, right, both) in cases {
        assert_eq!(
            (left.intersect(&right), right.intersect(&left)),
            (both.clone(), both)
        );
    }
}

// Covers: the context a parent writes into a child's environment is the one
// the child reads back; unset variables read as a person's context, and a
// set variable that does not parse is an error rather than no limit.
// Owner: caller-context environment encoding.
#[test]
fn a_context_reads_back_from_the_environment_it_wrote() {
    let read = |variables: &[(&str, &str)]| {
        let variables: HashMap<String, OsString> = variables
            .iter()
            .map(|(name, value)| ((*name).to_owned(), OsString::from(value)))
            .collect();
        CallerContext::from_env(|name| variables.get(name).cloned())
            .map_err(|error| error.to_string())
    };
    let restricted = CallerContext {
        depth: 2,
        scope: scope(Access::Read, only(&["/work"])),
    };
    let written: Vec<(&str, String)> = restricted.env();
    let written: Vec<(&str, &str)> = written
        .iter()
        .map(|(name, value)| (*name, value.as_str()))
        .collect();

    assert_eq!(
        [
            read(&written),
            read(&[]),
            read(&[(SESSION_DEPTH_ENV, "two")]),
            read(&[(SESSION_SCOPE_ENV, "write")]),
        ],
        [
            Ok(restricted),
            Ok(CallerContext::ROOT),
            Err("RHO_SESSION_DEPTH=\"two\" is not a depth".into()),
            Err("RHO_SESSION_SCOPE=\"write\" is not a scope".into()),
        ]
    );
    assert_eq!(
        CallerContext::ROOT.env(),
        vec![(SESSION_DEPTH_ENV, "0".to_owned())]
    );
}
