//! `/context` data: the next request's context window attributed by source.
//!
//! The SDK itemizes the request with its provider-neutral estimator. This
//! module owns Rho's grouping policy: system prompt sections from the prompt
//! source accounting, tool schemas split into built-in and per-MCP-server, and
//! history split by role, with tool results grouped by tool. Parts are scaled
//! so they share the calibrated total's scale when a provider measured the
//! prompt.
//!
//! [`session_context_report`] builds the typed report a daemon session answers
//! (`SessionReport::Context`); [`ContextReport`] is how rho's `/context`
//! overlay shows it.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use rho_protocol::{
    ContextMessageKind, ContextMessagePart, ContextPromptPart, ContextToolResultPart,
    ContextToolSchemaPart, PromptSourceKind as WirePromptSource, SessionContextReport,
    ToolSchemaOrigin,
};
use rho_sdk::{ContextBreakdown, ContextItem};
use serde::{Deserialize, Serialize};

use crate::prompt::{PromptSource, PromptSourceKind};

/// What the header total is based on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ContextBasis {
    /// The last provider-reported prompt plus local estimates for later additions.
    ProviderCalibrated,
    /// Local character-count estimate only.
    LocalEstimate,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ContextRow {
    pub(crate) label: String,
    /// Secondary text such as a source path or a schema count.
    pub(crate) detail: Option<String>,
    pub(crate) tokens: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ContextGroup {
    pub(crate) label: String,
    pub(crate) tokens: u64,
    pub(crate) rows: Vec<ContextRow>,
}

/// The `/context` overlay's view of a [`SessionContextReport`], in the shape
/// a daemon session's untyped report has always had.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ContextReport {
    /// Total for the next request, calibrated when the basis says so.
    pub(crate) tokens: u64,
    pub(crate) basis: ContextBasis,
    pub(crate) window: Option<u64>,
    /// Non-empty groups in display order. Group and row tokens are local
    /// estimates scaled to `tokens`; fixed request framing is not shown.
    pub(crate) groups: Vec<ContextGroup>,
}

/// Local token totals accumulated per bucket before scaling.
#[derive(Default)]
struct Buckets {
    system_prompt: u64,
    tools_builtin: (u64, usize),
    /// MCP server identity -> (tokens, schema count).
    tools_mcp: BTreeMap<String, (u64, usize)>,
    user: u64,
    compaction_summary: u64,
    assistant: u64,
    system_notices: u64,
    /// Tool name, `None` for results whose call is not in history. Ordered
    /// so that equal sizes list in a stable order.
    tool_results: BTreeMap<Option<String>, u64>,
    request_context: u64,
    /// Items from a newer SDK without their own row yet.
    other: u64,
}

/// Each MCP tool a session's model sees, by exported name, with the
/// identity of the server it comes from.
pub(crate) fn mcp_tool_servers(
    report: &crate::tools::mcp::McpSessionReport,
) -> HashMap<String, String> {
    report
        .servers
        .iter()
        .flat_map(|server| {
            server
                .tools()
                .iter()
                .map(|tool| (tool.exported_name.clone(), server.identity.clone()))
        })
        .collect()
}

/// The next request's context attributed by source. `mcp_tools` maps each
/// exported MCP tool name to its server identity; other schemas count as
/// built-in. `window` is the effective context limit.
pub(crate) fn session_context_report(
    breakdown: &ContextBreakdown,
    prompt_sources: &[PromptSource],
    mcp_tools: &HashMap<String, String>,
    window: Option<u64>,
) -> SessionContextReport {
    let mut buckets = Buckets::default();
    for part in breakdown.parts() {
        let tokens = part.tokens();
        match part.item() {
            ContextItem::RequestOverhead => {}
            ContextItem::ToolSchema { name } => {
                let entry = match mcp_tools.get(name) {
                    Some(server) => buckets.tools_mcp.entry(server.clone()).or_default(),
                    None => &mut buckets.tools_builtin,
                };
                entry.0 += tokens;
                entry.1 += 1;
            }
            ContextItem::SystemPrompt => buckets.system_prompt += tokens,
            ContextItem::System => buckets.system_notices += tokens,
            ContextItem::User => buckets.user += tokens,
            ContextItem::CompactionSummary => buckets.compaction_summary += tokens,
            ContextItem::Assistant => buckets.assistant += tokens,
            ContextItem::ToolResult { tool } => {
                *buckets.tool_results.entry(tool.clone()).or_default() += tokens;
            }
            ContextItem::RequestContext => buckets.request_context += tokens,
            // A new SDK item is still part of the request; count it rather
            // than silently dropping it or mislabeling it.
            _ => buckets.other += tokens,
        }
    }

    let estimate = breakdown.estimate();
    let scale = Scale {
        total: estimate.tokens(),
        local: estimate.estimated_tokens(),
    };
    let tool_schemas = std::iter::once((ToolSchemaOrigin::BuiltIn, buckets.tools_builtin))
        .chain(
            buckets
                .tools_mcp
                .into_iter()
                .map(|(server, schemas)| (ToolSchemaOrigin::Mcp { server }, schemas)),
        )
        .map(|(origin, (tokens, schemas))| ContextToolSchemaPart {
            origin,
            schemas: schemas as u64,
            tokens: scale.tokens(tokens),
        })
        .filter(|part| part.tokens > 0);
    let messages = [
        (ContextMessageKind::Assistant, buckets.assistant),
        (ContextMessageKind::User, buckets.user),
        (
            ContextMessageKind::CompactionSummary,
            buckets.compaction_summary,
        ),
        (ContextMessageKind::SystemNotice, buckets.system_notices),
        (ContextMessageKind::Unclassified, buckets.other),
    ]
    .into_iter()
    .map(|(kind, tokens)| ContextMessagePart {
        kind,
        tokens: scale.tokens(tokens),
    })
    .filter(|part| part.tokens > 0);
    let tool_results = buckets
        .tool_results
        .into_iter()
        .map(|(tool, tokens)| ContextToolResultPart {
            tool,
            tokens: scale.tokens(tokens),
        })
        .filter(|part| part.tokens > 0);
    SessionContextReport {
        tokens: estimate.tokens(),
        basis: if estimate.provider_reported_tokens().is_some() {
            rho_protocol::ContextBasis::ProviderCalibrated
        } else {
            rho_protocol::ContextBasis::LocalEstimate
        },
        window_tokens: window,
        system_prompt: system_prompt_parts(buckets.system_prompt, prompt_sources)
            .into_iter()
            .map(|part| ContextPromptPart {
                tokens: scale.tokens(part.tokens),
                ..part
            })
            .filter(|part| part.tokens > 0)
            .collect(),
        tool_schemas: largest_first(tool_schemas, |part| part.tokens),
        messages: largest_first(messages, |part| part.tokens),
        tool_results: largest_first(tool_results, |part| part.tokens),
        request_context_tokens: scale.tokens(buckets.request_context),
    }
}

/// `parts` largest first; equal sizes keep their order.
fn largest_first<T>(parts: impl Iterator<Item = T>, tokens: impl Fn(&T) -> u64) -> Vec<T> {
    let mut parts: Vec<T> = parts.collect();
    parts.sort_by_key(|part| std::cmp::Reverse(tokens(part)));
    parts
}

impl From<&SessionContextReport> for ContextReport {
    fn from(report: &SessionContextReport) -> Self {
        let system_prompt = report
            .system_prompt
            .iter()
            .map(|part| ContextRow {
                label: prompt_source_label(part.source).into(),
                detail: part
                    .path
                    .as_deref()
                    .map(Path::display)
                    .map(|path| path.to_string()),
                tokens: part.tokens,
            })
            .collect();
        let tool_schemas = report
            .tool_schemas
            .iter()
            .map(|part| ContextRow {
                label: match &part.origin {
                    ToolSchemaOrigin::BuiltIn => "Built-in tools".into(),
                    ToolSchemaOrigin::Mcp { server } => format!("MCP {server}"),
                    // A daemon fills its own report, so nothing it sends is unknown.
                    _ => "Other tools".into(),
                },
                detail: Some(format!(
                    "{} {}",
                    part.schemas,
                    plural(part.schemas, "schema", "schemas")
                )),
                tokens: part.tokens,
            })
            .collect();
        let messages = report
            .messages
            .iter()
            .map(|part| row(message_label(part.kind), part.tokens))
            .collect();
        let tool_results = report
            .tool_results
            .iter()
            .map(|part| row(part.tool.as_deref().unwrap_or("unknown tool"), part.tokens))
            .collect();
        let request_context = vec![row(
            "Host context sent with each request",
            report.request_context_tokens,
        )];
        Self {
            tokens: report.tokens,
            basis: match report.basis {
                rho_protocol::ContextBasis::ProviderCalibrated => ContextBasis::ProviderCalibrated,
                rho_protocol::ContextBasis::LocalEstimate => ContextBasis::LocalEstimate,
                _ => ContextBasis::LocalEstimate,
            },
            window: report.window_tokens,
            groups: [
                group("System prompt", system_prompt),
                group("Tool schemas", tool_schemas),
                group("Messages", messages),
                group("Tool results", tool_results),
                group("Request context", request_context),
            ]
            .into_iter()
            .flatten()
            .collect(),
        }
    }
}

fn prompt_source_label(source: WirePromptSource) -> &'static str {
    match source {
        WirePromptSource::Base => "Rho instructions",
        WirePromptSource::ModelAppend => "Model prompt (append)",
        WirePromptSource::ModelReplace => "Model prompt (replace)",
        WirePromptSource::InstructionFile => "Instruction file",
        WirePromptSource::Skills => "Skills listing",
        WirePromptSource::Unattributed => "System prompt",
        _ => "Other",
    }
}

fn message_label(kind: ContextMessageKind) -> &'static str {
    match kind {
        ContextMessageKind::Assistant => "Assistant (text, reasoning, tool calls)",
        ContextMessageKind::User => "User and host notes",
        ContextMessageKind::CompactionSummary => "Compaction summary",
        ContextMessageKind::SystemNotice => "System notices",
        ContextMessageKind::Unclassified => "Other",
        _ => "Other",
    }
}

/// Maps local estimates onto the calibrated total's scale.
struct Scale {
    total: u64,
    local: u64,
}

impl Scale {
    fn tokens(&self, local: u64) -> u64 {
        if self.local == 0 {
            return local;
        }
        (u128::from(local) * u128::from(self.total) / u128::from(self.local))
            .try_into()
            .unwrap_or(u64::MAX)
    }
}

fn row(label: impl Into<String>, tokens: u64) -> ContextRow {
    ContextRow {
        label: label.into(),
        detail: None,
        tokens,
    }
}

/// Drops empty rows; `None` when nothing remains.
fn group(label: &'static str, rows: Vec<ContextRow>) -> Option<ContextGroup> {
    let rows: Vec<_> = rows.into_iter().filter(|row| row.tokens > 0).collect();
    let tokens = rows.iter().map(|row| row.tokens).sum();
    (tokens > 0).then_some(ContextGroup {
        label: label.into(),
        tokens,
        rows,
    })
}

/// Splits the system message by recorded source bytes, in prompt order, on
/// the local scale.
fn system_prompt_parts(tokens: u64, sources: &[PromptSource]) -> Vec<ContextPromptPart> {
    let weights: Vec<u64> = sources.iter().map(|source| source.bytes as u64).collect();
    let Some(shares) = largest_remainder(tokens, &weights) else {
        return vec![ContextPromptPart {
            source: WirePromptSource::Unattributed,
            path: None,
            tokens,
        }];
    };
    sources
        .iter()
        .zip(shares)
        .map(|(source, tokens)| ContextPromptPart {
            source: match source.kind {
                PromptSourceKind::Base => WirePromptSource::Base,
                PromptSourceKind::ModelAppend => WirePromptSource::ModelAppend,
                PromptSourceKind::ModelReplace => WirePromptSource::ModelReplace,
                PromptSourceKind::Agents => WirePromptSource::InstructionFile,
                PromptSourceKind::Skills => WirePromptSource::Skills,
            },
            path: source.path.clone().map(Into::into),
            tokens,
        })
        .collect()
}

/// Splits `total` in proportion to `weights` so the shares sum to `total`:
/// each gets its floored share, then leftover units go to the largest
/// fractional remainders, earlier entries first on ties. `None` when every
/// weight is zero.
fn largest_remainder(total: u64, weights: &[u64]) -> Option<Vec<u64>> {
    let weight_sum: u128 = weights.iter().map(|weight| u128::from(*weight)).sum();
    if weight_sum == 0 {
        return None;
    }
    let exact: Vec<u128> = weights
        .iter()
        .map(|weight| u128::from(total) * u128::from(*weight))
        .collect();
    // Each floored share is at most `total`, so it fits in u64.
    let mut shares: Vec<u64> = exact
        .iter()
        .map(|exact| (exact / weight_sum) as u64)
        .collect();
    let mut order: Vec<usize> = (0..weights.len()).collect();
    order.sort_by_key(|&index| std::cmp::Reverse(exact[index] % weight_sum));
    let leftover = total - shares.iter().sum::<u64>();
    for &index in order.iter().take(leftover as usize) {
        shares[index] += 1;
    }
    Some(shares)
}

fn plural(count: u64, one: &'static str, many: &'static str) -> &'static str {
    if count == 1 {
        one
    } else {
        many
    }
}

#[cfg(test)]
#[path = "context_report_tests.rs"]
mod tests;
