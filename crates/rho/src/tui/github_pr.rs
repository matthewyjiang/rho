//! Current-branch GitHub pull request for the statusline.
//!
//! `gh pr view` runs off the UI thread. Missing `gh`, a timed-out probe, and
//! matrix fixtures stay silent. A non-zero `gh` that reports no pull request
//! is confirmed absence and clears a painted chip; other failures leave it.
//!
//! After startup, a probe runs when HEAD moves, when a finished shell command
//! looks like `gh pr` or `git push`, and on a 90s timer while the session has
//! had input in the last hour. The timer also re-reads HEAD so a checkout in
//! another pane still updates. Terminal focus is not a trigger.

use std::{
    path::Path,
    process::Stdio,
    time::{Duration, Instant},
};

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

/// Background `gh pr view` while the session is active.
///
/// Claude Code documents ~90s while you are in the session. One probe is ~1s
/// (see [`GH_PR_VIEW_BUDGET`]), so 90s is ~80× that cost, not a latency guess.
const GH_PR_POLL_INTERVAL: Duration = Duration::from_secs(90);

/// Stop interval probes after this long without a key or paste. The next input
/// starts them again. Same idle cutoff Claude Code documents.
const GH_PR_POLL_IDLE_STOP: Duration = Duration::from_secs(3600);

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

fn next_poll_in(
    now: Instant,
    last_started: Option<Instant>,
    last_input: Instant,
    pending: bool,
    has_branch: bool,
    matrix: bool,
) -> Option<Duration> {
    if matrix || pending || !has_branch {
        return None;
    }
    if now.saturating_duration_since(last_input) >= GH_PR_POLL_IDLE_STOP {
        return None;
    }
    let Some(started) = last_started else {
        return Some(Duration::ZERO);
    };
    Some(GH_PR_POLL_INTERVAL.saturating_sub(now.saturating_duration_since(started)))
}

/// `gh` global flags that take a value before the subcommand.
const GH_FLAGS_WITH_VALUE: &[&str] = &["-R", "--repo", "--hostname", "--dir"];
/// `git` global flags that take a value before the subcommand.
const GIT_FLAGS_WITH_VALUE: &[&str] = &["-C", "-c", "--git-dir", "--work-tree"];

/// True when a finished shell command likely created, closed, or retargeted
/// the current-branch PR.
///
/// One split over `command`, no allocation. Finds `gh pr` or `git push` after
/// a path/`.exe` basename and known global flags. Not limited to argv0, so
/// `sudo gh pr create` and `cd src && git push` match. A hit only starts a
/// background `gh pr view`.
fn command_may_change_pr(command: &str) -> bool {
    let mut tokens = command
        .split(|c: char| c.is_whitespace() || matches!(c, ';' | '|' | '&' | '(' | ')' | '`'))
        .filter(|token| !token.is_empty());
    while let Some(token) = tokens.next() {
        let name = tool_name(token);
        if name.eq_ignore_ascii_case("gh") {
            if next_positional(&mut tokens, GH_FLAGS_WITH_VALUE)
                .is_some_and(|subcommand| subcommand.eq_ignore_ascii_case("pr"))
            {
                return true;
            }
        } else if name.eq_ignore_ascii_case("git")
            && next_positional(&mut tokens, GIT_FLAGS_WITH_VALUE)
                .is_some_and(|subcommand| subcommand.eq_ignore_ascii_case("push"))
        {
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

fn next_positional<'a>(
    tokens: &mut impl Iterator<Item = &'a str>,
    flags_with_value: &[&str],
) -> Option<&'a str> {
    loop {
        let token = tokens.next()?;
        if token == "--" {
            return tokens.next();
        }
        if !token.starts_with('-') {
            return Some(token);
        }
        let flag = token.split_once('=').map_or(token, |(name, _)| name);
        if !token.contains('=')
            && flags_with_value
                .iter()
                .any(|known| known.eq_ignore_ascii_case(flag))
        {
            tokens.next()?;
        }
    }
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
        self.github_pr_last_started = Some(Instant::now());
        self.pending_github_pr = Some(tokio::spawn(async move { lookup(&cwd).await }));
    }

    fn restart_github_pr_fetch(&mut self) {
        if let Some(handle) = self.pending_github_pr.take() {
            handle.abort();
        }
        self.start_github_pr_fetch();
    }

    pub(super) fn note_github_pr_input(&mut self) {
        self.github_pr_last_input = Instant::now();
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

    pub(super) fn maybe_refresh_github_pr_on_interval(&mut self) {
        if next_poll_in(
            Instant::now(),
            self.github_pr_last_started,
            self.github_pr_last_input,
            self.pending_github_pr.is_some(),
            self.statusline.branch().is_some(),
            smoke_injection::matrix_enabled(),
        )
        .is_some_and(|wait| wait.is_zero())
        {
            if self.statusline.refresh_git_branch() {
                self.restart_github_pr_fetch();
            } else {
                self.start_github_pr_fetch();
            }
        }
    }

    pub(super) fn github_pr_next_poll_in(&self) -> Option<Duration> {
        next_poll_in(
            Instant::now(),
            self.github_pr_last_started,
            self.github_pr_last_input,
            self.pending_github_pr.is_some(),
            self.statusline.branch().is_some(),
            smoke_injection::matrix_enabled(),
        )
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
            match paint_for_current_branch(self.statusline.branch(), lookup) {
                GithubPrPaint::Keep => {}
                GithubPrPaint::Clear => self.statusline.update_cwd_extra(None),
                GithubPrPaint::Show(pr) => self.statusline.update_cwd_extra(Some(cwd_extra(&pr))),
            }
        }
    }
}

#[cfg(test)]
#[path = "github_pr_tests.rs"]
mod tests;
