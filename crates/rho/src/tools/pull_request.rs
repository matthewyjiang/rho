//! The pull request tools: link GitHub pull requests to the session and
//! watch them, so the daemon wakes the session when one changes.
//!
//! Each call goes to the session's [`PullRequestInbox`]; the daemon's PR
//! service reads the pull requests and wakes the session. Only a daemon
//! session gets an inbox, so only it gets these tools.

use std::{sync::Arc, time::Duration};

use rho_sdk::{
    model::ToolSpec,
    tool::{
        Tool, ToolContext, ToolError, ToolErrorKind, ToolFuture, ToolInvocation, ToolOutput,
        ToolSecurity,
    },
};
use serde_json::{json, Value};

use super::{code_mode::ContextExposure, sdk_registry::ToolBundle};
use crate::app::pull_requests::{
    CheckCounts, Gate, LastRead, LinkOrigin, Linked, ListedPullRequest, Mergeable, PrState,
    PullRequestError, PullRequestInbox, Rollup, Unlinked, Unwatched, Watched,
};

/// The tool whose presence adds the pull request block to the system prompt.
pub(crate) const WATCH_TOOL_NAME: &str = "watch_pull_request";

const PULL_REQUEST_PARAM: &str =
    "A URL, `owner/name#12`, or `#12` in this workspace's `origin` repository.";

/// What one of the five tools does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Watch,
    Unwatch,
    Link,
    Unlink,
    List,
}

impl Action {
    fn name(self) -> &'static str {
        match self {
            Self::Watch => WATCH_TOOL_NAME,
            Self::Unwatch => "unwatch_pull_request",
            Self::Link => "link_pull_request",
            Self::Unlink => "unlink_pull_request",
            Self::List => "list_session_pull_requests",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::Watch => {
                "Watch an open GitHub pull request for this session, linking it first. Returns \
                 its state now: head, checks, conflicts and comment count. That is the baseline, \
                 not news, so handle it before you end your turn. After that rho wakes this \
                 session when checks fail, when the required checks pass (all checks when none \
                 is required), on new comments or reviews by other people, on a merge conflict, \
                 and once when it merges or closes. Then end your turn; do not poll or sleep. The \
                 watch stays on after each wake, so do not call this again on a wake. A wake is \
                 not a merge decision. One pull request per call."
            }
            Self::Unwatch => {
                "Stop watching a pull request for this session. It stays linked. Call it before \
                 you hand the work back to the user."
            }
            Self::Link => {
                "Link a pull request to this session without watching it, so clients show its \
                 state beside the session. To be woken on changes, call `watch_pull_request` \
                 instead."
            }
            Self::Unlink => {
                "Remove a pull request's link from this session, and its watch if it has one. \
                 Succeeds when it was not linked."
            }
            Self::List => {
                "List this session's linked pull requests: URL, who linked it, whether rho \
                 watches it, its last known state and checks, how old that read is, and any \
                 trouble reading it. This is rho's last read, not a readiness check; ask `gh` \
                 for the current state."
            }
        }
    }

    fn input_schema(self) -> Value {
        match self {
            Self::Watch | Self::Unwatch | Self::Link | Self::Unlink => json!({
                "type": "object",
                "properties": {
                    "pull_request": {
                        "type": "string",
                        "description": PULL_REQUEST_PARAM,
                    },
                },
                "required": ["pull_request"],
                "additionalProperties": false,
            }),
            Self::List => json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false,
            }),
        }
    }
}

struct PullRequestTool {
    inbox: PullRequestInbox,
    action: Action,
}

impl Tool for PullRequestTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.action.name().into(),
            description: self.action.description().into(),
            input_schema: self.action.input_schema(),
        }
    }

    fn security(&self) -> ToolSecurity {
        ToolSecurity::built_in([])
    }

    fn call<'a>(&'a self, invocation: ToolInvocation, _context: ToolContext) -> ToolFuture<'a> {
        Box::pin(async move {
            let now = crate::app::pull_requests::unix_ms();
            let arguments = invocation.arguments();
            let text = match self.action {
                Action::Watch => watched_text(
                    &self
                        .inbox
                        .watch(target(arguments)?, now)
                        .await
                        .map_err(refused)?,
                ),
                Action::Unwatch => unwatched_text(
                    &self
                        .inbox
                        .unwatch(target(arguments)?)
                        .await
                        .map_err(refused)?,
                ),
                Action::Link => linked_text(
                    &self
                        .inbox
                        .link(target(arguments)?, LinkOrigin::Agent, now)
                        .await
                        .map_err(refused)?,
                ),
                Action::Unlink => unlinked_text(
                    &self
                        .inbox
                        .unlink(target(arguments)?)
                        .await
                        .map_err(refused)?,
                ),
                Action::List => listed_text(&self.inbox.list(now)),
            };
            Ok(ToolOutput::text(text))
        })
    }
}

/// Some of the pull request tools over one session's inbox. Exposure is per
/// bundle, so the tools come in two: [`Self::watch`], declared directly,
/// and [`Self::link`], which `tool_search` loads.
pub(super) struct PullRequestBundle {
    tools: Vec<Arc<dyn Tool>>,
    exposure: ContextExposure,
}

impl PullRequestBundle {
    /// `watch_pull_request` and `unwatch_pull_request`, which an agent
    /// needs as soon as it opens or pushes to a pull request.
    pub(super) fn watch(inbox: &PullRequestInbox) -> Self {
        Self::of(
            inbox,
            &[Action::Watch, Action::Unwatch],
            ContextExposure::Direct,
        )
    }

    /// `link_pull_request`, `unlink_pull_request` and
    /// `list_session_pull_requests`, which the system prompt names.
    pub(super) fn link(inbox: &PullRequestInbox) -> Self {
        Self::of(
            inbox,
            &[Action::Link, Action::Unlink, Action::List],
            ContextExposure::Hidden,
        )
    }

    fn of(inbox: &PullRequestInbox, actions: &[Action], exposure: ContextExposure) -> Self {
        let tools = actions
            .iter()
            .map(|&action| {
                Arc::new(PullRequestTool {
                    inbox: inbox.clone(),
                    action,
                }) as Arc<dyn Tool>
            })
            .collect();
        Self { tools, exposure }
    }
}

impl ToolBundle for PullRequestBundle {
    fn tools(&self) -> &[Arc<dyn Tool>] {
        &self.tools
    }

    fn context_exposure(&self) -> ContextExposure {
        self.exposure
    }
}

/// The `pull_request` argument, trimmed.
fn target(arguments: &Value) -> Result<&str, ToolError> {
    arguments
        .get("pull_request")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|target| !target.is_empty())
        .ok_or_else(|| {
            ToolError::new(
                ToolErrorKind::InvalidArguments,
                "pull_request must be a non-empty string: a URL, owner/name#12, or #12",
            )
        })
}

/// The tool error for a call the inbox refused. Its text names the cause,
/// and a limit's text names the limit and the asked value.
fn refused(error: PullRequestError) -> ToolError {
    let kind = match &error {
        PullRequestError::InvalidTarget(_) => ToolErrorKind::InvalidArguments,
        PullRequestError::AlreadyEnded { .. }
        | PullRequestError::NotInstalled
        | PullRequestError::SignedOut { .. }
        | PullRequestError::Budget { .. }
        | PullRequestError::TooManyWatches { .. }
        | PullRequestError::NotFound(_)
        | PullRequestError::Read(_)
        | PullRequestError::Store(_) => ToolErrorKind::Execution,
    };
    ToolError::new(kind, error.to_string())
}

fn watched_text(watched: &Watched) -> String {
    let Watched {
        pr,
        url,
        state,
        head,
        checks,
        gate,
        gate_checks,
        mergeable,
        comments,
        unresolved_threads,
        started,
    } = watched;
    let label = pr.to_string();
    let (opening, read) = if *started {
        (
            format!("Watching {label} ({url}), linked to this session."),
            "This is the baseline, not news",
        )
    } else {
        (
            format!(
                "{label} ({url}) was already watched for this session; the watch goes on as \
                 it was."
            ),
            "This is rho's last read, not news",
        )
    };
    let state = PrState::word(*state);
    let head = head.get(..7).unwrap_or(head);
    let checks = checks_text(checks, *gate, *gate_checks);
    let conflicts = match mergeable {
        Mergeable::Yes => "no conflicts",
        Mergeable::Conflicting => "conflicts with its base branch",
        Mergeable::Unknown => "conflicts not worked out yet",
    };
    let comments = match *unresolved_threads {
        0 => counted(*comments, "comment", "comments"),
        threads => format!(
            "{} ({})",
            counted(*comments, "comment", "comments"),
            counted(
                threads,
                "unresolved review thread",
                "unresolved review threads"
            )
        ),
    };
    format!(
        "{opening}\n\
         Now: {state}, head {head}, {checks}, {conflicts}, {comments}.\n\
         {read}: deal with it before you end your turn. rho wakes this session when checks \
         fail, when the checks rho waits on pass, on new comments or reviews by others, on a conflict, and \
         once when it merges or closes. End your turn; do not poll."
    )
}

/// `14 checks (12 passed, 2 running, 0 failed; none are required, so rho
/// waits on all 14)`,
/// or `50 of 60 checks read (...)` when the read left some out.
fn checks_text(checks: &CheckCounts, gate: Gate, gate_checks: u32) -> String {
    let CheckCounts {
        total,
        passed,
        running,
        failed,
    } = *checks;
    if total == 0 {
        return "no checks yet".to_owned();
    }
    // A read lists the first checks only. With some left out, the watch
    // can't know they passed: it tells failures among those read, never
    // the gate passing.
    let read = passed.saturating_add(running).saturating_add(failed);
    let (total_text, unread) = if read < total {
        (
            format!("{read} of {total} checks read"),
            "; rho can't read the rest, so it won't tell you when they all pass",
        )
    } else {
        (counted(total, "check", "checks"), "")
    };
    let gate = match gate {
        Gate::Required => format!("{gate_checks} required"),
        Gate::All => format!("none are required, so rho waits on all {gate_checks}"),
    };
    format!("{total_text} ({passed} passed, {running} running, {failed} failed; {gate}{unread})")
}

fn unwatched_text(unwatched: &Unwatched) -> String {
    let label = unwatched.pr.to_string();
    if unwatched.was_watched {
        format!("Stopped watching {label}. It stays linked.")
    } else {
        format!("This session was not watching {label}; nothing changed.")
    }
}

fn linked_text(linked: &Linked) -> String {
    let Linked { pr, url, new } = linked;
    let label = pr.to_string();
    if *new {
        format!("Linked {label} ({url}) to this session.")
    } else {
        format!("{label} ({url}) was already linked to this session.")
    }
}

fn unlinked_text(unlinked: &Unlinked) -> String {
    let label = unlinked.pr.to_string();
    if unlinked.was_linked {
        format!("Unlinked {label} from this session, and ended its watch if it had one.")
    } else {
        format!("{label} was not linked to this session; nothing changed.")
    }
}

fn listed_text(listed: &[ListedPullRequest]) -> String {
    if listed.is_empty() {
        return "No pull requests are linked to this session.".to_owned();
    }
    let mut text = String::new();
    for pull_request in listed {
        let ListedPullRequest {
            pr,
            url,
            origin,
            watched,
            last_read,
            trouble,
        } = pull_request;
        let origin = match origin {
            LinkOrigin::User => "linked by the user",
            LinkOrigin::Agent => "linked by the agent",
        };
        let watched = if *watched { "watched" } else { "not watched" };
        let read = match last_read {
            Some(LastRead { state, rollup, age }) => {
                let checks = match rollup {
                    Rollup::None => "no checks",
                    Rollup::Pending => "checks running",
                    Rollup::Passing => "checks passing",
                    Rollup::Failing => "checks failing",
                };
                format!(
                    "{}, {checks}, read {} ago",
                    PrState::word(*state),
                    ago(*age)
                )
            }
            None => "not read yet".to_owned(),
        };
        let trouble = trouble.as_ref().map_or_else(String::new, |trouble| {
            format!("; last read failed: {trouble}")
        });
        text.push_str(&format!(
            "- {pr} ({url}): {origin}, {watched}, {read}{trouble}\n"
        ));
    }
    text.push_str("This is rho's last read, not a readiness check; ask gh for the current state.");
    text
}

/// `1 comment`, `3 comments`.
fn counted(count: u32, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

/// `45 s`, `12 min`, `2 h 5 min`.
fn ago(age: Duration) -> String {
    let seconds = age.as_secs();
    if seconds < 60 {
        format!("{seconds} s")
    } else if seconds < 3600 {
        format!("{} min", seconds / 60)
    } else {
        format!("{} h {} min", seconds / 3600, seconds % 3600 / 60)
    }
}

#[cfg(test)]
#[path = "pull_request_tests.rs"]
mod tests;
