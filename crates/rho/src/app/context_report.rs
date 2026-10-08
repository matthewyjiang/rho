//! `/context` data: the next request's context window attributed by source.
//!
//! The SDK itemizes the request with its provider-neutral estimator. This
//! module owns Rho's grouping policy: system prompt sections from the prompt
//! source accounting, tool schemas split into built-in and per-MCP-server, and
//! history split by role, with tool results grouped by tool. Rows are scaled so
//! they share the calibrated total's scale when a provider measured the prompt.

use std::collections::{BTreeMap, HashMap};

use rho_sdk::{ContextBreakdown, ContextItem};

use crate::prompt::{PromptSource, PromptSourceKind};

/// What the header total is based on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ContextBasis {
    /// The last provider-reported prompt plus local estimates for later additions.
    ProviderCalibrated,
    /// Local character-count estimate only.
    LocalEstimate,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ContextRow {
    pub(crate) label: String,
    /// Secondary text such as a source path or a schema count.
    pub(crate) detail: Option<String>,
    pub(crate) tokens: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ContextGroup {
    pub(crate) label: &'static str,
    pub(crate) tokens: u64,
    pub(crate) rows: Vec<ContextRow>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
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
    /// Tool name, `None` for results whose call is not in history.
    tool_results: HashMap<Option<String>, u64>,
    request_context: u64,
    /// Items from a newer SDK without their own row yet.
    other: u64,
}

impl ContextReport {
    /// `mcp_tools` maps each exported MCP tool name to its server identity;
    /// other schemas count as built-in. `window` is the effective context limit.
    pub(crate) fn new(
        breakdown: &ContextBreakdown,
        prompt_sources: &[PromptSource],
        mcp_tools: &HashMap<String, String>,
        window: Option<u64>,
    ) -> Self {
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
        let messages = messages_group(&buckets);
        let groups = [
            system_prompt_group(buckets.system_prompt, prompt_sources),
            tools_group(buckets.tools_builtin, buckets.tools_mcp),
            messages,
            tool_results_group(buckets.tool_results),
            group(
                "Request context",
                vec![row(
                    "Host context sent with each request",
                    buckets.request_context,
                )],
            ),
        ]
        .into_iter()
        .filter_map(|group| group.map(|group| scale.group(group)))
        .collect();
        Self {
            tokens: estimate.tokens(),
            basis: if estimate.provider_reported_tokens().is_some() {
                ContextBasis::ProviderCalibrated
            } else {
                ContextBasis::LocalEstimate
            },
            window,
            groups,
        }
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

    fn group(&self, group: ContextGroup) -> ContextGroup {
        ContextGroup {
            tokens: self.tokens(group.tokens),
            rows: group
                .rows
                .into_iter()
                .map(|row| ContextRow {
                    tokens: self.tokens(row.tokens),
                    ..row
                })
                .collect(),
            ..group
        }
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
        label,
        tokens,
        rows,
    })
}

fn sorted_descending(mut rows: Vec<ContextRow>) -> Vec<ContextRow> {
    rows.sort_by(|a, b| b.tokens.cmp(&a.tokens).then_with(|| a.label.cmp(&b.label)));
    rows
}

/// Splits the system message by recorded source bytes, in prompt order.
fn system_prompt_group(tokens: u64, sources: &[PromptSource]) -> Option<ContextGroup> {
    let weights: Vec<u64> = sources.iter().map(|source| source.bytes as u64).collect();
    let Some(shares) = largest_remainder(tokens, &weights) else {
        return group("System prompt", vec![row("System prompt", tokens)]);
    };
    let rows = sources
        .iter()
        .zip(shares)
        .map(|(source, tokens)| ContextRow {
            label: match source.kind {
                PromptSourceKind::Base => "Rho instructions",
                PromptSourceKind::ModelAppend => "Model prompt (append)",
                PromptSourceKind::ModelReplace => "Model prompt (replace)",
                PromptSourceKind::Agents => "Instruction file",
                PromptSourceKind::Skills => "Skills listing",
            }
            .into(),
            detail: source.path.clone(),
            tokens,
        })
        .collect();
    group("System prompt", rows)
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

fn tools_group(builtin: (u64, usize), mcp: BTreeMap<String, (u64, usize)>) -> Option<ContextGroup> {
    let schemas = |count: usize| Some(format!("{count} {}", plural(count, "schema", "schemas")));
    let mut rows = vec![ContextRow {
        label: "Built-in tools".into(),
        detail: schemas(builtin.1),
        tokens: builtin.0,
    }];
    rows.extend(mcp.into_iter().map(|(server, (tokens, count))| ContextRow {
        label: format!("MCP {server}"),
        detail: schemas(count),
        tokens,
    }));
    group("Tool schemas", sorted_descending(rows))
}

fn messages_group(buckets: &Buckets) -> Option<ContextGroup> {
    group(
        "Messages",
        sorted_descending(vec![
            row("Assistant (text, reasoning, tool calls)", buckets.assistant),
            row("User and host notes", buckets.user),
            row("Compaction summary", buckets.compaction_summary),
            row("System notices", buckets.system_notices),
            row("Other", buckets.other),
        ]),
    )
}

fn tool_results_group(results: HashMap<Option<String>, u64>) -> Option<ContextGroup> {
    group(
        "Tool results",
        sorted_descending(
            results
                .into_iter()
                .map(|(tool, tokens)| row(tool.unwrap_or_else(|| "unknown tool".into()), tokens))
                .collect(),
        ),
    )
}

fn plural(count: usize, one: &'static str, many: &'static str) -> &'static str {
    if count == 1 {
        one
    } else {
        many
    }
}

#[cfg(test)]
#[path = "context_report_tests.rs"]
mod tests;
