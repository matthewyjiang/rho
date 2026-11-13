use std::{path::PathBuf, time::Duration};

use ratatui::text::Line;
use rho_providers::model::{ContextUsage, ContextUsageSource, ModelMetadata, ModelUsage};

use super::{
    command_block::CommandBlock,
    model_performance::ModelPerformanceSummary,
    types::ExtraCosts,
    usage_cost::{
        cache_hit_percent, context_fill_percent, display_input_tokens, format_usd,
        resolved_context_window, resolved_usage_cost_usd_micros, session_total_cost_usd_micros,
        CostSource,
    },
    workspace::git_branch,
    App,
};
use crate::agent::AgentRuntime;
use crate::app::compaction_report::CompactionReport;
use crate::claude_runtime::auth;
use crate::presentation::number::grouped as format_number;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BillingInfo {
    Metered,
    Subscription,
}

impl BillingInfo {
    fn from_provider_auth(provider: &str, auth: &str) -> Self {
        if provider == "openai-codex"
            || auth == "codex"
            || auth == "xai-oauth"
            || auth == "meta-muse"
        {
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
    /// Directories added to the session's workspace scope.
    added_dirs: Vec<PathBuf>,
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
    /// The Compaction section, read from the daemon after the overlay
    /// opens.
    compaction: CompactionSection,
    model_metadata: Option<ModelMetadata>,
    tree: Option<crate::session::tree::SessionTreeFacts>,
    tree_error: Option<String>,
    /// Session tree is read off the open path so a large file cannot delay the overlay.
    tree_loading: bool,
    /// Probed auth summaries for runtimes outside provider credentials
    /// (Claude, Cursor, Antigravity).
    external_runtimes: Vec<String>,
    /// Non-main spend for this conversation.
    extra: super::types::ExtraCosts,
}

/// What `/info` knows of the session's compaction.
#[derive(Clone, Debug, Default)]
enum CompactionSection {
    /// Nothing to show, or not read yet.
    #[default]
    Absent,
    Report(CompactionReport),
    /// Why the daemon could not report it.
    Unavailable(String),
}

impl RuntimeInfo {
    /// Fills the Compaction section with what the daemon answered.
    pub(super) fn set_compaction(&mut self, report: Result<CompactionReport, String>) {
        self.compaction = match report {
            Ok(report) => CompactionSection::Report(report),
            Err(error) => CompactionSection::Unavailable(error),
        };
    }

    pub(super) fn tree_loading(&self) -> bool {
        self.tree_loading
    }

    #[cfg(test)]
    pub(super) fn external_runtimes(&self) -> &[String] {
        &self.external_runtimes
    }

    pub(super) fn set_external_runtimes(&mut self, lines: Vec<String>) {
        self.external_runtimes = lines;
    }

    pub(super) fn set_tree(
        &mut self,
        tree: Option<crate::session::tree::SessionTreeFacts>,
        error: Option<String>,
    ) {
        self.tree = tree;
        self.tree_error = error;
        self.tree_loading = false;
    }

    pub(super) fn begin_tree_load(&mut self) {
        self.tree = None;
        self.tree_error = None;
        self.tree_loading = true;
    }
}

impl App {
    /// Open `/info` from in-memory state. External probes and the session tree
    /// fill in afterwards; this must not await either.
    pub(super) fn execute_info_command(&mut self) -> anyhow::Result<()> {
        let identity = self.info.services.diagnostics.identity();
        // A live turn may still be writing the tree. Read it only when idle.
        let (tree_error, tree_loading) = match self.current_session_id().as_deref() {
            Some(_) if self.is_ui_busy() => (
                Some("tree facts are available after the current model turn".into()),
                false,
            ),
            Some(_) => (None, true),
            None => (None, false),
        };
        // Loading stays false until the turn ends so the unavailable note
        // remains. The idle poll starts the read if the overlay is still open.
        self.info_tree_deferred = tree_error.is_some();
        let info = RuntimeInfo {
            version: identity.rho_version.to_string(),
            provider: identity.provider.to_string(),
            // `/info` keeps the id beside the name: it is where a person
            // looks the id up.
            model: match self.model_names().name(&identity.provider, &identity.model) {
                Some(name) => format!("{name} ({})", identity.model),
                None => identity.model.to_string(),
            },
            reasoning: identity.reasoning.to_string(),
            permission_mode: self.info.runtime.permission_mode.as_str().into(),
            advisor: self.advisor_status().detail(),
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
            added_dirs: self.info.runtime.added_dirs.as_slice().to_vec(),
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
            // The daemon reports it once asked, below.
            compaction: CompactionSection::Absent,
            model_metadata: self.model_metadata.clone(),
            tree: None,
            tree_error,
            tree_loading,
            external_runtimes: checking_external_runtimes(),
            extra: self.usage.extra,
        };
        self.show_info_overlay(info);
        self.start_info_refresh();
        self.open_daemon_info_compaction();
        Ok(())
    }
}

/// Placeholder rows painted before the Claude, Cursor, and Antigravity
/// probes return.
pub(super) fn checking_external_runtimes() -> Vec<String> {
    vec![
        "claude code: checking…".into(),
        format!("{}: checking…", AgentRuntime::Cursor.as_str()),
        format!("{}: checking…", AgentRuntime::Antigravity.as_str()),
    ]
}

pub(super) async fn load_external_runtimes() -> Vec<String> {
    let (claude, cursor, antigravity) = tokio::join!(
        auth::probe_snapshot(),
        crate::cursor_runtime::auth::query(),
        crate::antigravity_runtime::auth::query(),
    );
    vec![
        claude.auth_description(),
        match cursor {
            Ok(status) => status.auth_description(),
            Err(error) => error.to_string(),
        },
        match antigravity {
            Ok(status) => format!(
                "{}: {}",
                AgentRuntime::Antigravity.as_str(),
                status.description()
            ),
            Err(error) => error.to_string(),
        },
    ]
}

pub(super) fn runtime_info_lines(info: &RuntimeInfo, width: usize) -> Vec<Line<'static>> {
    let mut block = CommandBlock::on_surface(width);
    block.push_header("rho", &format!("v{}", info.version));

    block.push_section("Model");
    block.push_field("Provider", &info.provider);
    block.push_field("Model", &info.model);
    block.push_field("Reasoning", &info.reasoning);
    block.push_field("Permissions", &info.permission_mode);
    block.push_field("Advisor", &info.advisor);
    block.push_field("Billing", info.billing.description());

    block.push_section("External runtimes");
    for line in &info.external_runtimes {
        block.push_note(line);
    }

    block.push_section("Session");
    if info.tree_loading {
        block.push_note("checking…");
    } else if let Some(tree) = &info.tree {
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

    match &info.compaction {
        CompactionSection::Report(report) if !report.rows.is_empty() => {
            block.push_section("Compaction");
            for row in &report.rows {
                block.push_field(&row.label, &row.value);
            }
        }
        CompactionSection::Unavailable(error) => {
            block.push_section("Compaction");
            block.push_note(&format!("Compaction unavailable: {error}"));
        }
        CompactionSection::Report(_) | CompactionSection::Absent => {}
    }

    if let Some(metrics) = info.model_performance.latest_call {
        block.push_section("Last model call");
        if let Some(duration) = metrics.time_to_first_token {
            block.push_field("First event", &format_duration(duration));
        }
        if let Some(duration) = metrics.generation_time {
            block.push_field("Generation", &format_duration(duration));
        }
        push_optional_number(
            &mut block,
            "Generation tokens",
            metrics.resolved_generation_tokens(),
        );
        push_optional_number(&mut block, "Output tokens", metrics.output_tokens);
        if let Some(rate) = metrics.generation_tokens_per_second() {
            block.push_field("Generation rate", &format!("{rate:.1} tok/s"));
        }
        if let Some(rate) = metrics.response_tokens_per_second() {
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
    for dir in &info.added_dirs {
        block.push_field("Added", &dir.display().to_string());
    }
    block.push_field(
        "Git branch",
        info.branch.as_deref().unwrap_or("not in a Git worktree"),
    );
    block.finish()
}

/// Plain-text report for the clipboard. Wider than the panel so a pasted
/// field stays one line instead of keeping the on-screen wrap.
const INFO_COPY_WIDTH: usize = 200;

pub(super) fn info_copy_text(info: &RuntimeInfo) -> String {
    runtime_info_lines(info, INFO_COPY_WIDTH)
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn push_usage_fields(block: &mut CommandBlock, info: &RuntimeInfo) {
    if let Some(context) =
        format_context_row(info.context_usage.as_ref(), info.model_metadata.as_ref())
    {
        block.push_field("Context", &context);
    } else {
        block.push_field("Context", "not reported");
    }

    let Some(usage) = info.usage.as_ref() else {
        if info.extra.total() == 0 {
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
    let extra = info.extra;
    let Some(total_micros) = session_total_cost_usd_micros(main_cost_micros, extra.total()) else {
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
        extra,
        total_micros,
        main_qualifier,
        equivalent,
    ) {
        block.push_field(label, &value);
    }
}

/// Labels for /info cost rows. One component stays a single labeled field;
/// multiple components break out each component plus Total.
fn cost_field_rows(
    main_cost_micros: Option<u64>,
    extra: ExtraCosts,
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
    for (label, amount) in [
        ("Subagent cost", extra.subagent),
        ("Advisor cost", extra.advisor),
        ("Cache warm cost", extra.cache_warm),
    ] {
        if amount > 0 {
            rows.push((label, format_amount(amount, "")));
        }
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

/// Formats the `/info` context row; shows consumption even when no limit is
/// reported.
fn format_context_row(
    context_usage: Option<&ContextUsage>,
    model_metadata: Option<&ModelMetadata>,
) -> Option<String> {
    let window = resolved_context_window(context_usage, model_metadata);
    let source = match context_usage.map(|usage| usage.source) {
        Some(ContextUsageSource::Estimated) => "estimated",
        Some(ContextUsageSource::ProviderReported) => "provider reported",
        Some(ContextUsageSource::UnknownAfterCompaction) => "unknown after compaction",
        None => "model limit",
    };
    let Some(tokens) = context_usage.and_then(|usage| usage.tokens) else {
        // Nothing reported at all: let the caller render its fallback row.
        let window = window?;
        return Some(format!(
            "unknown / {} tokens ({source})",
            format_number(window)
        ));
    };
    let Some(window) = window else {
        // Without a known limit there is no fill percent; still show consumption.
        return Some(format!("{} tokens ({source})", format_number(tokens)));
    };
    let percent = context_fill_percent(tokens, window);
    Some(format!(
        "{} / {} tokens ({percent:.1}%, {source})",
        format_number(tokens),
        format_number(window)
    ))
}

fn format_duration(duration: Duration) -> String {
    if duration < Duration::from_secs(1) {
        format!("{} ms", duration.as_millis())
    } else {
        format!("{:.1} s", duration.as_secs_f64())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels<'a>(rows: &'a [(&'a str, String)]) -> Vec<&'a str> {
        rows.iter().map(|(label, _)| *label).collect()
    }

    fn extra(subagent: u64, advisor: u64, cache_warm: u64) -> ExtraCosts {
        ExtraCosts {
            subagent,
            advisor,
            cache_warm,
        }
    }

    #[test]
    fn cost_rows_cover_single_and_multi_component_layouts() {
        let cases: [(Option<u64>, ExtraCosts, &[&str]); 7] = [
            (Some(1_000), extra(0, 0, 0), &["Cost"]),
            (None, extra(2_000, 0, 0), &["Subagent cost"]),
            (None, extra(0, 3_000, 0), &["Advisor cost"]),
            (
                Some(1_000),
                extra(0, 3_000, 0),
                &["Main cost", "Advisor cost", "Total cost"],
            ),
            (
                Some(1_000),
                extra(2_000, 3_000, 0),
                &["Main cost", "Subagent cost", "Advisor cost", "Total cost"],
            ),
            (
                None,
                extra(2_000, 3_000, 0),
                &["Subagent cost", "Advisor cost", "Total cost"],
            ),
            (
                Some(1_000),
                extra(0, 0, 400),
                &["Main cost", "Cache warm cost", "Total cost"],
            ),
        ];
        for (main, extra, expected) in cases {
            let total = main.unwrap_or(0) + extra.total();
            assert_eq!(
                labels(&cost_field_rows(main, extra, total, "", "")),
                expected
            );
        }
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

    // Covers: the /info context row shows consumption without a reported
    // limit, uses metadata when usage is absent, and falls back to the
    // caller's "not reported" row when nothing is known.
    // Owner: /info context row
    #[test]
    fn context_row_formats_usage_and_limit_combinations() {
        let mut metadata = ModelMetadata::default();
        metadata.advertised_context_window = Some(200_000);
        for (case, usage, metadata, expected) in [
            (
                "tokens without a reported limit",
                Some(ContextUsage::estimated(123_456, None)),
                None,
                Some("123,456 tokens (estimated)"),
            ),
            (
                "tokens with a reported limit",
                Some(ContextUsage::estimated(50_000, Some(200_000))),
                None,
                Some("50,000 / 200,000 tokens (25.0%, estimated)"),
            ),
            ("nothing known", None, None, None),
            (
                "metadata window without usage",
                None,
                Some(&metadata),
                Some("unknown / 200,000 tokens (model limit)"),
            ),
        ] {
            assert_eq!(
                format_context_row(usage.as_ref(), metadata),
                expected.map(String::from),
                "{case}"
            );
        }
    }

    #[test]
    fn cost_rows_preserve_main_qualifier_and_amounts() {
        let rows = cost_field_rows(
            Some(1_500_000),
            extra(500_000, 0, 0),
            2_000_000,
            " estimated",
            "",
        );
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
