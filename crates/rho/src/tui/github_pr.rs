//! Current-branch GitHub pull request for the statusline.
//!
//! `gh pr view` runs off the UI thread. Missing `gh`, a failed or timed-out
//! probe, and matrix fixtures stay silent. `gh pr view` exits non-zero when
//! there is no PR, so that cannot be distinguished from a crash; a failed
//! refresh does not clear a chip that already painted.

use std::{path::Path, process::Stdio, time::Duration};

use futures_util::FutureExt;
use serde::Deserialize;

use super::{
    smoke_injection,
    statusline::{CwdExtra, CwdExtraTone},
    workspace, App,
};

const GH_PR_FIELDS: &str = "number,reviewDecision,mergeStateStatus,statusCheckRollup";

/// Budget for `gh pr view` so a hung CLI cannot pin the TUI poll loop.
///
/// Measured five serial in-tree calls: 701–1123ms (median 1036ms). 8s is a
/// tripwire (~7× max observed), not a latency target.
const GH_PR_VIEW_BUDGET: Duration = Duration::from_secs(8);

#[derive(Clone, Debug, PartialEq, Eq)]
struct GithubPr {
    number: u64,
    tone: Option<GithubPrTone>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GithubPrTone {
    Ready,
    Issues,
}

#[derive(Debug)]
enum GithubPrProbe {
    /// `gh` missing, timed out, non-zero (including no PR), or unreadable JSON.
    Unavailable,
    Found(GithubPr),
}

#[derive(Debug)]
pub(super) struct GithubPrLookup {
    branch: Option<String>,
    probe: GithubPrProbe,
}

#[derive(Debug, Deserialize)]
struct GhPrView {
    number: u64,
    #[serde(default, rename = "reviewDecision")]
    review_decision: Option<String>,
    #[serde(default, rename = "mergeStateStatus")]
    merge_state_status: String,
    #[serde(
        default,
        rename = "statusCheckRollup",
        deserialize_with = "deserialize_check_rollup"
    )]
    status_check_rollup: Vec<GhCheck>,
}

#[derive(Debug, Deserialize, Default)]
struct GhCheck {
    #[serde(default)]
    conclusion: String,
    #[serde(default)]
    state: String,
}

fn deserialize_check_rollup<'de, D>(deserializer: D) -> Result<Vec<GhCheck>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Rollup {
        List(Vec<GhCheck>),
        Other(serde::de::IgnoredAny),
    }
    Ok(match Option::<Rollup>::deserialize(deserializer)? {
        Some(Rollup::List(checks)) => checks,
        Some(Rollup::Other(_)) | None => Vec::new(),
    })
}

async fn lookup(cwd: &Path) -> GithubPrLookup {
    GithubPrLookup {
        branch: workspace::git_branch(cwd),
        probe: probe(cwd).await,
    }
}

async fn probe(cwd: &Path) -> GithubPrProbe {
    let Some(gh) = crate::executable::find_on_path("gh") else {
        return GithubPrProbe::Unavailable;
    };
    let mut command = tokio::process::Command::new(gh);
    command
        .args(["pr", "view", "--json", GH_PR_FIELDS])
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let child = match command.spawn() {
        Ok(child) => child,
        Err(_) => return GithubPrProbe::Unavailable,
    };
    match tokio::time::timeout(GH_PR_VIEW_BUDGET, child.wait_with_output()).await {
        Ok(Ok(output)) if output.status.success() => parse_gh_pr_view(&output.stdout)
            .map_or(GithubPrProbe::Unavailable, GithubPrProbe::Found),
        _ => GithubPrProbe::Unavailable,
    }
}

fn parse_gh_pr_view(bytes: &[u8]) -> Option<GithubPr> {
    let view: GhPrView = serde_json::from_slice(bytes).ok()?;
    Some(GithubPr {
        number: view.number,
        tone: tone_from_view(&view),
    })
}

fn tone_from_view(view: &GhPrView) -> Option<GithubPrTone> {
    let review = view
        .review_decision
        .as_deref()
        .unwrap_or("")
        .to_ascii_uppercase();
    let merge = view.merge_state_status.to_ascii_uppercase();
    let issues = review == "CHANGES_REQUESTED"
        || merge == "DIRTY"
        || view.status_check_rollup.iter().any(check_has_issues);
    if issues {
        Some(GithubPrTone::Issues)
    } else if merge == "CLEAN" {
        Some(GithubPrTone::Ready)
    } else {
        None
    }
}

fn check_has_issues(check: &GhCheck) -> bool {
    let conclusion = check.conclusion.to_ascii_uppercase();
    let state = check.state.to_ascii_uppercase();
    matches!(
        conclusion.as_str(),
        "FAILURE" | "TIMED_OUT" | "ACTION_REQUIRED" | "ERROR"
    ) || matches!(state.as_str(), "FAILURE" | "ERROR")
}

/// Paint only a probe that still matches the statusline branch and found a PR.
/// Stale and unavailable results leave a painted chip alone.
fn pr_for_current_branch(current: Option<&str>, lookup: GithubPrLookup) -> Option<GithubPr> {
    if current != lookup.branch.as_deref() {
        return None;
    }
    match lookup.probe {
        GithubPrProbe::Found(pr) => Some(pr),
        GithubPrProbe::Unavailable => None,
    }
}

fn cwd_extra(pr: &GithubPr) -> CwdExtra {
    CwdExtra::new(
        format!(" #{}", pr.number),
        match pr.tone {
            Some(GithubPrTone::Ready) => CwdExtraTone::Success,
            Some(GithubPrTone::Issues) => CwdExtraTone::Error,
            None => CwdExtraTone::Dim,
        },
    )
}

impl App {
    pub(super) fn start_github_pr_fetch(&mut self) {
        if smoke_injection::matrix_enabled()
            || self.pending_github_pr.is_some()
            || self.statusline.branch().is_none()
        {
            return;
        }
        let cwd = self.info.runtime.cwd.clone();
        self.pending_github_pr = Some(tokio::spawn(async move { lookup(&cwd).await }));
    }

    fn restart_github_pr_fetch(&mut self) {
        if let Some(handle) = self.pending_github_pr.take() {
            handle.abort();
        }
        self.start_github_pr_fetch();
    }

    /// Focus: HEAD may have moved in the background, and PR checks/review
    /// may have changed too.
    pub(super) fn refresh_workspace_on_focus(&mut self) {
        if self.statusline.refresh_git_branch() {
            self.restart_github_pr_fetch();
        } else {
            self.start_github_pr_fetch();
        }
    }

    /// After a command that can move HEAD. Do not spawn `gh` unless the
    /// branch actually changed; every tool finish is too expensive.
    pub(super) fn refresh_git_after_command(&mut self) {
        if self.statusline.refresh_git_branch() {
            self.restart_github_pr_fetch();
        }
    }

    pub(super) fn poll_github_pr(&mut self) {
        let Some(handle) = self.pending_github_pr.as_mut() else {
            return;
        };
        let Some(result) = handle.now_or_never() else {
            return;
        };
        self.pending_github_pr = None;
        if let Ok(lookup) = result {
            if let Some(pr) = pr_for_current_branch(self.statusline.branch(), lookup) {
                self.statusline.update_cwd_extra(Some(cwd_extra(&pr)));
            }
        }
    }
}

#[cfg(test)]
#[path = "github_pr_tests.rs"]
mod tests;
