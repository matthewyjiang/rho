//! Workflow-owned projection of the bounded, model-facing line summary.
//!
//! Live completion and history replay consume the same text protocol from
//! `tools/workflow_output.rs`; the generic card renderer knows no workflow policy.

use std::path::Path;

use rho_tools::{
    tool::compact_display_path,
    tool_card::{ToolBody, ToolCard, ToolFact, ToolFamily, ToolHeader, ToolStatus},
};
use serde_json::Value;

pub(crate) fn preview_card(arguments: &Value, cwd: &Path, status: ToolStatus) -> ToolCard {
    let action = argument(arguments, "action");
    let verb = action.map_or_else(|| "workflow".into(), |action| format!("workflow.{action}"));
    let primary = match action {
        Some("validate" | "plan") => {
            argument(arguments, "file").map(|file| compact_display_path(cwd, file))
        }
        _ => None,
    };
    let mut card = ToolCard::new(status, ToolFamily::Default, ToolHeader::call(verb, primary));
    let reference = match action {
        Some("run") => argument(arguments, "plan_id").map(|id| format!("plan: {id}")),
        Some("status" | "cancel" | "resume") => {
            argument(arguments, "run_id").map(|id| format!("run: {id}"))
        }
        _ => None,
    };
    if let Some(text) = reference {
        card.push_fact(ToolFact::Meta { text });
    }
    if action == Some("resume")
        && arguments.get("recover_uncertain").and_then(Value::as_bool) == Some(true)
    {
        card.push_fact(ToolFact::Meta {
            text: "recover uncertain attempts".into(),
        });
    }
    card
}

/// Facts form the scannable receipt. Full diagnostics, node attempts, artifact
/// pointers, exports and identity fields stay in the expandable body.
pub(crate) fn finished_card(arguments: &Value, content: &str, ok: bool, cwd: &Path) -> ToolCard {
    let mut card = preview_card(arguments, cwd, ToolStatus::from_finished(ok));
    let lines: Vec<_> = content.lines().collect();
    let Some(summary) = ok
        .then(|| parse_summary(&lines, argument(arguments, "action")))
        .flatten()
    else {
        let text = lines
            .iter()
            .map(|line| line.trim())
            .find(|line| !line.is_empty())
            .unwrap_or("workflow returned no summary");
        card.facts.insert(
            0,
            if ok {
                ToolFact::Meta {
                    text: if content.trim().is_empty() {
                        "workflow returned no summary"
                    } else {
                        "workflow result details available"
                    }
                    .into(),
                }
            } else {
                ToolFact::Error { text: text.into() }
            },
        );
        card.body = ToolBody::Lines(lines.into_iter().map(str::to_owned).collect());
        return card;
    };
    card.facts.clear();

    match summary.headline {
        Headline::Validation { valid } => {
            if !valid {
                card.status = ToolStatus::Error;
            }
            card.push_fact(if valid {
                ToolFact::Text {
                    text: "valid".into(),
                }
            } else {
                ToolFact::Error {
                    text: summary
                        .errors
                        .first()
                        .copied()
                        .unwrap_or("invalid workflow")
                        .into(),
                }
            });
            if summary.diagnostics > 0 {
                card.push_fact(ToolFact::Count {
                    label: "diagnostics".into(),
                    value: summary.diagnostics,
                    detail: None,
                });
            }
        }
        Headline::Plan { name } => {
            card.header = ToolHeader::status_first("workflow.plan", format!("{name} · planned"));
            if let Some(value) = summary.node_count {
                card.push_fact(ToolFact::Count {
                    label: "nodes".into(),
                    value,
                    detail: None,
                });
            }
            if let Some(id) = summary.plan_id {
                card.push_fact(ToolFact::Meta {
                    text: format!("plan: {id}"),
                });
            }
        }
        Headline::Run { id, state } => {
            let action = argument(arguments, "action").unwrap_or("status");
            let mut detail = state.replace('_', " ");
            if let Some(outcome) = summary.outcome {
                detail.push_str(&format!(" · {outcome}"));
            }
            card.header = ToolHeader::status_first(format!("workflow.{action}"), detail);
            if let Some(cancellation) = summary.cancellation {
                card.push_fact(ToolFact::Text {
                    text: format!("cancellation {}", cancellation.replace('_', " ")),
                });
            }
            if state == "needs_recovery" {
                card.push_fact(ToolFact::Error {
                    text: "run needs recovery before resuming".into(),
                });
            }
            if let Some(outcome @ ("failure" | "denial" | "blocked")) = summary.outcome {
                card.push_fact(ToolFact::Error {
                    text: format!("root outcome: {outcome}"),
                });
            }
            let problems: Vec<_> = summary
                .nodes
                .iter()
                .filter(|node| node.is_problem())
                .collect();
            if let Some(first) = problems.first() {
                card.push_fact(ToolFact::Error {
                    text: format!(
                        "{} nodes need attention · {} · {}",
                        problems.len(),
                        first.id,
                        first.state
                    ),
                });
            }
            if let Some(total) = summary.node_count {
                card.push_fact(ToolFact::Text {
                    text: summary.progress_text(total),
                });
            }
            card.push_fact(ToolFact::Meta {
                text: format!("run: {id}"),
            });
        }
    }
    if let Some(notice) = summary.omission {
        card.push_fact(ToolFact::Meta {
            text: notice.into(),
        });
    }
    // The first line is represented by the receipt. Everything else stays
    // intact, including unknown future detail fields and truncation notices.
    let mut details: Vec<String> = lines.iter().skip(1).map(|line| (*line).into()).collect();
    if let Some(file) = argument(arguments, "file") {
        details.push(format!("source: {file}"));
    }
    if let (Some("run"), Some(plan)) = (
        argument(arguments, "action"),
        argument(arguments, "plan_id"),
    ) {
        details.push(format!("plan_id: {plan}"));
    }
    card.body = ToolBody::Lines(details);
    card
}

fn argument<'a>(arguments: &'a Value, key: &str) -> Option<&'a str> {
    arguments
        .get(key)?
        .as_str()
        .filter(|value| !value.is_empty())
}

#[derive(Debug, PartialEq, Eq)]
enum Headline<'a> {
    Validation { valid: bool },
    Plan { name: &'a str },
    Run { id: &'a str, state: &'a str },
}

#[derive(Debug, PartialEq, Eq)]
struct Node<'a> {
    id: &'a str,
    state: &'a str,
}

impl Node<'_> {
    fn is_terminal(&self) -> bool {
        matches!(
            self.state,
            "success" | "failure" | "denial" | "cancellation" | "skipped" | "blocked"
        )
    }

    fn is_problem(&self) -> bool {
        matches!(self.state, "failure" | "denial" | "blocked")
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Summary<'a> {
    headline: Headline<'a>,
    plan_id: Option<&'a str>,
    node_count: Option<u64>,
    nodes: Vec<Node<'a>>,
    outcome: Option<&'a str>,
    cancellation: Option<&'a str>,
    diagnostics: u64,
    errors: Vec<&'a str>,
    omission: Option<&'a str>,
}

#[derive(Debug, PartialEq, Eq)]
struct NodeProgress {
    finished: usize,
    running: usize,
    waiting: usize,
    available: usize,
    total: u64,
    partial: bool,
}

impl Summary<'_> {
    fn progress(&self, total: u64) -> NodeProgress {
        let finished = self.nodes.iter().filter(|node| node.is_terminal()).count();
        let running = self
            .nodes
            .iter()
            .filter(|node| node.state == "running")
            .count();
        let waiting = self
            .nodes
            .iter()
            .filter(|node| matches!(node.state, "pending" | "ready"))
            .count();
        let partial =
            self.nodes.len() as u64 != total || finished + running + waiting != self.nodes.len();
        NodeProgress {
            finished,
            running,
            waiting,
            available: self.nodes.len(),
            total,
            partial,
        }
    }

    fn progress_text(&self, total: u64) -> String {
        let NodeProgress {
            finished,
            running,
            waiting,
            available,
            total,
            partial,
        } = self.progress(total);
        let mut text = if partial {
            format!("at least {finished}/{total} nodes finished")
        } else {
            format!("{finished}/{total} nodes finished")
        };
        if running > 0 {
            text.push_str(&format!(" · {running} running"));
        }
        if waiting > 0 {
            text.push_str(&format!(" · {waiting} waiting"));
        }
        if partial {
            text.push_str(&format!(
                " · partial progress ({available} node states available)"
            ));
        }
        text
    }
}

/// Match only protocol-depth fields: diagnostic continuation lines and artifact
/// details can contain the same labels without becoming run state or progress.
fn parse_summary<'a>(lines: &[&'a str], action: Option<&str>) -> Option<Summary<'a>> {
    let first = *lines.first()?;
    let headline = if let Some(validation) = first.strip_prefix("workflow validation: ") {
        Headline::Validation {
            valid: match validation {
                "valid" => true,
                "invalid" => false,
                _ => return None,
            },
        }
    } else {
        let (identity, state) = first.strip_prefix("workflow ")?.rsplit_once(": ")?;
        if identity.is_empty() || state.is_empty() {
            return None;
        }
        if state == "planned" && action == Some("plan") {
            Headline::Plan { name: identity }
        } else {
            Headline::Run {
                id: identity,
                state,
            }
        }
    };
    let mut summary = Summary {
        headline,
        plan_id: None,
        node_count: None,
        nodes: Vec::new(),
        outcome: None,
        cancellation: None,
        diagnostics: 0,
        errors: Vec::new(),
        omission: None,
    };
    let mut in_nodes = false;
    for line in lines.iter().skip(1).copied() {
        if let Some(value) = line.strip_prefix("nodes: ") {
            summary.node_count = value.parse().ok();
            in_nodes = true;
        } else if let Some(value) = line.strip_prefix("plan_id: ") {
            summary.plan_id = Some(value);
        } else if let Some(value) = line.strip_prefix("root outcome: ") {
            summary.outcome = Some(value);
        } else if let Some(value) = line.strip_prefix("cancellation: ") {
            summary.cancellation = Some(value);
        } else if line.starts_with("... ") {
            summary.omission = Some(line);
        } else if let Some(value) = line
            .strip_prefix("  ")
            .filter(|value| !value.starts_with(' '))
        {
            if in_nodes {
                if let Some((id, state)) = value.split_once(" · ") {
                    summary.nodes.push(Node {
                        id,
                        state: state.split(" · ").next().unwrap_or(state),
                    });
                }
            } else if value
                .split_once(" [")
                .is_some_and(|(severity, _)| matches!(severity, "error" | "warning" | "info"))
            {
                summary.diagnostics += 1;
                if value.starts_with("error [") {
                    summary.errors.push(value);
                }
            }
        }
    }
    Some(summary)
}

#[cfg(test)]
#[path = "workflow_tests.rs"]
mod tests;
