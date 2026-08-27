//! Current-branch GitHub pull request for the statusline.
//!
//! Probe order: git repo, GitHub-based remote, then `gh pr view`. Lookup runs
//! off the UI thread; missing `gh`, non-GitHub remotes, and matrix fixtures
//! stay silent.

use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use futures_util::FutureExt;
use serde::Deserialize;

use super::{smoke_injection, workspace, App};

const GH_PR_FIELDS: &str = "number,reviewDecision,mergeStateStatus,statusCheckRollup";

/// Current-branch pull request shown next to the cwd path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct GithubPr {
    pub number: u64,
    pub tone: Option<GithubPrTone>,
}

/// Green = ready to merge; red = conflicts, failing checks, or requested changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum GithubPrTone {
    Ready,
    Issues,
}

#[derive(Debug)]
pub(super) struct GithubPrLookup {
    pub branch: Option<String>,
    pub pr: Option<GithubPr>,
}

#[derive(Debug, Deserialize)]
struct GhPrView {
    number: u64,
    #[serde(default, rename = "reviewDecision")]
    review_decision: Option<String>,
    #[serde(default, rename = "mergeStateStatus")]
    merge_state_status: String,
    #[serde(default, rename = "statusCheckRollup")]
    status_check_rollup: Option<Vec<GhCheck>>,
}

#[derive(Debug, Deserialize, Default)]
struct GhCheck {
    #[serde(default)]
    conclusion: String,
    #[serde(default)]
    state: String,
}

pub(super) fn lookup(cwd: &Path) -> GithubPrLookup {
    GithubPrLookup {
        branch: workspace::git_branch(cwd),
        pr: probe(cwd),
    }
}

fn probe(cwd: &Path) -> Option<GithubPr> {
    if !should_probe(cwd) {
        return None;
    }
    let output = Command::new("gh")
        .args(["pr", "view", "--json", GH_PR_FIELDS])
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_gh_pr_view(&output.stdout)
}

fn should_probe(cwd: &Path) -> bool {
    let remotes = workspace::git_remote_urls(cwd);
    !remotes.is_empty()
        && remotes.iter().any(|url| remote_is_github(url))
        && crate::executable::find_on_path("gh").is_some()
}

fn remote_is_github(url: &str) -> bool {
    remote_host(url).is_some_and(host_is_github)
}

fn remote_host(url: &str) -> Option<&str> {
    let url = url.trim();
    if url.is_empty() {
        return None;
    }
    if let Some((_, rest)) = url.split_once("://") {
        let hostport_path = rest.rsplit_once('@').map_or(rest, |(_, host)| host);
        let hostport = hostport_path.split(['/', '?']).next()?;
        return host_from_hostport(hostport);
    }
    let hostpath = url.rsplit_once('@').map_or(url, |(_, host)| host);
    let host = hostpath.split(':').next()?.trim();
    if host.is_empty() || host.contains('/') {
        None
    } else {
        Some(host)
    }
}

fn host_from_hostport(hostport: &str) -> Option<&str> {
    if let Some(rest) = hostport.strip_prefix('[') {
        return rest.split(']').next().filter(|host| !host.is_empty());
    }
    hostport.split(':').next().filter(|host| !host.is_empty())
}

fn host_is_github(host: &str) -> bool {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    host == "github.com"
        || host.ends_with(".ghe.com")
        || host.split('.').any(|label| label == "github")
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
        || view
            .status_check_rollup
            .iter()
            .flatten()
            .any(check_has_issues);
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

impl App {
    pub(super) fn start_github_pr_fetch(&mut self) {
        if smoke_injection::matrix_enabled() || self.pending_github_pr.is_some() {
            return;
        }
        let cwd: PathBuf = self.info.runtime.cwd.clone();
        self.pending_github_pr = Some(tokio::task::spawn_blocking(move || lookup(&cwd)));
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
            self.statusline.apply_github_pr_lookup(lookup);
        }
    }

    pub(super) fn refresh_workspace_git(&mut self) {
        if self.statusline.refresh_git_branch() {
            if let Some(handle) = self.pending_github_pr.take() {
                handle.abort();
            }
        }
        self.start_github_pr_fetch();
    }
}

#[cfg(test)]
#[path = "github_pr_tests.rs"]
mod tests;
