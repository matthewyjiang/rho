//! `rho worktrees prune`: asks the daemon, which knows which sessions are
//! loaded or running, to remove the session worktrees that are clean and
//! merged (#1673), and prints what it did with each, or with `--dry-run`
//! what it would do.

use rho_protocol::{methods, WorktreePruneParams, WorktreeReport, WorktreeVerdict};

use super::daemon_cli;
use crate::cli::WorktreesCommand;

pub(super) async fn run(command: &WorktreesCommand) -> anyhow::Result<()> {
    match command {
        WorktreesCommand::Prune { dry_run } => prune(*dry_run).await,
    }
}

async fn prune(dry_run: bool) -> anyhow::Result<()> {
    let connection = daemon_cli::connect().await?;
    let pruned = connection
        .client
        .request::<methods::WorktreePrune>(&WorktreePruneParams { dry_run })
        .await?;
    print!("{}", prune_text(&pruned.worktrees, dry_run));
    Ok(())
}

/// One line per worktree: what happened to it, its branch and path, and why.
fn prune_text(reports: &[WorktreeReport], dry_run: bool) -> String {
    if reports.is_empty() {
        return "no session worktrees\n".to_owned();
    }
    reports
        .iter()
        .map(|report| {
            let (action, reason) = match &report.verdict {
                WorktreeVerdict::Merged { base } => (
                    if dry_run { "would remove" } else { "removed" },
                    format!("clean and merged into {base}; the branch stays"),
                ),
                WorktreeVerdict::InUse => ("kept", "its session is loaded or running".to_owned()),
                WorktreeVerdict::Dirty => ("kept", "uncommitted or untracked changes".to_owned()),
                WorktreeVerdict::Unmerged { base } => ("kept", format!("not merged into {base}")),
                WorktreeVerdict::Failed { error } => ("kept", error.clone()),
                // `WorktreeVerdict` is non-exhaustive: a newer daemon's.
                _ => (
                    "kept",
                    "the daemon gave a reason this rho does not know".to_owned(),
                ),
            };
            let stale = if report.stale {
                "; stale: unused for 14 days"
            } else {
                ""
            };
            format!(
                "{action:<12} {} {}: {reason}{stale}\n",
                report.worktree.branch,
                report.worktree.path.display()
            )
        })
        .collect()
}
