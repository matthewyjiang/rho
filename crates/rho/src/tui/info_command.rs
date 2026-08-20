use std::{path::PathBuf, time::Duration};

use ratatui::text::Line;
use rho_providers::model::{ContextUsage, ContextUsageSource, ModelMetadata, ModelUsage};

use super::{
    command_block::CommandBlock,
    model_performance::ModelPerformanceSummary,
    usage_cost::{
        display_input_tokens, format_usd, resolved_usage_cost_usd_micros,
        session_total_cost_usd_micros, CostSource,
    },
    workspace::git_branch,
    App, Entry,
};
use crate::claude_runtime::auth::{self, ClaudeProbeSnapshot};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BillingInfo {
    Metered,
    Subscription,
}

impl BillingInfo {
    fn from_provider_auth(provider: &str, auth: &str) -> Self {
        if provider == "openai-codex" || auth == "codex" || auth == "xai-oauth" {
            Self::Subscription
        } else {
            Self::Metered
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::Metered => "metered API",
            Self::Subscription => "subscription",
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct RuntimeInfo {
    version: String,
    provider: String,
    model: String,
    reasoning: String,
    permission_mode: String,
    advisor: String,
    billing: BillingInfo,
    cost_source: CostSource,
    cwd: PathBuf,
    branch: Option<String>,
    usage: Option<ModelUsage>,
    latest_usage: Option<ModelUsage>,
    /// Session cache hit rate over durable usage only.
    ///
    /// Kept separate from [`Self::usage`], which folds in the display-only live
    /// stream estimate. That estimate reports fabricated uncached input and no
    /// cache fields, so folding it in would dilute the rate mid-turn.
    session_cache_hit_percent: Option<f64>,
    cache_rebilled: super::cache_stats::CacheRebilled,
    model_performance: ModelPerformanceSummary,
    context_usage: Option<ContextUsage>,
    model_metadata: Option<ModelMetadata>,
    tree: Option<crate::session::tree::SessionTreeFacts>,
    tree_error: Option<String>,
    /// Claude Code auth summary. Outside provider credentials on purpose.
    claude_code: String,
    /// Cumulative cost from all completed subagents, including failed/canceled ones.
    subagent_total_cost_usd_micros: u64,
    /// Cumulative cost from finished advisor calls in this conversation.
    advisor_total_cost_usd_micros: u64,
}

impl App {
    pub(super) async fn execute_info_command(&mut self) -> anyhow::Result<()> {
        let identity = self.info.services.diagnostics.identity();
        let (tree, tree_error) = match self.info.session.session_id.as_deref() {
            Some(_) if self.is_ui_busy() => (
                None,
                Some("tree facts are available after the current model turn".into()),
            ),
            Some(id) => match crate::session::Session::tree_facts_by_id(&self.info.runtime.cwd, id)
            {
                Ok(facts) => (Some(facts), None),
                Err(error) => (None, Some(error.to_string())),
            },
            None => (None, None),
        };
        // During a turn never block stream draining on a Claude child probe.
        let claude_code = if self.is_ui_busy() {
            ClaudeProbeSnapshot::not_refreshed_during_turn().auth_description()
        } else {
            self.claude_probe_snapshot().await.auth_description()
        };
        let info = RuntimeInfo {
            version: identity.rho_version.to_string(),
            provider: identity.provider.to_string(),
            model: identity.model.to_string(),
            reasoning: identity.reasoning.to_string(),
            permission_mode: self.info.runtime.permission_mode.as_str().into(),
            advisor: super::advisor_status::AdvisorStatus::from_runtime(&self.info.runtime)
                .detail(),
            billing: BillingInfo::from_provider_auth(
                &self.info.runtime.provider,
                &self.info.runtime.auth,
            ),
            cost_source: if self.usage.live_stream.is_active() {
                CostSource::Estimated
            } else {
                self.usage.usage_cost_tracker.cumulative_source()
            },
            cwd: self.info.runtime.cwd.clone(),
            branch: git_branch(&self.info.runtime.cwd),
            usage: super::usage_cost::display_usage_with_live(
                self.usage.cumulative_usage.as_ref(),
                &self.usage.live_stream,
                self.model_metadata.as_ref(),
            ),
            latest_usage: self.usage.latest_usage.clone(),
            session_cache_hit_percent: self
                .usage
                .cumulative_usage
                .as_ref()
                .and_then(cache_hit_percent),
            cache_rebilled: self.usage.cache_stats.rebilled().clone(),
            model_performance: self
                .usage
                .model_performance
                .summary(&self.info.runtime.model_call_profile()),
            context_usage: self.usage.current_context.clone(),
            model_metadata: self.model_metadata.clone(),
            tree,
            tree_error,
            claude_code,
            subagent_total_cost_usd_micros: self.usage.subagent_total_cost_usd_micros,
            advisor_total_cost_usd_micros: self.usage.advisor_total_cost_usd_micros,
        };
        self.insert_entry(&Entry::RuntimeInfo(Box::new(info)));
        self.set_status("runtime info");
        Ok(())
    }

    /// Live Claude probe for idle surfaces.
    pub(super) async fn claude_probe_snapshot(&self) -> ClaudeProbeSnapshot {
        auth::probe_snapshot().await
    }
}

pub(super) fn runtime_info_lines(info: &RuntimeInfo, width: usize) -> Vec<Line<'static>> {
    let mut block = CommandBlock::new(width);
    block.push_header("rho", &format!("v{}", info.version));

    block.push_section("Model");
    block.push_field("Provider", &info.provider);
    block.push_field("Model", &info.model);
    block.push_field("Reasoning", &info.reasoning);
    block.push_field("Permissions", &info.permission_mode);
    block.push_field("Advisor", &info.advisor);
    block.push_field("Billing", info.billing.description());

    block.push_section("External runtimes");
    block.push_note(&info.claude_code);

    block.push_section("Session");
    if let Some(tree) = &info.tree {
        block.push_field(
            "Active leaf",
            tree.active_leaf_id
                .as_ref()
                .map_or("none", |id| id.as_str()),
        );
        block.push_field("Nodes", &tree.node_count.to_string());
        block.push_field("Branches", &tree.branch_count.to_string());
    } else if let Some(error) = &info.tree_error {
        block.push_note(&format!("Session tree unavailable: {error}"));
    } else {
        block.push_note("No durable conversation state yet.");
    }

    block.push_section("Session usage");
    push_usage_fields(&mut block, info);

    if let Some(call) = info.model_performance.latest_call {
        let metrics = call.metrics;
        let generation_output_tokens = call.throughput_output_tokens();
        block.push_section("Last model call");
        if let Some(duration) = metrics.time_to_first_token {
            block.push_field("First event", &format_duration(duration));
        }
        if let Some(duration) = metrics.generation_time {
            block.push_field("Generation", &format_duration(duration));
        }
        push_optional_number(&mut block, "Generation tokens", generation_output_tokens);
        push_optional_number(&mut block, "Output tokens", metrics.output_tokens);
        // Compute rates from published metric fields. Do not call
        // ModelCallMetrics::{generation,response}_tokens_per_second here until
        // a released rho-sdk cut exports them; package verify builds against
        // crates.io.
        if let Some(rate) = metrics
            .generation_time
            .and_then(|window| tokens_per_second(generation_output_tokens, window))
        {
            block.push_field("Generation rate", &format!("{rate:.1} tok/s"));
        }
        if let Some(rate) = tokens_per_second(metrics.output_tokens, metrics.total_latency) {
            block.push_field("Response rate", &format!("{rate:.1} tok/s"));
        }
        block.push_field("Total latency", &format_duration(metrics.total_latency));
    }

    if let Some(rate) = info.model_performance.average_generation_tokens_per_second {
        block.push_section("Model performance");
        block.push_field("Average generation", &format!("{rate:.1} tok/s"));
        block.push_field(
            "Samples",
            &info.model_performance.eligible_calls.to_string(),
        );
    }

    block.push_section("Workspace");
    block.push_field("Directory", &info.cwd.display().to_string());
    block.push_field(
        "Git branch",
        info.branch.as_deref().unwrap_or("not in a Git worktree"),
    );
    block.finish()
}

fn push_usage_fields(block: &mut CommandBlock, info: &RuntimeInfo) {
    if let Some(context) = format_context(info) {
        block.push_field("Context", &context);
    } else {
        block.push_field("Context", "not reported");
    }

    let Some(usage) = info.usage.as_ref() else {
        if info.subagent_total_cost_usd_micros == 0 && info.advisor_total_cost_usd_micros == 0 {
            block.push_note("No token usage recorded yet.");
        } else {
            push_cost_fields(block, info, None);
        }
        return;
    };

    push_optional_number(block, "Input tokens", display_input_tokens(usage));
    push_optional_number(block, "Output tokens", usage.output_tokens);
    push_optional_number(block, "Cache read", usage.cache_read_tokens);
    push_optional_number(block, "Cache write", usage.cache_write_tokens);
    if let Some(hit) = format_cache_hit(info) {
        block.push_field("Cache hit", &hit);
    }
    if let Some(rebilled) = format_cache_rebilled(&info.cache_rebilled, info.billing) {
        block.push_field("Cache re-billed", &rebilled);
    }

    let main_cost_micros = resolved_usage_cost_usd_micros(usage, info.model_metadata.as_ref());
    push_cost_fields(block, info, main_cost_micros);
}

fn push_cost_fields(block: &mut CommandBlock, info: &RuntimeInfo, main_cost_micros: Option<u64>) {
    let subagent = info.subagent_total_cost_usd_micros;
    let advisor = info.advisor_total_cost_usd_micros;
    let extra = subagent.saturating_add(advisor);
    let Some(total_micros) = session_total_cost_usd_micros(main_cost_micros, extra) else {
        return;
    };
    let equivalent = cost_equivalent_suffix(info.billing);
    let main_qualifier = if info.cost_source == CostSource::Estimated {
        " estimated"
    } else {
        ""
    };

    for (label, value) in cost_field_rows(
        main_cost_micros,
        subagent,
        advisor,
        total_micros,
        main_qualifier,
        equivalent,
    ) {
        block.push_field(label, &value);
    }
}

/// Labels for /info cost rows. One component stays a single labeled field;
/// multiple components break out Main/Subagent/Advisor plus Total.
fn cost_field_rows(
    main_cost_micros: Option<u64>,
    subagent: u64,
    advisor: u64,
    total_micros: u64,
    main_qualifier: &str,
    equivalent: &str,
) -> Vec<(&'static str, String)> {
    let format_amount =
        |amount: u64, qualifier: &str| format!("{}{qualifier}{equivalent}", format_usd(amount));

    let mut rows = Vec::new();
    if let Some(main) = main_cost_micros {
        rows.push(("Main cost", format_amount(main, main_qualifier)));
    }
    if subagent > 0 {
        rows.push(("Subagent cost", format_amount(subagent, "")));
    }
    if advisor > 0 {
        rows.push(("Advisor cost", format_amount(advisor, "")));
    }

    match rows.len() {
        0 => rows,
        1 => {
            if rows[0].0 == "Main cost" {
                rows[0].0 = "Cost";
            }
            rows
        }
        _ => {
            rows.push(("Total cost", format_amount(total_micros, "")));
            rows
        }
    }
}

fn cost_equivalent_suffix(billing: BillingInfo) -> &'static str {
    if billing == BillingInfo::Subscription {
        " API equivalent"
    } else {
        ""
    }
}

fn push_optional_number(block: &mut CommandBlock, label: &str, value: Option<u64>) {
    if let Some(value) = value {
        block.push_field(label, &format_number(value));
    }
}

fn cache_hit_percent(usage: &ModelUsage) -> Option<f64> {
    let cache_read = usage.cache_read_tokens?;
    let prompt_tokens = usage.inclusive_prompt_tokens()?;
    (prompt_tokens > 0).then(|| cache_read as f64 * 100.0 / prompt_tokens as f64)
}

/// Single cache-hit line covering session and latest-request rates.
fn format_cache_hit(info: &RuntimeInfo) -> Option<String> {
    let latest = info.latest_usage.as_ref().and_then(cache_hit_percent);
    match (info.session_cache_hit_percent, latest) {
        (Some(session), Some(latest)) => Some(format!(
            "{session:.1}% session · {latest:.1}% latest request"
        )),
        (Some(session), None) => Some(format!("{session:.1}% session")),
        (None, Some(latest)) => Some(format!("{latest:.1}% on the latest request")),
        (None, None) => None,
    }
}

/// Session re-bill line for `/info`. Hidden until at least one counted miss.
fn format_cache_rebilled(
    rebilled: &super::cache_stats::CacheRebilled,
    billing: BillingInfo,
) -> Option<String> {
    if rebilled.miss_count == 0 {
        return None;
    }
    let miss_label = if rebilled.miss_count == 1 {
        "1 miss".to_string()
    } else {
        format!("{} misses", rebilled.miss_count)
    };
    let tokens = format_number(rebilled.missed_tokens);
    Some(if rebilled.extra_cost_usd_micros > 0 {
        let cost = format_usd(rebilled.extra_cost_usd_micros);
        let cost = if rebilled.unpriced_miss_count > 0 {
            format!("{cost} partial")
        } else {
            cost
        };
        format!(
            "{cost} ({tokens} tokens, {miss_label}){}",
            cost_equivalent_suffix(billing)
        )
    } else {
        format!("{tokens} tokens ({miss_label})")
    })
}

fn format_context(info: &RuntimeInfo) -> Option<String> {
    let window = info
        .context_usage
        .as_ref()
        .and_then(|usage| usage.context_window)
        .or_else(|| {
            info.model_metadata
                .as_ref()
                .and_then(ModelMetadata::display_context_window)
        })
        .filter(|window| *window > 0)?;
    let source = match info.context_usage.as_ref().map(|usage| usage.source) {
        Some(ContextUsageSource::Estimated) => "estimated",
        Some(ContextUsageSource::ProviderReported) => "provider reported",
        Some(ContextUsageSource::UnknownAfterCompaction) => "unknown after compaction",
        None => "model limit",
    };
    let Some(tokens) = info.context_usage.as_ref().and_then(|usage| usage.tokens) else {
        return Some(format!(
            "unknown / {} tokens ({source})",
            format_number(window)
        ));
    };
    let percent = tokens as f64 * 100.0 / window as f64;
    Some(format!(
        "{} / {} tokens ({percent:.1}%, {source})",
        format_number(tokens),
        format_number(window)
    ))
}

fn tokens_per_second(tokens: Option<u64>, window: Duration) -> Option<f64> {
    let tokens = tokens?;
    let seconds = window.as_secs_f64();
    (seconds > 0.0).then(|| tokens as f64 / seconds)
}

fn format_duration(duration: Duration) -> String {
    if duration < Duration::from_secs(1) {
        format!("{} ms", duration.as_millis())
    } else {
        format!("{:.1} s", duration.as_secs_f64())
    }
}

fn format_number(value: u64) -> String {
    let digits = value.to_string();
    let mut formatted = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            formatted.push(',');
        }
        formatted.push(ch);
    }
    formatted
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels<'a>(rows: &'a [(&'a str, String)]) -> Vec<&'a str> {
        rows.iter().map(|(label, _)| *label).collect()
    }

    #[test]
    fn cost_rows_cover_single_and_multi_component_layouts() {
        assert_eq!(
            labels(&cost_field_rows(Some(1_000), 0, 0, 1_000, "", "")),
            ["Cost"]
        );
        assert_eq!(
            labels(&cost_field_rows(None, 2_000, 0, 2_000, "", "")),
            ["Subagent cost"]
        );
        assert_eq!(
            labels(&cost_field_rows(None, 0, 3_000, 3_000, "", "")),
            ["Advisor cost"]
        );
        assert_eq!(
            labels(&cost_field_rows(Some(1_000), 0, 3_000, 4_000, "", "")),
            ["Main cost", "Advisor cost", "Total cost"]
        );
        assert_eq!(
            labels(&cost_field_rows(Some(1_000), 2_000, 3_000, 6_000, "", "")),
            ["Main cost", "Subagent cost", "Advisor cost", "Total cost"]
        );
        assert_eq!(
            labels(&cost_field_rows(None, 2_000, 3_000, 5_000, "", "")),
            ["Subagent cost", "Advisor cost", "Total cost"]
        );
    }

    #[test]
    fn cache_hit_percent_uses_total_prompt_tokens() {
        let usage = ModelUsage {
            input_tokens: Some(5_000),
            cache_read_tokens: Some(95_000),
            cache_write_tokens: Some(0),
            ..ModelUsage::default()
        };
        assert_eq!(
            cache_hit_percent(&usage).map(|percent| format!("{percent:.1}")),
            Some("95.0".into())
        );
        // Providers that do not report cache accounting have no rate at all.
        assert_eq!(cache_hit_percent(&ModelUsage::default()), None);
    }

    #[test]
    fn cache_rebilled_field_covers_cost_and_count_forms() {
        use super::super::cache_stats::CacheRebilled;

        assert_eq!(
            format_cache_rebilled(&CacheRebilled::default(), BillingInfo::Metered),
            None
        );
        assert_eq!(
            format_cache_rebilled(
                &CacheRebilled {
                    missed_tokens: 45_230,
                    miss_count: 3,
                    extra_cost_usd_micros: 324_000,
                    unpriced_miss_count: 0,
                },
                BillingInfo::Metered,
            ),
            Some("$0.324 (45,230 tokens, 3 misses)".into())
        );
        assert_eq!(
            format_cache_rebilled(
                &CacheRebilled {
                    missed_tokens: 1_000,
                    miss_count: 1,
                    extra_cost_usd_micros: 0,
                    unpriced_miss_count: 1,
                },
                BillingInfo::Metered,
            ),
            Some("1,000 tokens (1 miss)".into())
        );
        assert_eq!(
            format_cache_rebilled(
                &CacheRebilled {
                    missed_tokens: 2_000,
                    miss_count: 2,
                    extra_cost_usd_micros: 100_000,
                    unpriced_miss_count: 0,
                },
                BillingInfo::Subscription,
            ),
            Some("$0.100 (2,000 tokens, 2 misses) API equivalent".into())
        );
        assert_eq!(
            format_cache_rebilled(
                &CacheRebilled {
                    missed_tokens: 8_000,
                    miss_count: 2,
                    extra_cost_usd_micros: 1_800,
                    unpriced_miss_count: 1,
                },
                BillingInfo::Metered,
            ),
            Some("$0.002 partial (8,000 tokens, 2 misses)".into())
        );
    }

    #[test]
    fn cost_rows_preserve_main_qualifier_and_amounts() {
        let rows = cost_field_rows(Some(1_500_000), 500_000, 0, 2_000_000, " estimated", "");
        assert_eq!(
            rows,
            vec![
                ("Main cost", "$1.500 estimated".into()),
                ("Subagent cost", "$0.500".into()),
                ("Total cost", "$2.000".into()),
            ]
        );
    }
}
