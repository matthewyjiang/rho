//! Current-branch GitHub pull request for the statusline.
//!
//! `gh pr view` runs off the UI thread. Missing `gh`, a timed-out probe, and
//! matrix fixtures stay silent. A non-zero `gh` that reports no pull request
//! is confirmed absence and clears a painted chip; other failures leave it.
//!
//! After startup, a probe runs when HEAD moves, when a finished shell command
//! looks like `gh pr` or `git push`, and when the terminal regains focus.

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

#[derive(Debug, PartialEq, Eq)]
enum GithubPrProbe {
    /// `gh` missing, timed out, crashed, or unreadable JSON.
    Unavailable,
    /// `gh` reported that this branch has no pull request.
    Absent,
    Found(GithubPr),
}

#[derive(Debug, PartialEq, Eq)]
enum GithubPrPaint {
    Keep,
    Clear,
    Show(GithubPr),
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
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let child = match command.spawn() {
        Ok(child) => child,
        Err(_) => return GithubPrProbe::Unavailable,
    };
    match tokio::time::timeout(GH_PR_VIEW_BUDGET, child.wait_with_output()).await {
        Ok(Ok(output)) => {
            classify_gh_pr_view(output.status.success(), &output.stdout, &output.stderr)
        }
        _ => GithubPrProbe::Unavailable,
    }
}

fn classify_gh_pr_view(success: bool, stdout: &[u8], stderr: &[u8]) -> GithubPrProbe {
    if success {
        parse_gh_pr_view(stdout).map_or(GithubPrProbe::Unavailable, GithubPrProbe::Found)
    } else if confirmed_no_pr(stderr) {
        GithubPrProbe::Absent
    } else {
        GithubPrProbe::Unavailable
    }
}

fn confirmed_no_pr(stderr: &[u8]) -> bool {
    let text = String::from_utf8_lossy(stderr).to_ascii_lowercase();
    text.contains("no pull requests found")
        || text.contains("no open pull requests found")
        || text.contains("no closed pull requests found")
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

/// Paint only a probe that still matches the statusline branch.
/// Stale and unavailable results leave a painted chip alone; confirmed
/// absence clears it.
fn paint_for_current_branch(current: Option<&str>, lookup: GithubPrLookup) -> GithubPrPaint {
    if current != lookup.branch.as_deref() {
        return GithubPrPaint::Keep;
    }
    match lookup.probe {
        GithubPrProbe::Found(pr) => GithubPrPaint::Show(pr),
        GithubPrProbe::Absent => GithubPrPaint::Clear,
        GithubPrProbe::Unavailable => GithubPrPaint::Keep,
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

/// True when a finished shell command likely created, closed, or retargeted
/// the current-branch PR.
///
/// One split over `command`, no allocation. Finds `gh` later followed by `pr`,
/// or `git` later followed by `push`, after a path/`.exe` basename. Not limited
/// to argv0, so `sudo gh pr create` and `cd src && git push` match. A hit only
/// starts a background `gh pr view`.
fn command_may_change_pr(command: &str) -> bool {
    let mut saw_gh = false;
    let mut saw_git = false;
    for token in command
        .split(|c: char| c.is_whitespace() || matches!(c, ';' | '|' | '&' | '(' | ')' | '`'))
        .filter(|token| !token.is_empty())
    {
        let name = tool_name(token);
        if name.eq_ignore_ascii_case("gh") {
            saw_gh = true;
        } else if saw_gh && name.eq_ignore_ascii_case("pr") {
            return true;
        } else if name.eq_ignore_ascii_case("git") {
            saw_git = true;
        } else if saw_git && name.eq_ignore_ascii_case("push") {
            return true;
        }
    }
    false
}

fn tool_name(token: &str) -> &str {
    let name = token.rsplit(['/', '\\']).next().unwrap_or(token);
    name.strip_suffix(".exe")
        .or_else(|| name.strip_suffix(".EXE"))
        .unwrap_or(name)
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

    /// After a finished shell or tool command. Refetch when HEAD moved, or
    /// when the command itself can create or retarget the current-branch PR.
    pub(super) fn refresh_git_after_command(&mut self, command: Option<&str>) {
        if self.statusline.refresh_git_branch() {
            self.restart_github_pr_fetch();
            return;
        }
        if command.is_some_and(command_may_change_pr) {
            self.restart_github_pr_fetch();
        }
    }

    pub(super) fn poll_github_pr(&mut self) -> bool {
        let Some(handle) = self.pending_github_pr.as_mut() else {
            return false;
        };
        let Some(result) = handle.now_or_never() else {
            return false;
        };
        self.pending_github_pr = None;
        if let Ok(lookup) = result {
            match paint_for_current_branch(self.statusline.branch(), lookup) {
                GithubPrPaint::Keep => {}
                GithubPrPaint::Clear => self.statusline.update_cwd_extra(None),
                GithubPrPaint::Show(pr) => self.statusline.update_cwd_extra(Some(cwd_extra(&pr))),
            }
        }
        true
    }
}

#[cfg(test)]
#[path = "github_pr_tests.rs"]
mod tests;
