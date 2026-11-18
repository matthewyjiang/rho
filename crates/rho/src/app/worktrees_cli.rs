//! `rho worktrees`: asks the daemon, which knows which sessions are loaded
//! or running, about the worktrees in its worktree directory (#1673).
//! `prune` removes those that are clean and merged and prints what it did
//! with each, or with `--dry-run` what it would do; `status` lists each
//! with the space it takes and what pruning would free.

use rho_protocol::{methods, WorktreePruneParams, WorktreeReport, WorktreeVerdict};

use super::daemon_cli::{self, Reach};
use crate::cli::WorktreesCommand;
use crate::daemon::ConnectAs;

pub(super) async fn run(command: &WorktreesCommand) -> anyhow::Result<()> {
    match command {
        WorktreesCommand::Prune { dry_run } => {
            let reports = ask(*dry_run, /*measure*/ false).await?;
            print!("{}", prune_text(&reports, *dry_run));
        }
        WorktreesCommand::Status => {
            let reports = ask(/*dry_run*/ true, /*measure*/ true).await?;
            print!("{}", status_text(&reports));
        }
    }
    Ok(())
}

/// The daemon's report of a prune, removing nothing when `dry_run`, and
/// with each worktree's size when it does and `measure`.
async fn ask(dry_run: bool, measure: bool) -> anyhow::Result<Vec<WorktreeReport>> {
    // A dry run must change nothing, so it only asks a daemon already
    // running: one started here would run its own pass a minute later.
    let reach = match dry_run {
        true => Reach::Running,
        false => Reach::StartIfNeeded,
    };
    let connection = daemon_cli::connect_with(reach, ConnectAs::Command).await?;
    let pruned = connection
        .client
        .request::<methods::WorktreePrune>(&WorktreePruneParams { dry_run, measure })
        .await?;
    Ok(pruned.worktrees)
}

/// Why a prune removes the worktree, or keeps it.
fn reason(report: &WorktreeReport) -> String {
    let reason = match &report.verdict {
        WorktreeVerdict::Merged { base } if report.detached => {
            format!("clean and merged into {base}; detached, so no branch stays")
        }
        WorktreeVerdict::Merged { base } => {
            format!("clean and merged into {base}; the branch stays")
        }
        WorktreeVerdict::InUse => "its session is loaded or running".to_owned(),
        WorktreeVerdict::Occupied => "a process works in it".to_owned(),
        WorktreeVerdict::Recent => "used within the last day".to_owned(),
        WorktreeVerdict::Locked => "locked with `git worktree lock`".to_owned(),
        WorktreeVerdict::Dirty => "uncommitted or untracked changes".to_owned(),
        WorktreeVerdict::Unmerged { base } => format!("not merged into {base}"),
        WorktreeVerdict::Failed { error } => error.clone(),
        // `WorktreeVerdict` is non-exhaustive: a newer daemon's.
        _ => "the daemon gave a reason this rho does not know".to_owned(),
    };
    let stale = if report.stale { "; stale" } else { "" };
    let unowned = match report.job {
        Some(_) => "",
        None => "; no session owns it",
    };
    format!("{reason}{stale}{unowned}")
}

/// One line per worktree: what happened to it, its branch and path, and why.
fn prune_text(reports: &[WorktreeReport], dry_run: bool) -> String {
    if reports.is_empty() {
        return "no worktrees\n".to_owned();
    }
    reports
        .iter()
        .map(|report| {
            let action = match (report.verdict.removes(), dry_run) {
                (true, true) => "would remove",
                (true, false) => "removed",
                (false, _) => "kept",
            };
            format!(
                "{action:<12} {} {}: {}\n",
                report.worktree.branch,
                report.worktree.path.display(),
                reason(report)
            )
        })
        .collect()
}

/// One line per worktree, largest first: its size, whether `rho worktrees
/// prune` would remove it, its branch and path, and why; then the totals.
fn status_text(reports: &[WorktreeReport]) -> String {
    if reports.is_empty() {
        return "no worktrees\n".to_owned();
    }
    let mut rows: Vec<_> = reports.iter().collect();
    rows.sort_by_key(|report| std::cmp::Reverse(report.bytes));
    let mut text = String::new();
    let mut total = Bytes::default();
    let mut freed = Bytes::default();
    let mut removable = 0;
    for report in rows {
        let removes = report.verdict.removes();
        total.add(report.bytes);
        if removes {
            freed.add(report.bytes);
            removable += 1;
        }
        let size = report.bytes.map_or_else(|| "?".to_owned(), human_bytes);
        let action = if removes { "removable" } else { "kept" };
        text += &format!(
            "{size:>10}  {action:<9} {} {}: {}\n",
            report.worktree.branch,
            report.worktree.path.display(),
            reason(report)
        );
    }
    let count = reports.len();
    let worktrees = if count == 1 { "worktree" } else { "worktrees" };
    text += &format!(
        "\n{count} {worktrees}, {total}; {removable} removable, {freed} (run `rho worktrees prune`)\n"
    );
    text
}

/// A sum of sizes, some of which the daemon may not have measured, as a
/// daemon older than `rho worktrees status` does not.
#[derive(Default)]
struct Bytes {
    measured: u64,
    unmeasured: bool,
}

impl Bytes {
    fn add(&mut self, bytes: Option<u64>) {
        match bytes {
            Some(bytes) => self.measured += bytes,
            None => self.unmeasured = true,
        }
    }
}

impl std::fmt::Display for Bytes {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let bytes = human_bytes(self.measured);
        match self.unmeasured {
            true => write!(formatter, "at least {bytes}"),
            false => formatter.write_str(&bytes),
        }
    }
}

/// `bytes` in binary units, one decimal from KiB up.
fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KiB", "MiB", "GiB", "TiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut size = bytes as f64 / 1024.0;
    let mut unit = 0;
    while size >= 1024.0 && unit + 1 < UNITS.len() {
        size /= 1024.0;
        unit += 1;
    }
    format!("{size:.1} {}", UNITS[unit])
}
