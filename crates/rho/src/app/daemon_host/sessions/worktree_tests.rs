use pretty_assertions::assert_eq;
use rho_protocol::SessionWorktree;
use tempfile::TempDir;

use super::*;
use crate::app::daemon_host::jobs::JobId;

// Covers: a kept worktree turns stale once its session's job has not
// changed for 14 days, and never while a job uses it or once it is gone
// from disk, which is what `rho/job/list` flags for `/jobs` and `rho daemon
// status`.
// Owner: daemon worktree cleanup (stale rule).
#[test]
fn a_kept_worktree_turns_stale_after_fourteen_unused_days() {
    let home = TempDir::new().unwrap();
    let path = home.path().join("repo-1234abcd");
    std::fs::create_dir(&path).unwrap();
    let worktree = SessionWorktree {
        branch: "rho/repo-1234abcd".to_owned(),
        path: path.clone(),
        origin: home.path().join("repo"),
    };
    let now = 2_000_000_000;
    let fourteen_days = STALE_AFTER.as_secs();
    let record = |worktree: Option<&SessionWorktree>, idle: u64| JobRecord {
        updated_at: now - idle,
        worktree: worktree.cloned(),
        ..JobRecord::new_session(
            JobId::generate(),
            path.join("src"),
            rho_sdk::SessionId::new(),
        )
    };
    let unused = InUse(Vec::new());
    let used = InUse(vec![path.join("src")]);
    let gone = SessionWorktree {
        path: home.path().join("removed"),
        ..worktree.clone()
    };
    let cases = [
        (
            "a day short",
            record(Some(&worktree), fourteen_days - 1),
            &unused,
            false,
        ),
        (
            "fourteen days",
            record(Some(&worktree), fourteen_days),
            &unused,
            true,
        ),
        (
            "in use",
            record(Some(&worktree), fourteen_days),
            &used,
            false,
        ),
        (
            "removed",
            record(Some(&gone), fourteen_days),
            &unused,
            false,
        ),
        ("no worktree", record(None, fourteen_days), &unused, false),
    ];

    let stale: Vec<_> = cases
        .iter()
        .map(|(case, record, in_use, _)| (*case, worktree_stale(record, in_use, now)))
        .collect();

    assert_eq!(
        stale,
        cases
            .iter()
            .map(|(case, _, _, stale)| (*case, *stale))
            .collect::<Vec<_>>()
    );
}
